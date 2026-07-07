mod grid;
mod render;
mod simulation;

use egui_macroquad::egui;
use macroquad::prelude::*;
use ::rand::Rng;

use render::Renderer;
use shared::ipc::{decode, read_frame, socket_path, write_msg, IPC_VERSION};
use shared::{
    config_panel, example_store, hue_for_index, is_video_path, scenes_path, AudioTarget,
    BeatAction, Boundary, Brush, ControlMsg, ControlState, InteractionMode, PanelEvent, PanelState,
    Playlist, SceneStore, SeqPlayback, ShapeStore, SimParams, TelemetryMsg, Tool, VideoSource,
    FRAME_PRESETS,
};
use simulation::Simulation;

use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Arc, Mutex};

fn window_conf() -> Conf {
    Conf {
        window_title: "Puntos de Atracción".to_owned(),
        window_width: 1280,
        window_height: 800,
        high_dpi: false,
        ..Default::default()
    }
}

/// Dónde se dibuja el panel de control.
#[derive(PartialEq, Clone, Copy)]
enum AppMode {
    /// Panel embebido como `SidePanel` dentro de esta ventana (por defecto).
    Embedded,
    /// Panel en una ventana del SO aparte (proceso `panel`), hablando por IPC.
    Detached,
}

/// Construye la cámara 2D para un nivel de zoom y un punto del mundo centrado.
/// Zoom mayor = se ve una porción más pequeña del mundo = más cerca.
///
/// `canvas` es la región de la ventana (en píxeles, esquina superior izquierda)
/// donde se dibuja el mundo: con el panel acoplado ocupa solo la parte libre a
/// su izquierda. Se aplica como `viewport` para que el render no invada el panel.
fn make_camera(zoom: f32, target: Vec2, canvas: Rect) -> Camera2D {
    let vw = canvas.w / zoom;
    let vh = canvas.h / zoom;
    let mut cam =
        Camera2D::from_display_rect(Rect::new(target.x - vw / 2.0, target.y - vh / 2.0, vw, vh));
    // El viewport de macroquad va en píxeles con el origen abajo-izquierda.
    cam.viewport = Some((
        canvas.x as i32,
        (screen_height() - canvas.y - canvas.h) as i32,
        canvas.w as i32,
        canvas.h as i32,
    ));
    cam
}

/// Convierte un punto de pantalla a mundo teniendo en cuenta el `viewport`.
/// (Los métodos de macroquad `screen_to_world`/`world_to_screen` asumen que la
/// cámara ocupa toda la ventana, así que dan mal la X con el panel acoplado.)
fn cam_s2w(cam: &Camera2D, p: Vec2, canvas: Rect) -> Vec2 {
    let ndc = vec2(
        (p.x - canvas.x) / canvas.w * 2.0 - 1.0,
        1.0 - (p.y - canvas.y) / canvas.h * 2.0,
    );
    let t = cam.matrix().inverse().project_point3(vec3(ndc.x, ndc.y, 0.0));
    vec2(t.x, t.y)
}

/// Inversa de [`cam_s2w`]: de mundo a pantalla respetando el `viewport`.
fn cam_w2s(cam: &Camera2D, p: Vec2, canvas: Rect) -> Vec2 {
    let t = cam.matrix().project_point3(vec3(p.x, p.y, 0.0));
    vec2(
        canvas.x + (t.x * 0.5 + 0.5) * canvas.w,
        canvas.y + (0.5 - t.y * 0.5) * canvas.h,
    )
}

// ----------------------------------------------------------------------------
// Grabación de vídeo vertical (TikTok): render offline a un `render_target` y
// volcado crudo (RGBA) a `ffmpeg` por stdin. Cada frame de la simulación es un
// frame del vídeo, así que el `.mp4` sale exacto a `REC_FPS` aunque el volcado
// vaya más lento que el tiempo real. El render se hace a `SSAA`× la resolución
// de salida (supersampling) y `ffmpeg` la reduce para bordes más nítidos.
// ----------------------------------------------------------------------------

const REC_FPS: i32 = 60;
/// Factor de supersampling de la grabación (antialias). 2 = 4× de píxeles.
const SSAA: u32 = 2;

/// Arrastre en curso del recuadro de encuadre.
#[derive(Clone, Copy)]
enum FrameDrag {
    Move,
    Resize,
}

/// Transición en curso de una escena a otra: interpola los parámetros numéricos
/// de `from` a `target`; el cruce del modo/matriz lo lleva el blend de
/// interacción (`start_transition`). La conduce el `sim`.
struct SceneMorph {
    from: Box<SimParams>,
    target: Box<SimParams>,
    blend: f32,
    dur: f32,
}

/// Aplica la escena `target` a `params`. Si `smooth`, arranca un morph y lo
/// devuelve; si no, la aplica al instante (devuelve `None`).
fn start_scene(
    params: &mut SimParams,
    target: &SimParams,
    smooth: bool,
    dur: f32,
) -> Option<SceneMorph> {
    if !smooth {
        *params = target.settled();
        return None;
    }
    let from = params.clone();
    let old_snap = params.current_snapshot();
    // Interacción destino + cruce gradual (viejo -> nuevo) con el blend existente.
    params.mode = target.mode;
    params.matrix = target.matrix;
    params.sim_range = target.sim_range;
    params.same_repel_others = target.same_repel_others;
    params.same_repel_strength = target.same_repel_strength;
    params.smooth = true;
    params.transition_duration = dur.max(0.05);
    params.start_transition(old_snap);
    // Discretos no-interacción: se fijan al destino de inmediato.
    params.boundary = target.boundary;
    params.style = target.style;
    params.random_color = target.random_color;
    params.gradual = target.gradual;
    params.color_smooth = target.color_smooth;
    params.speed_smooth = target.speed_smooth;
    params.attract_active = target.attract_active;
    params.auto_randomize = target.auto_randomize;
    // Descriptor de la forma (mensaje/imagen): el llamador reconstruye la forma.
    params.shape_text = target.shape_text.clone();
    params.shape_image = target.shape_image.clone();
    // Velocidad: por el sistema de transición de velocidad existente.
    params.set_speed(target.speed_target);
    Some(SceneMorph {
        from: Box::new(from),
        target: Box::new(target.clone()),
        blend: 0.0,
        dur: dur.max(0.05),
    })
}

/// Carga la escena en `idx + step` (con envoltura) de `store` sobre `params`,
/// actualizando `idx`. Devuelve el morph si la transición es suave.
fn cycle_scene(
    step: i32,
    store: &SceneStore,
    params: &mut SimParams,
    idx: &mut usize,
    smooth: bool,
    dur: f32,
) -> Option<SceneMorph> {
    let n = store.scenes.len();
    if n == 0 {
        return None;
    }
    *idx = (*idx as i32 + step).rem_euclid(n as i32) as usize;
    let target = store.scenes[*idx].params.clone();
    start_scene(params, &target, smooth, dur)
}

/// Estado runtime del secuenciador de escenas: la playlist (que edita el panel
/// y persiste en `playlist.json`) más la posición de reproducción del show.
struct Sequencer {
    playlist: Playlist,
    state: SeqPlayback,
    idx: usize,
    timer: f32,
}

impl Sequencer {
    /// Índice de la primera entrada cuya escena existe en `store`, empezando en
    /// `start` (incluida) y avanzando en la dirección `dir` (+1/-1) con
    /// envoltura, como mucho una vuelta completa. `None` = ninguna válida.
    fn find_valid(&self, store: &SceneStore, start: usize, dir: i32) -> Option<usize> {
        let n = self.playlist.entries.len();
        if n == 0 {
            return None;
        }
        let mut i = start.min(n - 1) as i32;
        for _ in 0..n {
            let idx = i.rem_euclid(n as i32) as usize;
            if store.get(&self.playlist.entries[idx].scene).is_some() {
                return Some(idx);
            }
            i += dir;
        }
        None
    }
}

/// Lanza la entrada `idx` de la playlist: carga su escena con la transición
/// propia de la entrada (o la global del panel) y reinicia el cronómetro de la
/// entrada. Mismo camino que `LoadScene` (morph + forma + aviso al panel).
#[allow(clippy::too_many_arguments)]
fn seq_launch(
    seq: &mut Sequencer,
    idx: usize,
    store: &SceneStore,
    params: &mut SimParams,
    sim: &mut Simulation,
    st: &PanelState,
    current_scene_idx: &mut usize,
    scene_morph: &mut Option<SceneMorph>,
    pending_apply: &mut Option<SimParams>,
    detached: bool,
    rng: &mut impl Rng,
) {
    seq.idx = idx;
    seq.timer = 0.0;
    let entry = match seq.playlist.entries.get(idx) {
        Some(e) => e,
        None => return,
    };
    let dur = entry.transition.unwrap_or(st.scene_transition_duration);
    if let Some(pos) = store.scenes.iter().position(|s| s.name == entry.scene) {
        *current_scene_idx = pos;
        let target = store.scenes[pos].params.clone();
        let (old_t, old_i) = (params.shape_text.clone(), params.shape_image.clone());
        *scene_morph = start_scene(params, &target, st.scene_smooth, dur);
        apply_scene_shape(sim, params, &old_t, &old_i, rng);
        if scene_morph.is_none() && detached {
            *pending_apply = Some(params.clone());
        }
    }
}

/// Cámara que mapea el rectángulo de mundo del recuadro (centro + ancho/alto)
/// al `render_target`, de modo que la grabación capture exactamente esa zona.
fn record_camera(rt: &RenderTarget, center: Vec2, w: f32, h: f32) -> Camera2D {
    let mut cam =
        Camera2D::from_display_rect(Rect::new(center.x - w / 2.0, center.y - h / 2.0, w, h));
    cam.render_target = Some(rt.clone());
    cam
}

/// Fisher–Yates parcial: deja `on` con como mucho `count` elementos.
fn partial_shuffle(on: &mut Vec<(usize, usize)>, count: usize, rng: &mut impl Rng) {
    if on.len() > count {
        for k in 0..count {
            let j = rng.gen_range(k..on.len());
            on.swap(k, j);
        }
        on.truncate(count);
    }
}

/// Mapea píxeles (px,py) a puntos de mundo, preservando aspecto, centrado al
/// 90% de la caja. `flip_y` compensa la orientación (los RT se leen de abajo
/// a arriba).
fn map_points(on: &[(usize, usize)], w: usize, h: usize, world: Vec2, flip_y: bool) -> Vec<Vec2> {
    let iw = w as f32;
    let ih = h as f32;
    let scale = (world.x * 0.9 / iw).min(world.y * 0.9 / ih);
    let center = world * 0.5;
    on.iter()
        .map(|&(px, py)| {
            let sx = (px as f32 + 0.5) / iw;
            let mut sy = (py as f32 + 0.5) / ih;
            if flip_y {
                sy = 1.0 - sy;
            }
            Vec2::new(
                center.x + (sx - 0.5) * iw * scale,
                center.y + (sy - 0.5) * ih * scale,
            )
        })
        .collect()
}

/// Convierte una imagen RGBA en una nube de puntos meta (mundo) para que las
/// partículas la formen. Marca "encendido" por alfa (siluetas/emoji con
/// transparencia) o, si la imagen es opaca, por luminancia. Submuestrea a
/// `count` puntos y ajusta la caja al lienzo con margen, preservando el aspecto.
/// `flip_y` compensa la orientación (los RT se leen de abajo a arriba).
fn image_to_points(
    img: &Image,
    flip_y: bool,
    world: Vec2,
    count: usize,
    rng: &mut impl Rng,
) -> Vec<Vec2> {
    let w = img.width as usize;
    let h = img.height as usize;
    if w == 0 || h == 0 {
        return Vec::new();
    }
    let bytes = &img.bytes;
    let total = w * h;

    // ¿La imagen usa transparencia de forma significativa? (muestreo rápido)
    let mut transparent = 0usize;
    let mut sampled = 0usize;
    let mut i = 0;
    while i < total {
        if bytes[i * 4 + 3] < 250 {
            transparent += 1;
        }
        sampled += 1;
        i += 7;
    }
    let use_alpha = transparent * 20 > sampled; // >5% de píxeles con alfa parcial

    let mut on: Vec<(usize, usize)> = Vec::new();
    for py in 0..h {
        for px in 0..w {
            let idx = (py * w + px) * 4;
            let hit = if use_alpha {
                bytes[idx + 3] > 128
            } else {
                let lum = 0.299 * bytes[idx] as f32
                    + 0.587 * bytes[idx + 1] as f32
                    + 0.114 * bytes[idx + 2] as f32;
                lum > 128.0
            };
            if hit {
                on.push((px, py));
            }
        }
    }
    if on.is_empty() {
        return Vec::new();
    }

    // Submuestreo aleatorio (Fisher–Yates parcial) hasta `count`.
    partial_shuffle(&mut on, count.max(1), rng);
    map_points(&on, w, h, world, flip_y)
}

/// Carga una imagen de disco como (textura, píxeles RGBA, ancho, alto) para el
/// efecto foto: la textura para superponerla (fase B) y los píxeles para
/// colorear las partículas (fase A). `None` si no se pudo abrir/decodificar.
fn load_photo(path: &str) -> Option<(Texture2D, Vec<u8>, usize, usize)> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("No pude abrir la imagen '{path}': {e}");
            return None;
        }
    };
    match Image::from_file_with_format(&bytes, None) {
        Ok(img) => {
            let tex = Texture2D::from_image(&img);
            tex.set_filter(FilterMode::Linear);
            Some((tex, img.bytes.clone(), img.width as usize, img.height as usize))
        }
        Err(e) => {
            eprintln!("No pude decodificar la imagen '{path}': {e}");
            None
        }
    }
}

/// Decodifica el PRIMER fotograma de un vídeo como (textura, bytes RGBA, ancho,
/// alto) para arrancar el mosaico/overlay. El streaming de la reproducción se
/// abre luego de forma diferida (`advance_video`), cuando la imagen ya se ha
/// formado. `None` si `ffmpeg`/`ffprobe` fallan.
fn load_video(path: &str) -> Option<(Texture2D, Vec<u8>, usize, usize)> {
    let (first, w, h) = VideoSource::decode_first_frame(path, 720)?;
    let img = Image { bytes: first.clone(), width: w as u16, height: h as u16 };
    let tex = Texture2D::from_image(&img);
    tex.set_filter(FilterMode::Linear);
    Some((tex, first, w as usize, h as usize))
}

/// Rasteriza `text` a un `render_target` y devuelve sus puntos meta (mundo).
fn text_to_points(text: &str, world: Vec2, count: usize, rng: &mut impl Rng) -> Vec<Vec2> {
    let font_size: u16 = 180;
    let dims = measure_text(text, None, font_size, 1.0);
    let pad = 24.0;
    let w = (dims.width + pad * 2.0).ceil().max(8.0) as u32;
    let h = (font_size as f32 + pad * 2.0).ceil() as u32;
    let rt = render_target(w, h);
    rt.texture.set_filter(FilterMode::Linear);
    let mut cam = Camera2D::from_display_rect(Rect::new(0.0, 0.0, w as f32, h as f32));
    cam.render_target = Some(rt.clone());
    set_camera(&cam);
    clear_background(Color::new(0.0, 0.0, 0.0, 0.0));
    draw_text(text, pad, pad + dims.offset_y, font_size as f32, WHITE);
    set_default_camera();
    let img = rt.texture.get_texture_data();
    image_to_points(&img, false, world, count, rng)
}

/// Lee una imagen de disco y devuelve sus puntos meta (o `None` si falla).
fn image_points_from_path(
    path: &str,
    world: Vec2,
    count: usize,
    rng: &mut impl Rng,
) -> Option<Vec<Vec2>> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("No pude abrir la imagen '{path}': {e}");
            return None;
        }
    };
    match Image::from_file_with_format(&bytes, None) {
        Ok(img) => Some(image_to_points(&img, true, world, count, rng)),
        Err(e) => {
            eprintln!("No pude decodificar la imagen '{path}': {e}");
            None
        }
    }
}


/// Puntos meta (mundo) de una rejilla `~count` sobre la caja de la foto, SOLO
/// en las celdas con píxel opaco: en un PNG sin fondo, las partículas se
/// reclutan únicamente donde hay imagen. `rgba`/`w`/`h` es la imagen.
fn mosaic_points(rgba: &[u8], w: usize, h: usize, c: Vec2, e: Vec2, count: usize) -> Vec<Vec2> {
    let count = count.max(1);
    if w == 0 || h == 0 {
        return Vec::new();
    }
    let aspect = (e.x / e.y.max(1e-3)).max(1e-3);
    let cols = ((count as f32 * aspect).sqrt().round().max(1.0)) as usize;
    let rows = ((count as f32 / aspect).sqrt().round().max(1.0)) as usize;
    let (x0, y0) = (c.x - e.x * 0.5, c.y - e.y * 0.5);
    let mut pts = Vec::with_capacity(cols * rows);
    for gy in 0..rows {
        // `py` invertida (Y de la cámara del CPU al revés), para que la máscara
        // coincida con el color (`Photo::color_at`) y la textura superpuesta.
        let py = ((((rows - 1 - gy) as f32 + 0.5) / rows as f32 * h as f32) as usize).min(h - 1);
        let wy = y0 + (gy as f32 + 0.5) / rows as f32 * e.y;
        for gx in 0..cols {
            let px = (((gx as f32 + 0.5) / cols as f32 * w as f32) as usize).min(w - 1);
            if rgba[(py * w + px) * 4 + 3] > 128 {
                let wx = x0 + (gx as f32 + 0.5) / cols as f32 * e.x;
                pts.push(Vec2::new(wx, wy));
            }
        }
    }
    pts
}

/// Construye la forma a partir del descriptor de `params` (mensaje o ruta de
/// imagen). Vacío = suelta la forma. Incondicional (siempre reconstruye).
/// En modo "recrear colores de la foto" es un efecto en dos fases: las
/// partículas se acomodan a una rejilla que cubre la imagen y toman su color
/// (mosaico), y luego la foto real se funde encima; en texto/silueta migran a
/// la silueta.
fn build_shape(sim: &mut Simulation, params: &SimParams, rng: &mut impl Rng) {
    // Modo foto: acomodar en rejilla + colorear (fase A) y preparar la foto.
    // Un vídeo se trata igual que una foto pero sus fotogramas se refrescan en
    // el tiempo (mismo efecto de entrada/salida; el mosaico usa el primer
    // fotograma para reclutar).
    if params.shape_photo_color && !params.shape_image.is_empty() {
        let is_video = is_video_path(&params.shape_image);
        let loaded = if is_video {
            load_video(&params.shape_image)
        } else {
            load_photo(&params.shape_image)
        };
        match loaded {
            Some((tex, bytes, w, h)) => {
                let mask = bytes.clone();
                sim.set_photo(tex, bytes, w, h);
                if is_video {
                    // La reproducción arranca diferida (al formarse la imagen).
                    sim.set_video_path(params.shape_image.clone());
                }
                if let Some(photo) = sim.photo.as_ref() {
                    let (c, e) = (photo.center, photo.extent);
                    // Reclutar SOLO en la zona opaca (PNG sin fondo → nada en lo
                    // transparente); el resto se desvanece (ver render). Un
                    // vídeo suele ser opaco entero, así que recluta toda la caja.
                    let recruit = (sim.particles.len() * 9 / 10).max(1);
                    let pts = mosaic_points(&mask, w, h, c, e, recruit);
                    sim.set_shape(pts);
                }
            }
            None => sim.clear_photo(),
        }
        return;
    }
    // Texto/silueta: solo una parte de las partículas forma la figura y el
    // resto queda de ambiente. Una forma nueva reemplaza a la foto (drop); si
    // no hay forma, la foto se disuelve suave y las partículas reclutadas
    // vuelven al enjambre (clear_photo + clear_shape).
    let count = (sim.particles.len() * 7 / 10).max(1);
    if !params.shape_text.trim().is_empty() {
        sim.drop_photo();
        let pts = text_to_points(&params.shape_text, sim.world, count, rng);
        sim.set_shape(pts);
    } else if !params.shape_image.is_empty() {
        sim.drop_photo();
        match image_points_from_path(&params.shape_image, sim.world, count, rng) {
            Some(pts) => sim.set_shape(pts),
            None => sim.clear_shape(),
        }
    } else {
        // Sin nueva forma: con foto, salida en reverso; sin foto, disolución.
        if sim.photo.is_some() {
            sim.clear_photo();
        } else {
            sim.clear_shape();
        }
        return;
    }
    if params.shape_tint {
        sim.tint_shape(hue_for_index(params.shape_color));
    }
}

/// Al cambiar de escena: reconstruye la forma SOLO si el descriptor cambió. Si la
/// nueva escena trae el mismo texto/imagen, se mantiene la forma actual (sin
/// re-scramblear); si trae otro, se cambia; si no trae ninguno, se suelta.
fn apply_scene_shape(
    sim: &mut Simulation,
    params: &SimParams,
    old_text: &str,
    old_image: &str,
    rng: &mut impl Rng,
) {
    if params.shape_text != old_text || params.shape_image != old_image {
        build_shape(sim, params, rng);
    }
}

struct Recorder {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    rt: RenderTarget,
    frames: u32,
    path: String,
}

impl Recorder {
    /// Arranca `ffmpeg` y el destino de render a la resolución de salida `w×h`,
    /// guardando en `dir` (o el directorio actual si está vacío). Si `music` no
    /// está vacío, se mezcla esa pista de audio en el vídeo (recortada a la
    /// duración con `-shortest`). Falla si `ffmpeg` no está.
    ///
    /// Para nitidez, el render se hace al doble de resolución (supersampling) y
    /// `ffmpeg` la baja a `w×h` con Lanczos.
    fn start(w: u32, h: u32, dir: &str, music: &str) -> std::io::Result<Recorder> {
        // Supersampling: el RT se dibuja a 2× y se reduce en la codificación.
        let (sw, sh) = (w * SSAA, h * SSAA);
        let rt = render_target(sw, sh);
        rt.texture.set_filter(FilterMode::Linear);
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let name = format!("enjambre_{ts}.mp4");
        let path = if dir.is_empty() {
            name
        } else {
            format!("{}/{}", dir.trim_end_matches('/'), name)
        };

        // Construimos los argumentos dinámicamente (la música añade un 2º input).
        let mut args: Vec<String> = vec![
            "-y".into(),
            // Input 0: vídeo crudo por la tubería, a la resolución supersampleada.
            "-f".into(), "rawvideo".into(),
            "-pix_fmt".into(), "rgba".into(),
            "-s".into(), format!("{sw}x{sh}"),
            "-r".into(), REC_FPS.to_string(),
            "-i".into(), "-".into(),
        ];
        let has_music = !music.is_empty();
        if has_music {
            // Input 1: la pista de música.
            args.extend(["-i".into(), music.to_string()]);
        }
        // Vídeo: reducir a la resolución de salida con Lanczos (antialias).
        args.extend([
            "-vf".into(), format!("scale={w}:{h}:flags=lanczos"),
            "-c:v".into(), "libx264".into(),
            "-preset".into(), "medium".into(),
            "-crf".into(), "18".into(),
            "-pix_fmt".into(), "yuv420p".into(),
        ]);
        if has_music {
            args.extend([
                "-map".into(), "0:v:0".into(),
                "-map".into(), "1:a:0".into(),
                "-c:a".into(), "aac".into(),
                "-b:a".into(), "192k".into(),
                "-shortest".into(),
            ]);
        }
        args.extend(["-movflags".into(), "+faststart".into(), path.clone()]);

        let mut child = std::process::Command::new("ffmpeg")
            .args(&args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        let stdin = child.stdin.take().expect("stdin de ffmpeg");
        let music_note = if has_music { " + música" } else { "" };
        eprintln!("● Grabando en {path} ({w}×{h} @{REC_FPS}fps{music_note}, pulsa R para parar)");
        Ok(Recorder {
            child,
            stdin,
            rt,
            frames: 0,
            path,
        })
    }

    /// Lee los píxeles del `render_target` y los vuelca a `ffmpeg`.
    /// `get_texture_data` ya devuelve las filas de arriba a abajo con la misma
    /// orientación que se ve en pantalla, así que las escribimos tal cual (una
    /// inversión extra dejaba el vídeo boca abajo, visible al formar texto).
    fn capture(&mut self) -> std::io::Result<()> {
        use std::io::Write;
        let img = self.rt.texture.get_texture_data();
        self.stdin.write_all(&img.bytes)?;
        self.frames += 1;
        Ok(())
    }

    /// Cierra la tubería para que `ffmpeg` finalice el `.mp4` y espera a que
    /// termine de escribir.
    /// Cierra la grabación y devuelve la ruta del `.mp4` guardado (para un
    /// posible post-muxeo del audio del vídeo del efecto foto).
    fn finish(self) -> String {
        drop(self.stdin); // EOF -> ffmpeg cierra el fichero limpiamente
        let path = self.path.clone();
        let frames = self.frames;
        let mut child = self.child;
        let _ = child.wait();
        eprintln!(
            "■ Vídeo guardado: {path} ({frames} frames · {:.1}s a {REC_FPS} fps)",
            frames as f32 / REC_FPS as f32
        );
        path
    }
}

/// Cierra la grabación y, si durante ella se reprodujo un vídeo del efecto
/// foto, muxea su audio en el `.mp4` al offset en que apareció.
fn finish_recording(r: Recorder, rec_video: &mut Option<(String, u32)>, has_music: bool) {
    let path = r.finish();
    if let Some((src, start_frame)) = rec_video.take() {
        let offset = start_frame as f32 / REC_FPS as f32;
        shared::video::overlay_audio(&path, &src, offset, has_music);
    }
}

// ----------------------------------------------------------------------------
// Servidor IPC: acepta la conexión del proceso `panel` en un hilo aparte y
// expone el último estado recibido (inbox) y el stream de escritura para la
// telemetría. La simulación nunca se bloquea esperando al panel.
// ----------------------------------------------------------------------------

struct Inbox {
    state: Option<ControlState>,
    events: Vec<PanelEvent>,
    connected: bool,
}

struct Ipc {
    inbox: Arc<Mutex<Inbox>>,
    writer: Arc<Mutex<Option<UnixStream>>>,
}

impl Ipc {
    fn start() -> Option<Ipc> {
        let path = socket_path();
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).ok()?;
        let inbox = Arc::new(Mutex::new(Inbox {
            state: None,
            events: Vec::new(),
            connected: false,
        }));
        let writer: Arc<Mutex<Option<UnixStream>>> = Arc::new(Mutex::new(None));
        let inbox_t = inbox.clone();
        let writer_t = writer.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let stream = match conn {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                let wclone = match stream.try_clone() {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                *writer_t.lock().unwrap() = Some(wclone);
                inbox_t.lock().unwrap().connected = true;

                let mut reader = stream;
                loop {
                    match read_frame(&mut reader) {
                        // Frame recibido: si no lo sabemos decodificar (evento de
                        // otra versión), lo ignoramos y seguimos.
                        Ok(Some(body)) => match decode::<ControlMsg>(&body) {
                            Some(ControlMsg::State(s)) => inbox_t.lock().unwrap().state = Some(s),
                            Some(ControlMsg::Event(e)) => inbox_t.lock().unwrap().events.push(e),
                            None => {}
                        },
                        Ok(None) | Err(_) => break,
                    }
                }

                inbox_t.lock().unwrap().connected = false;
                *writer_t.lock().unwrap() = None;
            }
        });
        Some(Ipc { inbox, writer })
    }

    /// Cierra la conexión con el panel actual (si la hay). Al apagar el stream,
    /// el lector del panel recibe EOF y la ventana se cierra sola; el hilo
    /// servidor vuelve a `accept()` esperando un panel nuevo. Se usa al volver
    /// a acoplar para no dejar paneles huérfanos (que provocan "dos paneles" y
    /// que un panel nuevo se quede sin atender).
    fn disconnect(&self) {
        if let Some(w) = self.writer.lock().unwrap().take() {
            let _ = w.shutdown(std::net::Shutdown::Both);
        }
        self.inbox.lock().unwrap().connected = false;
    }
}

/// Localiza el binario `panel`. Primero el hermano del ejecutable actual
/// (`target/<perfil>/panel`); si no, prueba `target/debug` y `target/release`
/// subiendo desde el ejecutable, por si solo se compiló en otro perfil.
fn find_panel_binary() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let sibling = exe.with_file_name("panel");
    if sibling.exists() {
        return Some(sibling);
    }
    // exe = .../target/<perfil>/sim  ->  .../target
    if let Some(target_dir) = exe.parent().and_then(|p| p.parent()) {
        for profile in ["release", "debug"] {
            let cand = target_dir.join(profile).join("panel");
            if cand.exists() {
                return Some(cand);
            }
        }
    }
    None
}

/// Lanza el proceso `panel`. Devuelve `true` si arrancó.
fn spawn_panel() -> bool {
    match find_panel_binary() {
        Some(path) => match std::process::Command::new(&path).spawn() {
            Ok(_) => true,
            Err(e) => {
                eprintln!("No se pudo lanzar el panel ({path:?}): {e}");
                false
            }
        },
        None => {
            eprintln!(
                "No encuentro el binario `panel`. Compílalo con `cargo build -p panel` \
                 (o `cargo build --release -p panel` si usas --release)."
            );
            false
        }
    }
}

/// Empaqueta el estado actual (params + UI) para enviarlo al panel.
fn control_state(params: &SimParams, st: &PanelState) -> ControlState {
    ControlState {
        params: params.clone(),
        paused: st.paused,
        canvas_size: st.canvas_size,
        zoom_level: st.zoom_level,
        tool: st.tool,
        brush: st.brush,
        brush_size: st.brush_size,
        active_color: st.active_color,
        fill_count: st.fill_count,
        video_dir: st.video_dir.clone(),
        music_path: st.music_path.clone(),
        scene_smooth: st.scene_smooth,
        scene_transition_duration: st.scene_transition_duration,
        scene_autoplay: st.scene_autoplay,
        scene_autoplay_interval: st.scene_autoplay_interval,
        music_sync: st.music_sync.clone(),
    }
}

/// Ejecuta un evento del panel que no cambia de modo (todos menos
/// `Detach`/`Reattach`, que el bucle principal maneja directamente).
fn apply_local_event(
    ev: PanelEvent,
    sim: &mut Simulation,
    params: &mut SimParams,
    st: &mut PanelState,
    pan_target: &mut Vec2,
    canvas: Vec2,
    rng: &mut impl Rng,
    step_once: &mut bool,
) {
    let aspect = canvas.x / canvas.y;
    let world = Vec2::new(st.canvas_size * aspect, st.canvas_size);
    match ev {
        PanelEvent::Step => *step_once = true,
        PanelEvent::Clear => sim.clear(),
        PanelEvent::Fill(n) => sim.spawn_random(n, rng),
        PanelEvent::StartTransition(snap) => params.start_transition(snap),
        PanelEvent::MatrixBlend(snap) => params.start_matrix_blend(snap),
        PanelEvent::SetSpeed(v) => params.set_speed(v),
        PanelEvent::FitCanvas => {
            st.zoom_level = (canvas.x / world.x)
                .min(canvas.y / world.y)
                .clamp(0.02, 30.0);
            *pan_target = world * 0.5;
        }
        PanelEvent::CanvasEqualsScreen => {
            // El mundo pasa a medir exactamente el lienzo (1 unidad = 1 píxel),
            // así llena la región visible sea cual sea el tamaño del WM (Hyprland).
            st.canvas_size = canvas.y;
            st.zoom_level = 1.0;
            *pan_target = canvas * 0.5;
        }
        // Los maneja el bucle principal (necesitan cambiar de modo, el grabador
        // o el estado del recuadro de encuadre / carpeta de guardado).
        PanelEvent::Detach
        | PanelEvent::Reattach
        | PanelEvent::ToggleRecord
        | PanelEvent::ToggleFrame
        | PanelEvent::SetFramePreset(_)
        | PanelEvent::CenterFrame
        | PanelEvent::PickVideoDir
        | PanelEvent::PickMusic
        | PanelEvent::SaveScene(_)
        | PanelEvent::LoadScene(_)
        | PanelEvent::SetDefaultScene(_)
        | PanelEvent::DeleteScene(_)
        | PanelEvent::NextScene
        | PanelEvent::PrevScene
        | PanelEvent::ExportScenes
        | PanelEvent::ImportScenes
        | PanelEvent::FormText(_)
        | PanelEvent::FormImagePick
        | PanelEvent::FormImagePath(_)
        | PanelEvent::ReleaseShape
        | PanelEvent::SaveShape
        | PanelEvent::ApplyShape(_)
        | PanelEvent::DeleteShape(_)
        | PanelEvent::SeqSetPlaylist(_)
        | PanelEvent::SeqPlay
        | PanelEvent::SeqPause
        | PanelEvent::SeqStop
        | PanelEvent::SeqNext
        | PanelEvent::SeqPrev
        | PanelEvent::SeqJump(_)
        | PanelEvent::MusicAnalyze
        | PanelEvent::MusicPreviewToggle
        | PanelEvent::HidePanel => {}
    }
}

#[macroquad::main(window_conf)]
async fn main() {
    let mut sim = Simulation::new(Vec2::new(screen_width(), screen_height()));
    let mut params = SimParams::default();
    let mut renderer = Renderer::new();
    let mut rng = ::rand::thread_rng();

    // Estado de la UI (tamaño de lienzo, zoom, brocha...) compartido con el panel.
    let mut st = PanelState {
        canvas_size: screen_height(),
        ..PanelState::default()
    };
    let mut step_once = false;

    // Cámara: punto del mundo centrado (el zoom vive en `st.zoom_level`).
    let mut pan_target = Vec2::new(screen_width(), screen_height()) * 0.5;
    let mut last_mouse = Vec2::from(mouse_position());

    let mut mode = AppMode::Embedded;
    // Panel acoplado: visible por defecto, se oculta/muestra con la tecla H.
    // `panel_px` recuerda su ancho real (en píxeles) para reservarle el lienzo.
    let mut panel_visible = true;
    let mut panel_px = 310.0f32;
    // El tema egui (visuales + fuente de iconos) se instala una vez.
    let mut theme_applied = false;
    let mut rec: Option<Recorder> = None;
    // Marcador para muxear el audio del vídeo del efecto foto en la grabación:
    // (ruta del vídeo, frame de grabación en que arrancó su reproducción) y si
    // la grabación lleva música (para mezclar en vez de sustituir).
    let mut rec_video: Option<(String, u32)> = None;
    let mut rec_music = false;
    // Recuadro de encuadre de grabación (estado local, lo mueve el ratón).
    let mut show_frame = false;
    let mut frame_preset = 0usize;
    let mut frame_center = pan_target;
    let mut frame_height = screen_height() * 0.8;
    let mut frame_drag: Option<FrameDrag> = None;
    let mut video_dir = String::new();
    let mut auto_rng_timer = 0.0f32;
    // Buffer de acumulación para las estelas (se recrea si cambia la ventana).
    let mut trails_rt: Option<RenderTarget> = None;
    // Captura de audio (mantener vivo el stream) + nivel suavizado 0..1. Se
    // reinicia si el usuario cambia la fuente (Micrófono/Sistema) en el panel.
    let mut audio_source_active = params.audio_source;
    let mut audio_in = shared::audio::start(audio_source_active);
    if audio_in.is_none() {
        eprintln!("Audio: sin dispositivo de entrada; 'Reactivo al audio' no tendrá efecto.");
    }
    let mut audio_level = 0.0f32;
    // Sincronía con la música: análisis (resultado + canal del hilo de fondo),
    // preescucha, y estado de beats (cursor sobre los onsets, contador para el
    // divisor y pulso con decaimiento).
    let mut music: Option<shared::music::MusicAnalysis> = None;
    let mut music_rx: Option<
        std::sync::mpsc::Receiver<std::io::Result<shared::music::MusicAnalysis>>,
    > = None;
    // Ruta a la que corresponden `music`/`music_rx` (si el usuario cambia de
    // pista, el análisis viejo deja de valer).
    let mut music_path_analyzed = String::new();
    let mut music_rx_path = String::new();
    let mut preview: Option<shared::music::Preview> = None;
    let mut beat_cursor = 0usize;
    let mut beat_count = 0u32;
    let mut beat_pulse = 0.0f32;
    // Escenas guardadas (el `sim` es el dueño; ver sección "Escenas" del panel).
    // En el primer arranque (sin fichero) sembramos un set de ejemplos.
    let mut store = if scenes_path().exists() {
        SceneStore::load()
    } else {
        let s = example_store();
        let _ = s.save();
        eprintln!("Sembradas escenas de ejemplo en {:?}", scenes_path());
        s
    };
    let mut scene_morph: Option<SceneMorph> = None;
    let mut pending_apply: Option<SimParams> = None;
    // Pausa cambiada con el teclado en la ventana del lienzo, pendiente de
    // empujar al panel separado (ver TelemetryMsg::SetPaused).
    let mut pending_paused: Option<bool> = None;
    let mut scenes_dirty = false;
    // Biblioteca de formas/letras guardadas (persistida en shapes.json).
    let mut shape_store = ShapeStore::load();
    let mut shapes_dirty = false;
    let mut current_scene_idx = 0usize;
    let mut scene_autoplay_timer = 0.0f32;
    // Secuenciador de escenas (show). La playlist persiste en playlist.json.
    let mut sequencer = Sequencer {
        playlist: Playlist::load(),
        state: SeqPlayback::Stopped,
        idx: 0,
        timer: 0.0,
    };
    let mut seq_dirty = false;
    if let Some(def) = store.default.clone() {
        if let Some(scene) = store.get(&def) {
            params = scene.params.settled();
            current_scene_idx = store.scenes.iter().position(|s| s.name == def).unwrap_or(0);
            eprintln!("Escena predeterminada cargada: {def}");
        }
    }
    eprintln!("Teclas: R grabar · G encuadre. Elige tamaño y carpeta en el panel.");
    let mut ipc: Option<Ipc> = None;
    let mut panel_was_connected = false;
    let mut init_sent = false;
    // Para distinguir un movimiento del slider de zoom del panel (que debemos
    // adoptar) del eco de nuestro propio zoom de rueda (que debemos ignorar).
    let mut prev_incoming_zoom = st.zoom_level;

    sim.spawn_random(st.fill_count as usize, &mut rng);
    // Si la escena predeterminada traía un mensaje/forma, reconstruirlo ya que
    // las partículas existen.
    build_shape(&mut sim, &params, &mut rng);

    loop {
        // Región de la ventana donde se dibuja el mundo: con el panel acoplado y
        // visible, se reserva su ancho a la derecha y el lienzo ocupa el resto.
        let panel_w = if mode == AppMode::Embedded && panel_visible {
            panel_px
        } else {
            0.0
        };
        let canvas = Rect::new(0.0, 0.0, (screen_width() - panel_w).max(1.0), screen_height());
        // El lienzo mantiene el aspecto de su región; su alto lo fija `st`.
        let aspect = canvas.w / canvas.h;
        let world = Vec2::new(st.canvas_size * aspect, st.canvas_size);
        sim.world = world;
        // El recentrado de zonas activas tira hacia el centro de la vista.
        sim.focus = pan_target;

        // La velocidad transita de forma suave hacia su objetivo (aunque esté
        // en pausa, para que al reanudar ya esté en el valor pedido).
        params.advance_speed(get_frame_time());

        // Tiempo de "show" (secuenciador, autoplay y morphs de escena): el del
        // frame en vivo, pero 1/60 exacto mientras se graba, porque cada frame
        // de simulación es 1/60 s de vídeo. Así las duraciones del show y de
        // las transiciones salen exactas en el .mp4 aunque el volcado vaya
        // más lento que el tiempo real.
        let show_dt = if rec.is_some() {
            1.0 / REC_FPS as f32
        } else {
            get_frame_time()
        };

        // Transición de escena en curso: interpola los números; el cruce de
        // interacción lo lleva el blend de `advance_transition` (más abajo).
        let mut morph_done = false;
        if let Some(m) = scene_morph.as_mut() {
            m.blend = (m.blend + show_dt / m.dur).min(1.0);
            let t = m.blend * m.blend * (3.0 - 2.0 * m.blend);
            params.lerp_scene_numeric(&m.from, &m.target, t);
            if m.blend >= 1.0 {
                let target = (*m.target).clone();
                params.smooth = target.smooth;
                params.transition_duration = target.transition_duration;
                params.color_transition_duration = target.color_transition_duration;
                params.speed_transition_duration = target.speed_transition_duration;
                morph_done = true;
            }
        }
        if morph_done {
            scene_morph = None;
            if mode == AppMode::Detached {
                pending_apply = Some(params.clone());
            }
        }

        // Secuenciador: mientras reproduce, conduce él los cambios de escena
        // (el auto-avance simple queda en espera). La duración de cada entrada
        // incluye su transición, así el show dura la suma de las entradas.
        if sequencer.state == SeqPlayback::Playing && !sequencer.playlist.entries.is_empty() {
            sequencer.timer += show_dt;
            let n = sequencer.playlist.entries.len();
            let cur_dur = sequencer.playlist.entries[sequencer.idx.min(n - 1)]
                .duration
                .max(0.1);
            if sequencer.timer >= cur_dur {
                let next = sequencer.idx + 1;
                if next >= n && !sequencer.playlist.loop_at_end {
                    // Show terminado: parar y, si la grabación la arrancó el
                    // show (start_on_record), cerrarla también.
                    sequencer.state = SeqPlayback::Stopped;
                    sequencer.idx = 0;
                    sequencer.timer = 0.0;
                    if sequencer.playlist.start_on_record {
                        if let Some(r) = rec.take() {
                            finish_recording(r, &mut rec_video, rec_music);
                        }
                    }
                } else if let Some(v) = sequencer.find_valid(&store, next % n, 1) {
                    seq_launch(
                        &mut sequencer,
                        v,
                        &store,
                        &mut params,
                        &mut sim,
                        &st,
                        &mut current_scene_idx,
                        &mut scene_morph,
                        &mut pending_apply,
                        mode == AppMode::Detached,
                        &mut rng,
                    );
                } else {
                    // No queda ninguna entrada válida (escenas borradas).
                    sequencer.state = SeqPlayback::Stopped;
                }
            }
        }

        // Auto-avance de escenas (slideshow): cambia a la siguiente cada X s.
        if st.scene_autoplay
            && sequencer.state != SeqPlayback::Playing
            && scene_morph.is_none()
            && store.scenes.len() > 1
        {
            scene_autoplay_timer += show_dt;
            if scene_autoplay_timer >= st.scene_autoplay_interval.max(0.5) {
                scene_autoplay_timer = 0.0;
                let (old_t, old_i) = (params.shape_text.clone(), params.shape_image.clone());
                scene_morph = cycle_scene(
                    1,
                    &store,
                    &mut params,
                    &mut current_scene_idx,
                    st.scene_smooth,
                    st.scene_transition_duration,
                );
                apply_scene_shape(&mut sim, &params, &old_t, &old_i, &mut rng);
                if scene_morph.is_none() && mode == AppMode::Detached {
                    pending_apply = Some(params.clone());
                }
            }
        } else if !st.scene_autoplay {
            scene_autoplay_timer = 0.0;
        }

        // Resultado del análisis de la música, si hay uno en marcha.
        if let Some(rx) = &music_rx {
            match rx.try_recv() {
                Ok(Ok(a)) => {
                    eprintln!(
                        "Música analizada: {} beats{} · {:.0} s",
                        a.onsets.len(),
                        a.bpm.map(|b| format!(" · ~{b:.0} BPM")).unwrap_or_default(),
                        a.duration
                    );
                    music = Some(a);
                    music_path_analyzed = music_rx_path.clone();
                    music_rx = None;
                    beat_cursor = 0;
                    beat_count = 0;
                }
                Ok(Err(e)) => {
                    eprintln!("No se pudo analizar la música: {e}");
                    music_rx = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => music_rx = None,
            }
        }
        // Preescucha que terminó por sí sola (fin de la pista).
        if preview.as_mut().is_some_and(|p| p.finished()) {
            preview = None;
        }

        // Reloj musical: grabando es el frame de vídeo (exacto por
        // construcción, el audio del .mp4 empieza en 0 con el frame 0); en
        // vivo, el tiempo de la preescucha menos la latencia configurada.
        // `None` = la sincronía no actúa este frame.
        let music_t: Option<f32> = if st.music_sync.enabled
            && music.is_some()
            && music_path_analyzed == st.music_path
        {
            if let Some(r) = &rec {
                Some(r.frames as f32 / REC_FPS as f32)
            } else {
                preview
                    .as_ref()
                    .map(|p| (p.elapsed() - st.music_sync.latency_offset).max(0.0))
            }
        } else {
            None
        };

        // Reiniciar la captura si el usuario cambió de fuente en el panel
        // (Micrófono/Sistema).
        if params.audio_source != audio_source_active {
            audio_source_active = params.audio_source;
            audio_in = shared::audio::start(audio_source_active);
            if audio_in.is_none() {
                eprintln!("Audio: sin captura para esa fuente.");
            }
        }

        // Nivel de audio suavizado (ataque rápido, caída lenta) para la
        // reactividad. Se calcula cada frame, aun en pausa (afecta al brillo).
        let audio_raw = audio_in.as_ref().map(|a| a.level()).unwrap_or(0.0);
        let audio_goal = (audio_raw * 6.0).clamp(0.0, 1.0);
        let k = if audio_goal > audio_level { 0.5 } else { 0.08 };
        audio_level += (audio_goal - audio_level) * k;

        // Con sincronía activa, la envolvente analizada de la pista sustituye
        // al micrófono como nivel (mismo objetivo/intensidad de audio).
        let env_drive = music_t.is_some() && st.music_sync.envelope_drive;
        if let (Some(t), Some(m), true) = (music_t, &music, env_drive) {
            audio_level = m.envelope_at(t);
        }

        // Beats: al cruzar cada onset, dispara la acción configurada cada
        // `beat_divisor` golpes. El pulso decae en ~0.2 s (tiempo de show).
        beat_pulse *= (-show_dt / 0.2).exp();
        if let Some(t) = music_t {
            let onsets_len = music.as_ref().map_or(0, |m| m.onsets.len());
            while beat_cursor < onsets_len
                && music.as_ref().unwrap().onsets[beat_cursor] <= t
            {
                beat_cursor += 1;
                beat_count += 1;
                if beat_count % st.music_sync.beat_divisor.max(1) != 0 {
                    continue;
                }
                match st.music_sync.beat_action {
                    // "Onda de choque" es un efecto GPU-only (empuje radial en
                    // el kernel del motor GPU); no-op en la CPU.
                    BeatAction::None | BeatAction::Shockwave => {}
                    BeatAction::Pulse => beat_pulse = st.music_sync.pulse_gain,
                    BeatAction::RandomizeMatrix => {
                        let snap = params.current_snapshot();
                        params.randomize_matrix(&mut rng);
                        params.start_matrix_blend(snap);
                        if mode == AppMode::Detached {
                            pending_apply = Some(params.clone());
                        }
                    }
                    BeatAction::NextScene => {
                        if sequencer.state == SeqPlayback::Playing {
                            // El beat fuerza el paso a la siguiente entrada del
                            // show (el cronómetro se reinicia en seq_launch).
                            let n = sequencer.playlist.entries.len();
                            if n > 0 {
                                if let Some(v) =
                                    sequencer.find_valid(&store, (sequencer.idx + 1) % n, 1)
                                {
                                    seq_launch(
                                        &mut sequencer,
                                        v,
                                        &store,
                                        &mut params,
                                        &mut sim,
                                        &st,
                                        &mut current_scene_idx,
                                        &mut scene_morph,
                                        &mut pending_apply,
                                        mode == AppMode::Detached,
                                        &mut rng,
                                    );
                                }
                            }
                        } else {
                            let (old_t, old_i) =
                                (params.shape_text.clone(), params.shape_image.clone());
                            scene_morph = cycle_scene(
                                1,
                                &store,
                                &mut params,
                                &mut current_scene_idx,
                                st.scene_smooth,
                                st.scene_transition_duration,
                            );
                            apply_scene_shape(&mut sim, &params, &old_t, &old_i, &mut rng);
                            if scene_morph.is_none() && mode == AppMode::Detached {
                                pending_apply = Some(params.clone());
                            }
                        }
                    }
                }
            }
        }

        // La modulación de audio actúa con el micrófono activo, con la
        // envolvente de la pista o mientras quede pulso de beat vivo.
        let audio_mod_on = params.audio_reactive || env_drive || beat_pulse > 1e-3;
        let mut audio_gain = 1.0;
        if params.audio_reactive || env_drive {
            audio_gain += audio_level * params.audio_intensity;
        }
        audio_gain += beat_pulse;

        // Física.
        if !st.paused || step_once {
            // Auto-aleatorizado de la matriz cada X segundos (solo en modo Matriz).
            if params.auto_randomize && params.mode == InteractionMode::Matrix {
                auto_rng_timer += get_frame_time();
                if auto_rng_timer >= params.auto_randomize_interval.max(0.2) {
                    let snap = params.current_snapshot();
                    params.randomize_matrix(&mut rng);
                    params.start_matrix_blend(snap);
                    auto_rng_timer = 0.0;
                }
            }
            sim.apply_dynamics(&mut params, &mut rng, get_frame_time());
            // Modulación de audio transitoria sobre velocidad o fuerza: se aplica
            // solo para este `step` y se restaura, para no pisar el valor base ni
            // la transición de velocidad (`advance_transition`).
            let saved_ts = params.time_scale;
            let saved_force = params.force;
            let saved_fix = params.shape_strength;
            if audio_mod_on {
                match params.audio_target {
                    AudioTarget::Speed => params.time_scale *= audio_gain,
                    AudioTarget::Force => params.force *= audio_gain,
                    // Brillo/Tamaño/Resplandor se aplican en el render (más
                    // abajo); "Bandas → colores" es efecto GPU-only.
                    AudioTarget::Brightness | AudioTarget::Size | AudioTarget::Bloom => {}
                }
            }
            // Fase A: con foto activa, fijación alta para que las partículas se
            // asienten en la rejilla y cubran la imagen.
            if sim.photo.is_some() {
                params.shape_strength = params.shape_strength.max(0.9);
            }
            // Aparición/disolución fluida de la forma (fase A, posición + color).
            sim.advance_shape(get_frame_time(), params.shape_transition_duration);
            // Vídeo: avanza con el `dt` del show (1/60 grabando) — si la foto
            // es un vídeo, sube el fotograma actual; al soltar se congela en el
            // frame visible para el reverso. Al arrancar la reproducción durante
            // una grabación, anota el offset para muxear su audio al terminar.
            if sim.advance_video(show_dt) {
                if let (Some(r), Some(path)) = (rec.as_ref(), sim.video_path()) {
                    rec_video = Some((path.to_string(), r.frames));
                }
            }
            // Efecto foto (fase B + salida en reverso): la imagen se funde tras
            // acomodarse; al soltar, se va primero la imagen y luego la forma.
            sim.advance_photo_effect(get_frame_time(), params.shape_transition_duration);
            sim.step(&params);
            params.time_scale = saved_ts;
            params.force = saved_force;
            params.shape_strength = saved_fix;
            params.advance_transition(get_frame_time());
            step_once = false;
        }

        let mut want_pointer = false;
        let mut want_keyboard = false;
        let mut events: Vec<PanelEvent> = Vec::new();
        let (preset_w, preset_h) = (FRAME_PRESETS[frame_preset].1, FRAME_PRESETS[frame_preset].2);
        let frame_aspect = preset_w as f32 / preset_h as f32;
        st.recording = rec.is_some();
        st.show_frame = show_frame;
        st.frame_preset = frame_preset;
        st.frame_w = preset_w;
        st.frame_h = preset_h;
        st.video_dir = video_dir.clone();
        st.scenes = store.names();
        st.default_scene = store.default.clone().unwrap_or_default();
        st.saved_shapes = shape_store.shapes.clone();
        st.seq_playlist = sequencer.playlist.clone();
        st.seq_state = sequencer.state;
        st.seq_idx = sequencer.idx;
        st.seq_elapsed = sequencer.timer;
        st.music_analyzed = music.is_some() && music_path_analyzed == st.music_path;
        st.music_duration = music.as_ref().map_or(0.0, |m| m.duration);
        st.music_onsets = music.as_ref().map_or(0, |m| m.onsets.len());
        st.music_bpm = music.as_ref().and_then(|m| m.bpm);
        st.music_previewing = preview.is_some();

        match mode {
            AppMode::Embedded => {
                st.standalone = false;
                st.particle_count = sim.particles.len();
                st.fps = get_fps();
                // Si un panel se conecta estando acoplados (p. ej. uno lanzado
                // que arranca tarde tras un Reattach), lo cerramos: en modo
                // embebido no lo atendemos, y dejarlo abierto daría "dos paneles".
                if let Some(ipc) = &ipc {
                    if ipc.inbox.lock().unwrap().connected {
                        ipc.disconnect();
                    }
                }
                egui_macroquad::ui(|ctx| {
                    // Aplica el tema neón + fuente de iconos una sola vez.
                    if !theme_applied {
                        shared::ui_theme::apply(ctx);
                        theme_applied = true;
                    }
                    want_pointer = ctx.wants_pointer_input();
                    want_keyboard = ctx.wants_keyboard_input();
                    // Oculto: no dibujamos panel (egui queda vacío este frame, así
                    // el ratón/teclado controlan el lienzo sin robar el foco).
                    if panel_visible {
                        let resp = egui::SidePanel::right("panel")
                            .default_width(310.0)
                            .show(ctx, |ui| {
                                events = config_panel(ui, &mut params, &mut st);
                            });
                        // Ancho real del panel (en píxeles) para reservarle el lienzo
                        // el próximo frame.
                        panel_px = resp.response.rect.width() * ctx.pixels_per_point();
                    }
                });
            }
            AppMode::Detached => {
                if let Some(ipc) = &ipc {
                    let mut inbox = ipc.inbox.lock().unwrap();
                    if inbox.connected {
                        panel_was_connected = true;
                    }
                    if let Some(state) = inbox.state.take() {
                        st.canvas_size = state.canvas_size;
                        st.tool = state.tool;
                        st.brush = state.brush;
                        st.brush_size = state.brush_size;
                        st.active_color = state.active_color;
                        st.fill_count = state.fill_count;
                        st.paused = state.paused;
                        st.scene_smooth = state.scene_smooth;
                        st.scene_transition_duration = state.scene_transition_duration;
                        st.scene_autoplay = state.scene_autoplay;
                        st.scene_autoplay_interval = state.scene_autoplay_interval;
                        st.music_sync = state.music_sync.clone();
                        // La carpeta de guardado y la música las elige el panel.
                        video_dir = state.video_dir.clone();
                        st.music_path = state.music_path.clone();
                        // El zoom lo puede mover tanto el slider del panel como
                        // la rueda en esta ventana: solo adoptamos el del panel
                        // cuando cambia de verdad.
                        if (state.zoom_level - prev_incoming_zoom).abs() > 1e-6 {
                            st.zoom_level = state.zoom_level;
                        }
                        prev_incoming_zoom = state.zoom_level;

                        // Durante una transición de escena el `sim` es el dueño de
                        // los params (los está interpolando): no adoptamos los del
                        // panel para no cortar el morph.
                        if scene_morph.is_none() {
                            // Adoptamos los parámetros, pero conservamos lo que esta
                            // simulación evoluciona por su cuenta (transición y, con
                            // `gradual`, la matriz a la deriva).
                            let mut p = state.params;
                            p.blend = params.blend;
                            p.from_state = params.from_state;
                            // La transición de velocidad la conduce el sim; el panel
                            // solo fija el objetivo vía evento SetSpeed.
                            p.time_scale = params.time_scale;
                            p.speed_target = params.speed_target;
                            p.speed_from = params.speed_from;
                            p.speed_blend = params.speed_blend;
                            // La matriz la evoluciona el `sim` cuando hay deriva o
                            // auto-aleatorizado; en esos casos conservamos la suya.
                            if p.gradual || p.auto_randomize {
                                p.matrix = params.matrix;
                            }
                            // El descriptor de la forma lo posee el `sim` (se cambia
                            // por eventos Form*/Release, no por el panel).
                            p.shape_text = params.shape_text.clone();
                            p.shape_image = params.shape_image.clone();
                            params = p;
                        }
                    }
                    events = std::mem::take(&mut inbox.events);
                    let connected = inbox.connected;
                    drop(inbox);

                    // Si el panel se cerró, volvemos a acoplar.
                    if panel_was_connected && !connected {
                        mode = AppMode::Embedded;
                        panel_was_connected = false;
                    }
                }
            }
        }

        // --- Atajos de teclado (control del lienzo sin ratón) ---
        // Se ignoran si egui tiene el foco de teclado (edición de un control).
        if !want_keyboard {
            if is_key_pressed(KeyCode::Space) {
                st.paused = !st.paused;
                // Estando separado, empuja la pausa al panel (si no, su State la
                // revertiría al instante).
                pending_paused = Some(st.paused);
            }
            if is_key_pressed(KeyCode::Period) {
                st.paused = true;
                events.push(PanelEvent::Step);
            }
            if is_key_pressed(KeyCode::C) {
                events.push(PanelEvent::Clear);
            }
            if is_key_pressed(KeyCode::F) {
                events.push(PanelEvent::Fill(st.fill_count as usize));
            }
            if is_key_pressed(KeyCode::M) {
                // La matriz la posee el `sim` en modo embebido; aleatorizamos y
                // transicionamos igual que el botón del panel.
                let snap = params.current_snapshot();
                params.randomize_matrix(&mut rng);
                params.start_matrix_blend(snap);
                pending_apply = Some(params.clone());
            }
            if is_key_pressed(KeyCode::L) {
                events.push(PanelEvent::CanvasEqualsScreen);
            }
            if is_key_pressed(KeyCode::Z) {
                events.push(PanelEvent::FitCanvas);
            }
            if is_key_pressed(KeyCode::D) {
                events.push(if mode == AppMode::Detached {
                    PanelEvent::Reattach
                } else {
                    PanelEvent::Detach
                });
            }
            if is_key_pressed(KeyCode::R) {
                events.push(PanelEvent::ToggleRecord);
            }
            if is_key_pressed(KeyCode::G) {
                show_frame = !show_frame;
            }
            if is_key_pressed(KeyCode::H) {
                // Ocultar/mostrar el panel acoplado (deja el lienzo a pantalla
                // completa). No afecta al panel separado.
                panel_visible = !panel_visible;
            }
            if is_key_pressed(KeyCode::S) {
                // Soltar la forma/texto activo (disolución fluida).
                events.push(PanelEvent::ReleaseShape);
            }
            if is_key_pressed(KeyCode::Enter) && !st.shape_text.trim().is_empty() {
                // Aplicar el texto escrito en el panel (formar la letra/mensaje).
                events.push(PanelEvent::FormText(st.shape_text.clone()));
            }
            if is_key_pressed(KeyCode::A) {
                params.attract_active = !params.attract_active;
                pending_apply = Some(params.clone());
            }
            if is_key_pressed(KeyCode::N) {
                events.push(PanelEvent::NextScene);
            }
            if is_key_pressed(KeyCode::P) {
                events.push(PanelEvent::PrevScene);
            }
            if is_key_pressed(KeyCode::B) {
                // Alternar el comportamiento en los bordes del lienzo.
                params.boundary = match params.boundary {
                    Boundary::Wrap => Boundary::Bounce,
                    Boundary::Bounce => Boundary::Wrap,
                };
                pending_apply = Some(params.clone());
            }
            if is_key_pressed(KeyCode::X) {
                // Aleatorizar la matriz sola cada cierto tiempo (on/off).
                params.auto_randomize = !params.auto_randomize;
                pending_apply = Some(params.clone());
            }
            if is_key_pressed(KeyCode::V) {
                // Deriva lenta y gradual del color y de la atracción (on/off).
                params.gradual = !params.gradual;
                pending_apply = Some(params.clone());
            }
            if is_key_pressed(KeyCode::E) {
                // Estelas de movimiento (on/off).
                params.trails = !params.trails;
                pending_apply = Some(params.clone());
            }
            if is_key_pressed(KeyCode::Y) {
                // Resplandor cinematográfico (on/off).
                params.bloom = !params.bloom;
                pending_apply = Some(params.clone());
            }
            if is_key_pressed(KeyCode::U) {
                // Anti-aglomeración: disolver bolas hiperdensas (on/off), para
                // comparar con/sin al vuelo.
                params.anti_clump = !params.anti_clump;
                pending_apply = Some(params.clone());
            }
            // Velocidad: teclas 1..9 = 10..90 %, tecla 0 = 100 %.
            for (key, pct) in [
                (KeyCode::Key1, 10),
                (KeyCode::Key2, 20),
                (KeyCode::Key3, 30),
                (KeyCode::Key4, 40),
                (KeyCode::Key5, 50),
                (KeyCode::Key6, 60),
                (KeyCode::Key7, 70),
                (KeyCode::Key8, 80),
                (KeyCode::Key9, 90),
                (KeyCode::Key0, 100),
            ] {
                if is_key_pressed(key) {
                    params.set_speed(pct as f32 / 100.0);
                }
            }
        }

        // Eventos del panel (mismo trato venga de la UI embebida o por IPC).
        for ev in events {
            match ev {
                PanelEvent::Detach => {
                    if mode == AppMode::Embedded {
                        if ipc.is_none() {
                            ipc = Ipc::start();
                        }
                        // Solo nos separamos si el panel arrancó de verdad; si no,
                        // seguimos con el panel embebido (y avisamos por stderr).
                        if ipc.is_some() && spawn_panel() {
                            mode = AppMode::Detached;
                            panel_was_connected = false;
                            init_sent = false;
                            prev_incoming_zoom = st.zoom_level;
                        }
                    }
                }
                PanelEvent::Reattach => {
                    // Cerramos el panel separado para no dejarlo huérfano (si el
                    // Reattach vino de la tecla `D` en esta ventana, el panel no
                    // se entera por sí solo de que debe cerrarse).
                    if let Some(ipc) = &ipc {
                        ipc.disconnect();
                    }
                    mode = AppMode::Embedded;
                    panel_was_connected = false;
                    init_sent = false;
                }
                PanelEvent::ToggleRecord => match rec.take() {
                    Some(r) => finish_recording(r, &mut rec_video, rec_music),
                    None => match Recorder::start(preset_w, preset_h, &video_dir, &st.music_path) {
                        Ok(r) => {
                            rec = Some(r);
                            rec_music = !st.music_path.is_empty();
                            rec_video = None;
                            // La música del .mp4 empieza en 0 con el frame 0:
                            // rebobinar el cursor de beats.
                            beat_cursor = 0;
                            beat_count = 0;
                            // Show ligado a la grabación: arranca desde el
                            // principio junto con ella.
                            if sequencer.playlist.start_on_record {
                                if let Some(v) = sequencer.find_valid(&store, 0, 1) {
                                    sequencer.state = SeqPlayback::Playing;
                                    seq_launch(
                                        &mut sequencer,
                                        v,
                                        &store,
                                        &mut params,
                                        &mut sim,
                                        &st,
                                        &mut current_scene_idx,
                                        &mut scene_morph,
                                        &mut pending_apply,
                                        mode == AppMode::Detached,
                                        &mut rng,
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("No se pudo iniciar la grabación (¿está ffmpeg?): {e}")
                        }
                    },
                },
                PanelEvent::ToggleFrame => show_frame = !show_frame,
                PanelEvent::SetFramePreset(i) => {
                    // No cambiar la resolución de salida en mitad de una grabación.
                    if rec.is_none() && i < FRAME_PRESETS.len() {
                        frame_preset = i;
                    }
                }
                PanelEvent::CenterFrame => {
                    frame_center = pan_target;
                    frame_height = canvas.h * 0.8 / st.zoom_level;
                    show_frame = true;
                }
                PanelEvent::PickVideoDir => {
                    if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                        video_dir = dir.to_string_lossy().into_owned();
                    }
                }
                PanelEvent::PickMusic => {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Audio", &["mp3", "wav", "flac", "m4a", "ogg", "aac"])
                        .pick_file()
                    {
                        st.music_path = path.to_string_lossy().into_owned();
                    }
                }
                PanelEvent::SaveScene(name) => {
                    store.upsert(&name, params.settled());
                    if let Err(e) = store.save() {
                        eprintln!("No se pudo guardar la escena '{name}': {e}");
                    }
                    scenes_dirty = true;
                }
                PanelEvent::LoadScene(name) => {
                    if let Some(idx) = store.scenes.iter().position(|s| s.name == name) {
                        current_scene_idx = idx;
                        let target = store.scenes[idx].params.clone();
                        let (old_t, old_i) =
                            (params.shape_text.clone(), params.shape_image.clone());
                        scene_morph = start_scene(
                            &mut params,
                            &target,
                            st.scene_smooth,
                            st.scene_transition_duration,
                        );
                        // Cambiar/soltar la forma solo si la escena trae otra distinta.
                        apply_scene_shape(&mut sim, &params, &old_t, &old_i, &mut rng);
                        // Carga instantánea: avisamos ya al panel; la suave, al
                        // terminar el morph (ver `morph_done`).
                        if scene_morph.is_none() && mode == AppMode::Detached {
                            pending_apply = Some(params.clone());
                        }
                    }
                }
                PanelEvent::NextScene => {
                    let (old_t, old_i) = (params.shape_text.clone(), params.shape_image.clone());
                    scene_morph = cycle_scene(
                        1,
                        &store,
                        &mut params,
                        &mut current_scene_idx,
                        st.scene_smooth,
                        st.scene_transition_duration,
                    );
                    apply_scene_shape(&mut sim, &params, &old_t, &old_i, &mut rng);
                    if scene_morph.is_none() && mode == AppMode::Detached {
                        pending_apply = Some(params.clone());
                    }
                }
                PanelEvent::PrevScene => {
                    let (old_t, old_i) = (params.shape_text.clone(), params.shape_image.clone());
                    scene_morph = cycle_scene(
                        -1,
                        &store,
                        &mut params,
                        &mut current_scene_idx,
                        st.scene_smooth,
                        st.scene_transition_duration,
                    );
                    apply_scene_shape(&mut sim, &params, &old_t, &old_i, &mut rng);
                    if scene_morph.is_none() && mode == AppMode::Detached {
                        pending_apply = Some(params.clone());
                    }
                }
                PanelEvent::ExportScenes => {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("JSON", &["json"])
                        .set_file_name("escenas_enjambre.json")
                        .save_file()
                    {
                        if let Err(e) = store.export_to(&path) {
                            eprintln!("No se pudo exportar las escenas: {e}");
                        }
                    }
                }
                PanelEvent::ImportScenes => {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("JSON", &["json"])
                        .pick_file()
                    {
                        match SceneStore::import_from(&path) {
                            Ok(other) => {
                                store.merge(other);
                                if let Err(e) = store.save() {
                                    eprintln!("No se pudo guardar tras importar: {e}");
                                }
                                scenes_dirty = true;
                            }
                            Err(e) => eprintln!("No se pudo importar las escenas: {e}"),
                        }
                    }
                }
                PanelEvent::SetDefaultScene(name) => {
                    store.set_default(&name);
                    if let Err(e) = store.save() {
                        eprintln!("No se pudo guardar la escena predeterminada: {e}");
                    }
                    scenes_dirty = true;
                }
                PanelEvent::DeleteScene(name) => {
                    store.remove(&name);
                    if let Err(e) = store.save() {
                        eprintln!("No se pudo borrar la escena '{name}': {e}");
                    }
                    scenes_dirty = true;
                }
                PanelEvent::FormText(text) => {
                    params.shape_text = text;
                    params.shape_image = String::new();
                    build_shape(&mut sim, &params, &mut rng);
                }
                PanelEvent::FormImagePick => {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Imagen o vídeo", &[
                            "png", "jpg", "jpeg", "webp", "bmp", "mp4", "mov", "mkv", "webm",
                            "avi", "m4v",
                        ])
                        .pick_file()
                    {
                        params.shape_image = path.to_string_lossy().into_owned();
                        params.shape_text = String::new();
                        // El vídeo solo tiene sentido con el efecto de color
                        // (mosaico + overlay animado); lo activamos solo.
                        if is_video_path(&params.shape_image) {
                            params.shape_photo_color = true;
                            params.shape_tint = false;
                        }
                        build_shape(&mut sim, &params, &mut rng);
                    }
                }
                PanelEvent::FormImagePath(path) => {
                    params.shape_image = path;
                    params.shape_text = String::new();
                    if is_video_path(&params.shape_image) {
                        params.shape_photo_color = true;
                        params.shape_tint = false;
                    }
                    build_shape(&mut sim, &params, &mut rng);
                }
                PanelEvent::ReleaseShape => {
                    params.shape_text = String::new();
                    params.shape_image = String::new();
                    // Con foto: salida en reverso (imagen primero, luego soltar
                    // — lo gestiona clear_photo + advance_photo_effect). Sin
                    // foto: disolución normal de la forma.
                    if sim.photo.is_some() {
                        sim.clear_photo();
                    } else {
                        sim.clear_shape();
                    }
                }
                PanelEvent::SaveShape => {
                    // Guarda el descriptor activo con un nombre derivado (el texto,
                    // o el nombre de fichero de la imagen). Sin forma activa, no-op.
                    let (name, text, image) = if !params.shape_text.trim().is_empty() {
                        let t = params.shape_text.trim().to_string();
                        (t.clone(), t, String::new())
                    } else if !params.shape_image.is_empty() {
                        let name = std::path::Path::new(&params.shape_image)
                            .file_stem()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "imagen".to_string());
                        (name, String::new(), params.shape_image.clone())
                    } else {
                        eprintln!("No hay forma activa que guardar.");
                        continue;
                    };
                    shape_store.upsert(&name, text, image);
                    if let Err(e) = shape_store.save() {
                        eprintln!("No se pudo guardar la forma '{name}': {e}");
                    }
                    shapes_dirty = true;
                }
                PanelEvent::ApplyShape(name) => {
                    if let Some(s) = shape_store.get(&name) {
                        params.shape_text = s.text.clone();
                        params.shape_image = s.image.clone();
                        build_shape(&mut sim, &params, &mut rng);
                        if mode == AppMode::Detached {
                            pending_apply = Some(params.clone());
                        }
                    }
                }
                PanelEvent::DeleteShape(name) => {
                    shape_store.remove(&name);
                    if let Err(e) = shape_store.save() {
                        eprintln!("No se pudo borrar la forma '{name}': {e}");
                    }
                    shapes_dirty = true;
                }
                PanelEvent::SeqSetPlaylist(pl) => {
                    sequencer.playlist = pl;
                    let n = sequencer.playlist.entries.len();
                    if n == 0 {
                        sequencer.state = SeqPlayback::Stopped;
                        sequencer.idx = 0;
                        sequencer.timer = 0.0;
                    } else if sequencer.idx >= n {
                        sequencer.idx = n - 1;
                    }
                    if let Err(e) = sequencer.playlist.save() {
                        eprintln!("No se pudo guardar la playlist: {e}");
                    }
                }
                PanelEvent::SeqPlay => match sequencer.state {
                    SeqPlayback::Paused => sequencer.state = SeqPlayback::Playing,
                    SeqPlayback::Stopped => {
                        if let Some(v) = sequencer.find_valid(&store, 0, 1) {
                            sequencer.state = SeqPlayback::Playing;
                            seq_launch(
                                &mut sequencer,
                                v,
                                &store,
                                &mut params,
                                &mut sim,
                                &st,
                                &mut current_scene_idx,
                                &mut scene_morph,
                                &mut pending_apply,
                                mode == AppMode::Detached,
                                &mut rng,
                            );
                        } else {
                            eprintln!("Secuenciador: no hay entradas válidas que reproducir.");
                        }
                    }
                    SeqPlayback::Playing => {}
                },
                PanelEvent::SeqPause => {
                    if sequencer.state == SeqPlayback::Playing {
                        sequencer.state = SeqPlayback::Paused;
                    }
                }
                PanelEvent::SeqStop => {
                    sequencer.state = SeqPlayback::Stopped;
                    sequencer.idx = 0;
                    sequencer.timer = 0.0;
                }
                PanelEvent::SeqNext => {
                    let n = sequencer.playlist.entries.len();
                    if n > 0 {
                        if let Some(v) = sequencer.find_valid(&store, (sequencer.idx + 1) % n, 1) {
                            seq_launch(
                                &mut sequencer,
                                v,
                                &store,
                                &mut params,
                                &mut sim,
                                &st,
                                &mut current_scene_idx,
                                &mut scene_morph,
                                &mut pending_apply,
                                mode == AppMode::Detached,
                                &mut rng,
                            );
                        }
                    }
                }
                PanelEvent::SeqPrev => {
                    let n = sequencer.playlist.entries.len();
                    if n > 0 {
                        if let Some(v) =
                            sequencer.find_valid(&store, (sequencer.idx + n - 1) % n, -1)
                        {
                            seq_launch(
                                &mut sequencer,
                                v,
                                &store,
                                &mut params,
                                &mut sim,
                                &st,
                                &mut current_scene_idx,
                                &mut scene_morph,
                                &mut pending_apply,
                                mode == AppMode::Detached,
                                &mut rng,
                            );
                        }
                    }
                }
                PanelEvent::SeqJump(i) => {
                    let valid = sequencer
                        .playlist
                        .entries
                        .get(i)
                        .is_some_and(|e| store.get(&e.scene).is_some());
                    if valid {
                        seq_launch(
                            &mut sequencer,
                            i,
                            &store,
                            &mut params,
                            &mut sim,
                            &st,
                            &mut current_scene_idx,
                            &mut scene_morph,
                            &mut pending_apply,
                            mode == AppMode::Detached,
                            &mut rng,
                        );
                    }
                }
                PanelEvent::MusicAnalyze => {
                    if st.music_path.is_empty() {
                        eprintln!("Elige primero una pista de música (sección Grabación).");
                    } else if music_rx.is_none() {
                        eprintln!("Analizando '{}'…", st.music_path);
                        music_rx_path = st.music_path.clone();
                        music_rx = Some(shared::music::analyze_async(st.music_path.clone()));
                    }
                }
                PanelEvent::MusicPreviewToggle => {
                    if preview.take().is_none() && !st.music_path.is_empty() {
                        preview = shared::music::Preview::start(&st.music_path);
                        // La preescucha arranca la pista en 0: rebobinar beats.
                        beat_cursor = 0;
                        beat_count = 0;
                    }
                }
                PanelEvent::HidePanel => panel_visible = false,
                other => apply_local_event(
                    other,
                    &mut sim,
                    &mut params,
                    &mut st,
                    &mut pan_target,
                    Vec2::new(canvas.w, canvas.h),
                    &mut rng,
                    &mut step_once,
                ),
            }
        }

        // Telemetría hacia el panel.
        if mode == AppMode::Detached {
            if let Some(ipc) = &ipc {
                let connected = ipc.inbox.lock().unwrap().connected;
                if !connected {
                    init_sent = false;
                } else if !init_sent {
                    // Sincronización inicial: primero anunciamos la versión del
                    // protocolo, luego el estado real que el panel adopta.
                    let state = control_state(&params, &st);
                    if let Some(w) = ipc.writer.lock().unwrap().as_mut() {
                        let _ = write_msg(w, &TelemetryMsg::Version(IPC_VERSION));
                        let _ = write_msg(w, &TelemetryMsg::Init(Box::new(state)));
                    }
                    init_sent = true;
                    scenes_dirty = true; // envía la lista de escenas al panel nuevo
                    shapes_dirty = true; // y la biblioteca de formas
                    seq_dirty = true; // y la playlist del secuenciador
                }
                let tele = TelemetryMsg::Stats {
                    particle_count: sim.particles.len(),
                    fps: get_fps(),
                    blend: params.blend,
                    time_scale: params.time_scale,
                    recording: rec.is_some(),
                    show_frame,
                    frame_preset,
                    frame_w: preset_w,
                    frame_h: preset_h,
                    matrix: params.matrix,
                    canvas_size: st.canvas_size,
                    zoom_level: st.zoom_level,
                };
                if let Some(w) = ipc.writer.lock().unwrap().as_mut() {
                    let _ = write_msg(w, &tele);
                }
                // Lista de escenas (solo cuando cambia) y aplicación de params
                // tras cargar una escena (para que el panel no reenvíe los viejos).
                if scenes_dirty {
                    let list = TelemetryMsg::ScenesList {
                        names: store.names(),
                        default: store.default.clone().unwrap_or_default(),
                    };
                    if let Some(w) = ipc.writer.lock().unwrap().as_mut() {
                        let _ = write_msg(w, &list);
                    }
                    scenes_dirty = false;
                }
                if shapes_dirty {
                    let list = TelemetryMsg::ShapesList(shape_store.shapes.clone());
                    if let Some(w) = ipc.writer.lock().unwrap().as_mut() {
                        let _ = write_msg(w, &list);
                    }
                    shapes_dirty = false;
                }
                if seq_dirty {
                    let msg = TelemetryMsg::SeqPlaylist(sequencer.playlist.clone());
                    if let Some(w) = ipc.writer.lock().unwrap().as_mut() {
                        let _ = write_msg(w, &msg);
                    }
                    seq_dirty = false;
                }
                // Posición de reproducción del show (continuo, como Stats).
                let seq_tele = TelemetryMsg::SeqStatus {
                    state: sequencer.state,
                    idx: sequencer.idx,
                    elapsed: sequencer.timer,
                };
                if let Some(w) = ipc.writer.lock().unwrap().as_mut() {
                    let _ = write_msg(w, &seq_tele);
                }
                // Estado del análisis/preescucha de la música (continuo; son
                // cinco campos, no compensa un flag de "sucio").
                let music_tele = TelemetryMsg::MusicInfo {
                    analyzed: st.music_analyzed,
                    duration: st.music_duration,
                    onsets: st.music_onsets,
                    bpm: st.music_bpm,
                    previewing: st.music_previewing,
                };
                if let Some(w) = ipc.writer.lock().unwrap().as_mut() {
                    let _ = write_msg(w, &music_tele);
                }
                if let Some(p) = pending_apply.take() {
                    if let Some(w) = ipc.writer.lock().unwrap().as_mut() {
                        let _ = write_msg(w, &TelemetryMsg::ApplyParams(Box::new(p)));
                    }
                }
                if let Some(p) = pending_paused.take() {
                    if let Some(w) = ipc.writer.lock().unwrap().as_mut() {
                        let _ = write_msg(w, &TelemetryMsg::SetPaused(p));
                    }
                }
            }
        }

        // --- Cámara: zoom y desplazamiento ---
        let mouse = Vec2::from(mouse_position());

        // Zoom con la rueda, hacia el cursor (mantiene fijo el punto bajo él).
        let wheel = mouse_wheel().1;
        if wheel != 0.0 && !want_pointer {
            let world_before = cam_s2w(&make_camera(st.zoom_level, pan_target, canvas), mouse, canvas);
            let factor = if wheel > 0.0 { 1.15 } else { 1.0 / 1.15 };
            st.zoom_level = (st.zoom_level * factor).clamp(0.2, 30.0);
            let world_after = cam_s2w(&make_camera(st.zoom_level, pan_target, canvas), mouse, canvas);
            pan_target += world_before - world_after;
        }

        // Desplazamiento arrastrando con el botón derecho o central.
        if is_mouse_button_down(MouseButton::Right) || is_mouse_button_down(MouseButton::Middle) {
            let cam = make_camera(st.zoom_level, pan_target, canvas);
            pan_target += cam_s2w(&cam, last_mouse, canvas) - cam_s2w(&cam, mouse, canvas);
        }

        // --- Edición del recuadro de encuadre con el botón izquierdo ---
        // (solo si está visible; si no se agarra, el izquierdo pinta como siempre).
        if show_frame && !want_pointer {
            let fcam = make_camera(st.zoom_level, pan_target, canvas);
            let hw = frame_height * frame_aspect / 2.0;
            let hh = frame_height / 2.0;
            if frame_drag.is_none() && is_mouse_button_pressed(MouseButton::Left) {
                let corners = [
                    frame_center + Vec2::new(-hw, -hh),
                    frame_center + Vec2::new(hw, -hh),
                    frame_center + Vec2::new(hw, hh),
                    frame_center + Vec2::new(-hw, hh),
                ];
                let near_corner = corners
                    .iter()
                    .any(|c| (cam_w2s(&fcam, *c, canvas) - mouse).length() < 14.0);
                let wm = cam_s2w(&fcam, mouse, canvas);
                let inside = wm.x > frame_center.x - hw
                    && wm.x < frame_center.x + hw
                    && wm.y > frame_center.y - hh
                    && wm.y < frame_center.y + hh;
                if near_corner {
                    frame_drag = Some(FrameDrag::Resize);
                } else if inside {
                    frame_drag = Some(FrameDrag::Move);
                }
            }
            if let Some(drag) = frame_drag {
                if is_mouse_button_down(MouseButton::Left) {
                    let now = cam_s2w(&fcam, mouse, canvas);
                    match drag {
                        FrameDrag::Move => {
                            frame_center += now - cam_s2w(&fcam, last_mouse, canvas);
                        }
                        FrameDrag::Resize => {
                            frame_height = (2.0 * (now.y - frame_center.y).abs()).max(10.0);
                        }
                    }
                } else {
                    frame_drag = None;
                }
            }
        } else {
            frame_drag = None;
        }

        last_mouse = mouse;

        let camera = make_camera(st.zoom_level, pan_target, canvas);

        // Herramienta del ratón (fuera del panel y si no movemos el recuadro):
        // Fuerza atrae/repele el enjambre; Pincel pinta o borra.
        sim.pointer = None;
        if frame_drag.is_none() && !want_pointer && is_mouse_button_down(MouseButton::Left) {
            let pos = cam_s2w(&camera, mouse, canvas);
            match st.tool {
                Tool::Force => sim.pointer = Some(pos),
                Tool::Brush => match st.brush {
                    Brush::Add => {
                        let count = (st.brush_size / 5.0).max(1.0) as usize;
                        for _ in 0..count {
                            let ang = rng.gen_range(0.0..std::f32::consts::TAU);
                            let rad = rng.gen_range(0.0..st.brush_size);
                            sim.add(
                                pos + Vec2::new(ang.cos() * rad, ang.sin() * rad),
                                hue_for_index(st.active_color),
                            );
                        }
                    }
                    Brush::Erase => sim.erase_near(pos, st.brush_size),
                },
            }
        }

        // Modulación de audio sobre brillo/tamaño/resplandor (transitoria durante
        // el render y la grabación; se restaura tras volcar el frame). El efecto
        // "bandas → colores" es GPU-only, aquí no tiene equivalente barato.
        let saved_brightness = params.brightness;
        let saved_point_size = params.point_size;
        let saved_bloom_intensity = params.bloom_intensity;
        if audio_mod_on {
            match params.audio_target {
                AudioTarget::Brightness => {
                    params.brightness = (params.brightness * audio_gain).min(1.0);
                }
                AudioTarget::Size => {
                    params.point_size = (params.point_size * audio_gain).min(80.0);
                }
                AudioTarget::Bloom => {
                    params.bloom_intensity = (params.bloom_intensity * audio_gain).min(4.0);
                }
                AudioTarget::Speed | AudioTarget::Force => {}
            }
        }

        // Overlays del lienzo (borde + recuadro de encuadre) en coordenadas de
        // mundo. Se dibujan encima de las partículas y NUNCA llevan estela.
        let draw_overlays = || {
            draw_rectangle_lines(
                0.0,
                0.0,
                world.x,
                world.y,
                2.0 / st.zoom_level,
                Color::new(0.3, 0.3, 0.35, 1.0),
            );
            if show_frame {
                let fw = frame_height * frame_aspect;
                let x0 = frame_center.x - fw / 2.0;
                let y0 = frame_center.y - frame_height / 2.0;
                let th = 2.0 / st.zoom_level;
                let edge = Color::new(1.0, 1.0, 1.0, 0.9);
                let thirds = Color::new(1.0, 1.0, 1.0, 0.30);
                draw_rectangle_lines(x0, y0, fw, frame_height, th, edge);
                for k in 1..3 {
                    let x = x0 + fw * k as f32 / 3.0;
                    draw_line(x, y0, x, y0 + frame_height, th * 0.6, thirds);
                    let y = y0 + frame_height * k as f32 / 3.0;
                    draw_line(x0, y, x0 + fw, y, th * 0.6, thirds);
                }
                let hs = 6.0 / st.zoom_level;
                for (cx, cy) in [
                    (x0, y0),
                    (x0 + fw, y0),
                    (x0 + fw, y0 + frame_height),
                    (x0, y0 + frame_height),
                ] {
                    draw_rectangle(cx - hs, cy - hs, hs * 2.0, hs * 2.0, edge);
                }
            }
        };

        // Render del mundo. Con estelas dibujamos en un buffer persistente que se
        // desvanece un poco cada frame; sin estelas, directo a pantalla.
        if params.trails {
            let sw = screen_width();
            let sh = screen_height();
            let need_new = trails_rt.as_ref().map_or(true, |rt| {
                (rt.texture.width() - sw).abs() > 0.5 || (rt.texture.height() - sh).abs() > 0.5
            });
            if need_new {
                let rt = render_target(sw as u32, sh as u32);
                rt.texture.set_filter(FilterMode::Linear);
                trails_rt = Some(rt);
            }
            let rt = trails_rt.as_ref().unwrap();
            let mut tcam = make_camera(st.zoom_level, pan_target, canvas);
            tcam.render_target = Some(rt.clone());
            set_camera(&tcam);
            // Desvanecido: rectángulo negro translúcido sobre el mundo visible.
            let tl = cam_s2w(&tcam, vec2(canvas.x, canvas.y), canvas);
            let br = cam_s2w(&tcam, vec2(canvas.x + canvas.w, canvas.y + canvas.h), canvas);
            draw_rectangle(
                tl.x.min(br.x),
                tl.y.min(br.y),
                (br.x - tl.x).abs(),
                (br.y - tl.y).abs(),
                Color::new(0.0, 0.0, 0.0, params.trail_fade),
            );
            renderer.draw_particles(&sim, &params);
            // Volcar el buffer a la pantalla y pintar los overlays encima.
            set_default_camera();
            clear_background(BLACK);
            draw_texture_ex(
                &rt.texture,
                0.0,
                0.0,
                WHITE,
                DrawTextureParams {
                    dest_size: Some(vec2(sw, sh)),
                    flip_y: false,
                    ..Default::default()
                },
            );
            set_camera(&camera);
            draw_overlays();
            set_default_camera();
        } else {
            trails_rt = None; // liberar el buffer cuando no se usa
            set_camera(&camera);
            renderer.draw(&sim, &params);
            draw_overlays();
            set_default_camera();
        }

        // Grabación: renderizamos la escena en vertical al render target y
        // volcamos el frame a ffmpeg (invisible para la ventana). Si falla la
        // escritura (ffmpeg murió), cerramos la grabación.
        if let Some(r) = rec.as_mut() {
            let vw = frame_height * frame_aspect;
            let rcam = record_camera(&r.rt, frame_center, vw, frame_height);
            set_camera(&rcam);
            if params.trails {
                // El render target del `Recorder` persiste entre frames, así que
                // acumula por sí solo: solo lo desvanecemos y pintamos encima.
                draw_rectangle(
                    frame_center.x - vw / 2.0,
                    frame_center.y - frame_height / 2.0,
                    vw,
                    frame_height,
                    Color::new(0.0, 0.0, 0.0, params.trail_fade),
                );
                renderer.draw_particles(&sim, &params);
            } else {
                renderer.draw(&sim, &params);
            }
            set_default_camera();
            if let Err(e) = r.capture() {
                eprintln!("Grabación detenida (error escribiendo a ffmpeg): {e}");
                finish_recording(rec.take().unwrap(), &mut rec_video, rec_music);
            }
        }
        // Restaurar brillo/tamaño/resplandor base tras el render/grabación.
        params.brightness = saved_brightness;
        params.point_size = saved_point_size;
        params.bloom_intensity = saved_bloom_intensity;
        if let Some(r) = &rec {
            draw_text(
                &format!("● REC  {:.1}s", r.frames as f32 / REC_FPS as f32),
                20.0,
                40.0,
                34.0,
                RED,
            );
        }

        // Con el panel oculto, un recordatorio discreto de cómo recuperarlo.
        if mode == AppMode::Embedded && !panel_visible {
            let hint = "H: mostrar panel";
            let d = measure_text(hint, None, 22, 1.0);
            draw_text(
                hint,
                screen_width() - d.width - 16.0,
                28.0,
                22.0,
                Color::new(0.8, 0.8, 0.85, 0.7),
            );
        }

        if mode == AppMode::Embedded {
            egui_macroquad::draw();
        }
        next_frame().await;
    }
}
