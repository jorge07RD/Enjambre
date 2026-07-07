//! Enjambre en GPU (experimental): la física corre entera en compute shaders
//! (wgpu) y el render lee los buffers sin pasar por la CPU. No toca la app
//! principal (`sim`); comparte `shared` (parámetros, escenas y el panel egui).
//!
//! Hitos 3-5: paridad de todos los modos de interacción (Boids incluido) con
//! transiciones fluidas, paridad visual (estilos, flechas, bloom y estelas),
//! escenas (`scenes.json`, la misma biblioteca que `sim`) y panel de control
//! embebido (egui-wgpu) reutilizando `shared::config_panel`.
//! Hito 6: formas/texto (shape.rs + resortes en el kernel) y grabación de
//! vídeo con música (rec.rs, R o el botón del panel).
//!
//! Uso: `cargo run --release -p sim-gpu [n_partículas] [vertical] [2k|4k]` (por
//! defecto 20000, horizontal, 1080p). `vertical` (o `v`/`movil`) abre en retrato
//! 9:19.5 (pantalla completa de móvil); `2k`/`4k` graban a 1440/2160 de lado
//! menor (si no, Full HD 1080). La grabación se renderiza a esa resolución, NO a
//! la de la ventana, así sale nítida y en la orientación elegida.
//! Teclas: Espacio = pausa · . = paso · C = vaciar · F = llenar · S = soltar
//! forma · Enter = formar el texto del panel · N/P = escena sig./ant. ·
//! M = aleatorizar matriz · X = auto-aleatorizar · V = deriva gradual ·
//! E = estelas · Y = bloom · A = atraer al centro · U = anti-aglomeración ·
//! B = contorno · R = grabar · H = panel · G = grid/naive (debug) ·
//! 1..9/0 = velocidad 10..90/100 % · +/- = ±10 % · Esc = salir.
//! No aplican en el visor GPU (mundo fijo, panel embebido): L/Z (zoom/lienzo),
//! D (separar panel), y G no alterna recuadro (aquí es el debug grid/naive).

mod gpu_sim;
mod rec;
mod shape;

use gpu_sim::{AudioDrive, GpuSim, Mosaic, ShapeDrive};
use rand::Rng;
use shared::{
    config_panel, example_store, hue_for_index, is_video_path, ui_theme, AudioSource, AudioTarget,
    BeatAction, PanelEvent, PanelState, Playlist, SceneStore, SeqPlayback, ShapeStore, SimParams,
    VideoSource, NUM_COLORS,
};
use std::sync::Arc;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

/// Mundo fijo (como el bench de la CPU); la ventana lo estira a su tamaño.
/// Mundo horizontal (16:10, por defecto) y vertical (9:16, para móvil/TikTok).
/// El vertical se elige con el argumento `vertical`; la ventana y la grabación
/// salen en retrato automáticamente (la grabación captura el tamaño de ventana).
const WORLD_H: [f32; 2] = [1600.0, 1000.0];
/// Vertical 9:19.5 (pantalla completa de móviles modernos, p. ej. Galaxy S23).
const WORLD_V: [f32; 2] = [900.0, 1950.0];
/// Tamaño lógico inicial de ventana en cada orientación (mismo aspecto que el
/// mundo, para que las partículas no se deformen).
const WIN_H: (f64, f64) = (1280.0, 800.0);
const WIN_V: (f64, f64) = (405.0, 878.0);

/// Ganancia del empuje de la onda de choque sobre `pulse_gain` (se integra
/// como aceleración ×force×dt en sim.wgsl; ajustar aquí si empuja poco/mucho).
const SHOCK_GAIN: f32 = 12.0;

/// Dimensiones de grabación (pares) para una calidad `minor` (el lado MENOR:
/// ancho en vertical, alto en horizontal), respetando el aspecto del mundo.
/// 1080 = Full HD, 1440 = 2K. Independiente del tamaño de ventana.
fn rec_dims(world: [f32; 2], minor: u32) -> (u32, u32) {
    let (ww, wh) = (world[0], world[1]);
    let m = minor as f32;
    let (w, h) = if wh >= ww {
        (m, m * wh / ww) // retrato: el ancho es el lado menor
    } else {
        (m * ww / wh, m) // apaisado: el alto es el lado menor
    };
    (((w.round() as u32) & !1).max(2), ((h.round() as u32) & !1).max(2))
}

/// Transición de escena en curso (port del `SceneMorph` de `sim/src/main.rs`):
/// interpola los parámetros numéricos de `from` a `target`; el cruce del
/// modo/matriz lo lleva el blend de interacción (`start_transition`).
struct SceneMorph {
    from: Box<SimParams>,
    target: Box<SimParams>,
    blend: f32,
    dur: f32,
}

/// Aplica la escena `target` a `params`. Si `smooth`, arranca un morph y lo
/// devuelve; si no, la aplica al instante (devuelve `None`). Igual que en
/// `sim`, sin la reconstrucción de formas (sin formas en el visor GPU).
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
    // Interacción destino + cruce gradual (viejo → nuevo) con el blend existente.
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
    params.trails = target.trails;
    params.bloom = target.bloom;
    params.boids_scope = target.boids_scope;
    // Descriptor de la forma (mensaje/imagen): el llamador reconstruye la
    // forma si cambió (ver `State::apply_scene_shape`).
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

/// Fecha de última modificación de un fichero (para la recarga en caliente).
/// `None` si no existe o no se puede leer.
fn file_mtime(path: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Estado runtime de la forma activa (port de la parte de forma de
/// `Simulation` en la CPU): los puntos meta viven en la GPU; aquí solo la
/// mezcla de aparición/disolución y cuántos puntos hay subidos.
#[derive(Default)]
struct ShapeState {
    /// Puntos meta subidos al buffer (0 = sin forma).
    n: u32,
    blend: f32,
    /// Objetivo de la mezcla: 1 mientras hay forma, 0 al soltarla.
    target: f32,
}

impl ShapeState {
    /// Avanza la mezcla hacia su objetivo durante `duration` s (0 =
    /// instantáneo). Al terminar de disolverse, descarta la forma.
    fn advance(&mut self, dt: f32, duration: f32) {
        let step = if duration > 1e-3 { dt / duration } else { 1.0 };
        if self.blend < self.target {
            self.blend = (self.blend + step).min(self.target);
        } else if self.blend > self.target {
            self.blend = (self.blend - step).max(self.target);
            if self.blend <= 1e-4 {
                self.n = 0;
            }
        }
    }

    /// Factores que consumen los kernels (mismas curvas que la CPU): mezcla
    /// suavizada (ease-in/out) escalando resorte, interacción residual y
    /// evasión del fondo.
    fn drive(&self, shape_strength: f32) -> ShapeDrive {
        let sb = self.blend.clamp(0.0, 1.0);
        let b = sb * sb * (3.0 - 2.0 * sb);
        if self.n == 0 || b <= 1e-3 {
            return ShapeDrive::default();
        }
        let fix = shape_strength.clamp(0.0, 1.0);
        ShapeDrive {
            n: self.n,
            k: (0.02 + fix * 0.38) * b,
            inter: 1.0 - b * (1.0 - 0.35 * (1.0 - fix)),
            avoid: 2.5 * b,
        }
    }
}

struct State {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    sim: GpuSim,
    params: SimParams,
    /// Dimensiones del mundo (horizontal o vertical). Antes era la constante
    /// `WORLD`; ahora runtime para poder elegir orientación al arrancar.
    world: [f32; 2],
    /// Calidad de grabación: lado menor en píxeles (1080 = Full HD, 1440 = 2K).
    /// La grabación se renderiza a esta resolución, no a la de la ventana.
    rec_minor: u32,
    /// Hook de prueba (`ENJAMBRE_AUTOREC=segundos`): graba automáticamente al
    /// arrancar y sale al terminar, para verificar la grabación sin teclado.
    /// `None` en uso normal.
    autorec_left: Option<u32>,
    // Escenas: la misma biblioteca (`scenes.json`) que la app CPU. Aquí solo
    // se lee/escribe bajo demanda; el `sim` sigue siendo el dueño habitual.
    store: SceneStore,
    scene_idx: usize,
    morph: Option<SceneMorph>,
    autoplay_timer: f32,
    auto_rand_timer: f32,
    // Secuenciador (show): reproduce la playlist (`playlist.json`) con duración
    // propia por entrada. La playlist vive en `panel.seq_playlist` (la edita el
    // panel embebido); aquí guardamos solo el estado de reproducción.
    seq_state: SeqPlayback,
    seq_idx: usize,
    seq_timer: f32,
    // Recarga en caliente: mtime de los ficheros de datos para detectar
    // ediciones externas (p.ej. autoradas) y recargar sin reiniciar.
    reload_timer: f32,
    scenes_mtime: Option<std::time::SystemTime>,
    playlist_mtime: Option<std::time::SystemTime>,
    shapes_mtime: Option<std::time::SystemTime>,
    // Formas (texto/imagen) y grabación de vídeo (hito 6).
    shape_store: ShapeStore,
    // Fase A del efecto foto: las partículas se acomodan a una rejilla que
    // cubre la imagen (`shape`) y toman su color (mosaico). Fase B: la foto
    // real se funde encima (`overlay_*`) tras acomodarse. `photo_loaded` = hay
    // una textura subida; `photo_extent` = caja de la foto en mundo.
    shape: ShapeState,
    overlay_reveal: f32,
    overlay_target: f32,
    photo_extent: [f32; 2],
    photo_loaded: bool,
    /// En salida: la imagen se desvanece primero y luego se sueltan las
    /// partículas (transición de entrada en reverso).
    photo_releasing: bool,
    /// Ruta del vídeo pendiente de reproducir: se rellena al cargar el vídeo y
    /// la reproducción arranca DIFERIDA (cuando la imagen ya se formó del todo)
    /// vaciándola en `video`. `None` = imagen fija o ya reproduciendo.
    video_pending: Option<String>,
    /// Fuente de fotogramas en streaming (solo tras arrancar). `video_frozen` se
    /// activa al soltar para congelar el frame visible durante el reverso.
    video: Option<VideoSource>,
    video_frozen: bool,
    /// Para muxear el audio del vídeo en la grabación: (ruta, frame de grabación
    /// en que arrancó la reproducción) y si la grabación lleva música.
    rec_video: Option<(String, u32)>,
    rec_music: bool,
    recorder: Option<rec::Recorder>,
    // --- Audio en vivo + sincronía con la pista (port del bucle CPU) ---
    /// Captura en vivo (micrófono o sistema); `None` si `audio_reactive` está
    /// apagado o no se pudo abrir. `audio_source_active` es la fuente con la
    /// que se abrió, para reiniciar la captura si el panel la cambia.
    audio_in: Option<shared::audio::AudioIn>,
    audio_source_active: AudioSource,
    /// Nivel de audio suavizado 0..1 (ataque rápido, caída lenta).
    audio_level: f32,
    /// Energía por banda suavizada de la captura en vivo (graves/medios/agudos).
    audio_bands_live: [f32; 3],
    /// Resultado del análisis de la pista (envolvente + bandas + onsets/bpm) y
    /// el canal del hilo de fondo mientras se analiza.
    music: Option<shared::music::MusicAnalysis>,
    music_rx: Option<std::sync::mpsc::Receiver<std::io::Result<shared::music::MusicAnalysis>>>,
    /// Ruta a la que corresponde `music_rx` (si el usuario cambia de pista a
    /// medio análisis, el resultado que llegue no vale).
    music_rx_path: String,
    /// Ruta a la que corresponde `music` (si no coincide con `panel.music_path`,
    /// el análisis está desactualizado y la sincronía no actúa).
    music_path_analyzed: String,
    /// Preescucha de la pista (`ffplay`); su reloj aproximado conduce la
    /// sincronía en vivo (grabando, el reloj exacto es `recorder.frames/60`).
    preview: Option<shared::music::Preview>,
    /// Cursor sobre los onsets analizados y contador para el divisor de beats.
    beat_cursor: usize,
    beat_count: u32,
    /// Pulso transitorio de beat (decae ~0.2 s) que alimenta `audio_gain`.
    beat_pulse: f32,
    /// Empuje transitorio de la onda de choque (decae ~0.25 s), sube al kernel
    /// de física como `GpuParams::shock`.
    shock_pulse: f32,
    // Panel egui embebido (mismo `config_panel` que `sim` y `panel`).
    panel: PanelState,
    panel_visible: bool,
    egui_state: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,
    /// Avanzar un solo paso aunque esté en pausa (botón "Paso").
    step_once: bool,
    // FPS en el título (media sobre una ventana de frames).
    frames: u32,
    t0: Instant,
    last_frame: Instant,
}

impl State {
    async fn new(window: Arc<Window>, count: u32, world: [f32; 2], rec_minor: u32) -> State {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let surface = instance.create_surface(window.clone()).expect("superficie wgpu");
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .expect("ningún adaptador de GPU compatible");
        eprintln!("GPU: {}", adapter.get_info().name);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default(), None)
            .await
            .expect("dispositivo wgpu");

        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .expect("configuración de superficie");
        config.present_mode = wgpu::PresentMode::AutoVsync;
        surface.configure(&device, &config);

        // Escenas: la biblioteca compartida; si aún no existe, los ejemplos
        // (solo en memoria: `scenes.json` lo persiste el `sim`).
        let mut store = SceneStore::load();
        if store.scenes.is_empty() {
            store = example_store();
        }

        // Parámetros iniciales: la escena predeterminada si la hay.
        let mut params = SimParams::default();
        let mut scene_idx = 0;
        if let Some(name) = store.default.clone() {
            if let Some(pos) = store.scenes.iter().position(|s| s.name == name) {
                scene_idx = pos;
                params = store.scenes[pos].params.settled();
            }
        }

        let mut rng = rand::thread_rng();
        let sim = GpuSim::new(
            &device,
            config.format,
            &params,
            world,
            count,
            (config.width, config.height),
            &mut rng,
        );

        // Validar el counting sort del grid antes de fiar las fuerzas a él.
        match sim.validate_grid(&device, &queue) {
            Ok(()) => eprintln!("Grid GPU validado (prefix sum: total == {count})."),
            Err(e) => panic!("Validación del grid GPU fallida: {e}"),
        }

        // Panel egui: mismo tema/iconos que el panel de la app CPU.
        let egui_ctx = egui::Context::default();
        ui_theme::apply(&egui_ctx);
        let egui_state = egui_winit::State::new(
            egui_ctx,
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
            Some(device.limits().max_texture_dimension_2d as usize),
        );
        let egui_renderer = egui_wgpu::Renderer::new(&device, config.format, None, 1, false);

        let shape_store = ShapeStore::load();
        let mut panel = PanelState::default();
        panel.scenes = store.names();
        panel.default_scene = store.default.clone().unwrap_or_default();
        panel.particle_count = count as usize;
        panel.saved_shapes = shape_store.shapes.clone();
        // Secuenciador: la playlist del show (misma `playlist.json` que el CPU).
        panel.seq_playlist = Playlist::load();
        let scenes_mtime = file_mtime(&shared::scenes_path());
        let playlist_mtime = file_mtime(&shared::playlist_path());
        let shapes_mtime = file_mtime(&shared::shapes_path());
        let audio_source_active = params.audio_source;

        // Hook de prueba (`ENJAMBRE_MUSIC=<ruta>`): fija la pista, activa la
        // sincronía y arranca el análisis ya, para poder verificar la
        // reactividad al audio grabando sin tocar el panel.
        let mut music_rx = None;
        let mut music_rx_path = String::new();
        if let Ok(path) = std::env::var("ENJAMBRE_MUSIC") {
            panel.music_path = path.clone();
            panel.music_sync.enabled = true;
            music_rx_path = path.clone();
            music_rx = Some(shared::music::analyze_async(path));
        }

        State {
            window,
            surface,
            device,
            queue,
            config,
            sim,
            params,
            world,
            rec_minor,
            autorec_left: std::env::var("ENJAMBRE_AUTOREC")
                .ok()
                .and_then(|s| s.parse::<f32>().ok())
                .map(|secs| (secs.max(0.0) * rec::REC_FPS as f32) as u32),
            store,
            scene_idx,
            morph: None,
            autoplay_timer: 0.0,
            auto_rand_timer: 0.0,
            seq_state: SeqPlayback::Stopped,
            seq_idx: 0,
            seq_timer: 0.0,
            reload_timer: 0.0,
            scenes_mtime,
            playlist_mtime,
            shapes_mtime,
            shape_store,
            shape: ShapeState::default(),
            overlay_reveal: 0.0,
            overlay_target: 0.0,
            photo_extent: [world[0], world[1]],
            photo_loaded: false,
            photo_releasing: false,
            video_pending: None,
            video: None,
            video_frozen: false,
            rec_video: None,
            rec_music: false,
            recorder: None,
            audio_in: None,
            audio_source_active,
            audio_level: 0.0,
            audio_bands_live: [0.0; 3],
            music: None,
            music_rx,
            music_rx_path,
            music_path_analyzed: String::new(),
            preview: None,
            beat_cursor: 0,
            beat_count: 0,
            beat_pulse: 0.0,
            shock_pulse: 0.0,
            panel,
            // En vertical la ventana es estrecha: ocultamos el panel por defecto
            // para ver el lienzo completo (se muestra con H). En horizontal, visible.
            panel_visible: world[0] >= world[1],
            egui_state,
            egui_renderer,
            step_once: false,
            frames: 0,
            t0: Instant::now(),
            last_frame: Instant::now(),
        }
    }

    fn resize(&mut self, w: u32, h: u32) {
        if w > 0 && h > 0 {
            self.config.width = w;
            self.config.height = h;
            self.surface.configure(&self.device, &self.config);
            self.sim.resize(&self.device, w, h);
            // La captura tiene resolución fija: cambiar la ventana la corta.
            if let Some(r) = self.recorder.take() {
                eprintln!("Grabación detenida por el cambio de tamaño de la ventana.");
                self.finish_recording(r);
            }
        }
    }

    /// (Re)construye la forma desde el descriptor de `params` (= `build_shape`
    /// de la CPU). En modo "recrear colores de la foto" NO se mueve nada: se
    /// sube la imagen como textura y se revela su color encima del enjambre
    /// (`start_photo`). En modo texto/silueta las partículas migran a los
    /// puntos meta con el resorte de siempre.
    fn build_shape(&mut self) {
        // Modo foto: efecto en dos fases (mosaico + superposición).
        if self.params.shape_photo_color && !self.params.shape_image.is_empty() {
            self.start_photo();
            return;
        }
        // Salir del modo foto: la superposición se desvanece.
        self.overlay_target = 0.0;

        // Texto/silueta: solo una parte de las partículas forma la figura y el
        // resto queda de ambiente.
        let count = ((self.sim.count as usize) * 7 / 10).max(1);
        let mut rng = rand::thread_rng();
        let pts = if !self.params.shape_text.trim().is_empty() {
            shape::text_to_points(&self.params.shape_text, self.world, count, &mut rng)
        } else if !self.params.shape_image.is_empty() {
            shape::image_points_from_path(&self.params.shape_image, self.world, count, &mut rng)
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        if pts.is_empty() {
            // Sin nueva forma. Con foto activa, salida en reverso (la imagen ya
            // se desvanece; el bucle suelta las partículas cuando la imagen se
            // fue). Sin foto, disolución normal de la forma.
            if self.photo_loaded {
                self.photo_releasing = true;
            } else {
                self.shape.target = 0.0;
                if self.shape.blend <= 1e-4 {
                    self.shape.n = 0;
                }
            }
            return;
        }
        // Una nueva forma (texto/silueta) reemplaza al efecto foto.
        self.photo_loaded = false;
        self.photo_releasing = false;
        self.video = None;
        self.video_pending = None;
        self.shape.n = self.sim.upload_shape_targets(&self.queue, &pts);
        self.shape.blend = 0.0;
        self.shape.target = 1.0;
        if self.params.shape_tint {
            self.sim
                .tint(&self.queue, self.shape.n, hue_for_index(self.params.shape_color));
        }
    }

    /// Arranca el efecto foto en dos fases: (A) las partículas se reparten en
    /// una rejilla que cubre la imagen y toman su color (mosaico); (B) la foto
    /// real se funde encima al terminar de acomodarse (ver el bucle).
    fn start_photo(&mut self) {
        // Vídeo o imagen fija: en ambos casos arrancamos con un primer
        // fotograma RGBA. El vídeo, además, refresca la textura en el bucle
        // (`advance_video`) hasta que se suelta (se congela en el frame visible).
        let (rgba, w, h) = if is_video_path(&self.params.shape_image) {
            // Solo el primer fotograma para el mosaico/overlay; el streaming se
            // abre diferido (cuando la imagen se forma del todo).
            let Some(frame) = VideoSource::decode_first_frame(&self.params.shape_image, 720) else {
                self.overlay_target = 0.0;
                return;
            };
            self.video = None;
            self.video_frozen = false;
            self.video_pending = Some(self.params.shape_image.clone());
            frame
        } else {
            self.video = None;
            self.video_pending = None;
            let Some(frame) = shape::decode_image_rgba(&self.params.shape_image) else {
                self.overlay_target = 0.0;
                return;
            };
            frame
        };
        self.sim.upload_photo(&self.device, &self.queue, &rgba, w, h);
        // Caja de la foto en mundo: 90% del lienzo, centrada, aspecto preservado.
        let (iw, ih) = (w as f32, h as f32);
        let scale = (self.world[0] * 0.9 / iw).min(self.world[1] * 0.9 / ih);
        self.photo_extent = [iw * scale, ih * scale];
        self.photo_loaded = true;
        self.photo_releasing = false;
        // Fase B (overlay) arranca en 0 y espera a que se acomoden.
        self.overlay_reveal = 0.0;
        self.overlay_target = 0.0;
        // Fase A: se reclutan partículas SOLO en la zona opaca de la imagen
        // (en un PNG sin fondo, nada de partículas en lo transparente); toman
        // su color. Las no reclutadas se desvanecen (ver particles.wgsl).
        let recruit = ((self.sim.count as usize) * 9 / 10).max(1);
        let center = [self.world[0] * 0.5, self.world[1] * 0.5];
        let pts = shape::mosaic_points(&rgba, w as usize, h as usize, center, self.photo_extent, recruit);
        self.shape.n = self.sim.upload_shape_targets(&self.queue, &pts);
        self.shape.blend = 0.0;
        self.shape.target = 1.0;
    }

    /// Al cambiar de escena: reconstruye la forma SOLO si el descriptor
    /// cambió (mismo criterio que `apply_scene_shape` en la CPU).
    fn apply_scene_shape(&mut self, old_text: &str, old_image: &str) {
        if self.params.shape_text != old_text || self.params.shape_image != old_image {
            self.build_shape();
        }
    }

    /// Cierra la grabación y, si durante ella se reprodujo un vídeo del efecto
    /// foto, muxea su audio en el `.mp4` al offset en que apareció.
    fn finish_recording(&mut self, r: rec::Recorder) {
        let path = r.finish(&self.device);
        if let Some((src, start_frame)) = self.rec_video.take() {
            let offset = start_frame as f32 / rec::REC_FPS as f32;
            shared::video::overlay_audio(&path, &src, offset, self.rec_music);
        }
        // Restaura la textura de escena al tamaño de ventana (durante la
        // grabación se subió a la resolución de grabación).
        self.sim.resize(&self.device, self.config.width, self.config.height);
        self.panel.recording = false;
    }

    /// Arranca o detiene la grabación de vídeo (tecla R o botón del panel).
    fn toggle_record(&mut self) {
        if let Some(r) = self.recorder.take() {
            self.finish_recording(r);
            return;
        }
        // Grabar a la resolución elegida (1080/2K), no a la de la ventana:
        // renderizamos la escena a una textura HDR de ese tamaño (nítida) y la
        // capturamos; la ventana solo muestra una reducción.
        let (rw, rh) = rec_dims(self.world, self.rec_minor);
        self.sim.resize(&self.device, rw, rh);
        match rec::Recorder::start(
            &self.device,
            rw,
            rh,
            &self.panel.video_dir,
            &self.panel.music_path,
        ) {
            Ok(r) => {
                self.recorder = Some(r);
                self.rec_music = !self.panel.music_path.is_empty();
                self.rec_video = None;
                self.panel.recording = true;
                // El reloj musical de la grabación arranca en 0 (frames/60):
                // rebobinar el cursor de beats para que no venga adelantado de
                // una preescucha previa.
                self.beat_cursor = 0;
                self.beat_count = 0;
                // Show ligado a la grabación: arranca la secuencia desde el
                // principio junto con ella (y, sin bucle, la para al terminar).
                if self.panel.seq_playlist.start_on_record {
                    if let Some(v) = self.seq_find_valid(0, 1) {
                        self.seq_state = SeqPlayback::Playing;
                        self.seq_launch(v);
                    }
                }
            }
            Err(e) => {
                eprintln!("No se pudo iniciar la grabación (¿está ffmpeg?): {e}");
                // Restaura el offscreen al tamaño de ventana (lo subimos arriba).
                self.sim.resize(&self.device, self.config.width, self.config.height);
            }
        }
    }

    /// Refresca la lista de escenas que muestra el panel.
    fn refresh_scenes(&mut self) {
        self.panel.scenes = self.store.names();
        self.panel.default_scene = self.store.default.clone().unwrap_or_default();
    }

    /// Guarda `scenes.json`, actualiza el mtime propio (para no auto-recargar) y
    /// refresca el panel.
    fn persist_scenes(&mut self) {
        if let Err(e) = self.store.save() {
            eprintln!("No se pudo guardar scenes.json: {e}");
        }
        self.scenes_mtime = file_mtime(&shared::scenes_path());
        self.refresh_scenes();
    }

    /// Guarda `shapes.json`, actualiza el mtime propio y refresca el panel.
    fn persist_shapes(&mut self) {
        if let Err(e) = self.shape_store.save() {
            eprintln!("No se pudo guardar shapes.json: {e}");
        }
        self.shapes_mtime = file_mtime(&shared::shapes_path());
        self.panel.saved_shapes = self.shape_store.shapes.clone();
    }

    /// Cambia a la escena siguiente (+1) o anterior (-1).
    fn cycle(&mut self, step: i32) {
        let (old_t, old_i) = (self.params.shape_text.clone(), self.params.shape_image.clone());
        self.morph = cycle_scene(
            step,
            &self.store,
            &mut self.params,
            &mut self.scene_idx,
            self.panel.scene_smooth,
            self.panel.scene_transition_duration,
        );
        self.apply_scene_shape(&old_t, &old_i);
    }

    /// Índice de la primera entrada de la playlist cuya escena existe, desde
    /// `start` en la dirección `dir` (±1) con envoltura. `None` = ninguna válida.
    fn seq_find_valid(&self, start: usize, dir: i32) -> Option<usize> {
        let entries = &self.panel.seq_playlist.entries;
        let n = entries.len();
        if n == 0 {
            return None;
        }
        let mut i = start.min(n - 1) as i32;
        for _ in 0..n {
            let idx = i.rem_euclid(n as i32) as usize;
            if self.store.get(&entries[idx].scene).is_some() {
                return Some(idx);
            }
            i += dir;
        }
        None
    }

    /// Lanza la entrada `idx` de la playlist: carga su escena con la transición
    /// propia de la entrada (o la global) y reinicia el cronómetro. Igual que
    /// `LoadScene` pero desde el secuenciador.
    fn seq_launch(&mut self, idx: usize) {
        self.seq_idx = idx;
        self.seq_timer = 0.0;
        let Some(entry) = self.panel.seq_playlist.entries.get(idx).cloned() else {
            return;
        };
        let dur = entry.transition.unwrap_or(self.panel.scene_transition_duration);
        if let Some(pos) = self.store.scenes.iter().position(|s| s.name == entry.scene) {
            self.scene_idx = pos;
            let target = self.store.scenes[pos].params.clone();
            let (old_t, old_i) =
                (self.params.shape_text.clone(), self.params.shape_image.clone());
            self.morph = start_scene(&mut self.params, &target, self.panel.scene_smooth, dur);
            self.apply_scene_shape(&old_t, &old_i);
        }
    }

    /// Avanza el secuenciador `dt` segundos: al agotar la duración de la entrada
    /// actual, salta a la siguiente válida (o para al final si no hay bucle).
    fn seq_advance(&mut self, dt: f32) {
        if self.seq_state != SeqPlayback::Playing || self.panel.seq_playlist.entries.is_empty() {
            return;
        }
        self.seq_timer += dt;
        let n = self.panel.seq_playlist.entries.len();
        let cur = self.panel.seq_playlist.entries[self.seq_idx.min(n - 1)]
            .duration
            .max(0.1);
        if self.seq_timer < cur {
            return;
        }
        let next = self.seq_idx + 1;
        if next >= n && !self.panel.seq_playlist.loop_at_end {
            // Show terminado: parar (y cerrar la grabación si la arrancó el show).
            self.seq_state = SeqPlayback::Stopped;
            self.seq_idx = 0;
            self.seq_timer = 0.0;
            if self.panel.seq_playlist.start_on_record {
                if let Some(r) = self.recorder.take() {
                    self.finish_recording(r);
                }
            }
        } else if let Some(v) = self.seq_find_valid(next % n, 1) {
            self.seq_launch(v);
        } else {
            self.seq_state = SeqPlayback::Stopped;
        }
    }

    /// Recarga en caliente: si `scenes.json` / `playlist.json` / `shapes.json`
    /// cambiaron en disco (edición externa), los recarga sin reiniciar. Se
    /// ignoran las escrituras propias (que actualizan el mtime guardado al
    /// guardar), así solo reacciona a cambios de fuera.
    fn hot_reload(&mut self, dt: f32) {
        self.reload_timer += dt;
        if self.reload_timer < 0.5 {
            return;
        }
        self.reload_timer = 0.0;

        let sm = file_mtime(&shared::scenes_path());
        if sm != self.scenes_mtime {
            self.scenes_mtime = sm;
            self.store = SceneStore::load();
            self.scene_idx = self.scene_idx.min(self.store.scenes.len().saturating_sub(1));
            self.refresh_scenes();
            eprintln!("↻ scenes.json recargado ({} escenas).", self.store.scenes.len());
        }

        let pm = file_mtime(&shared::playlist_path());
        if pm != self.playlist_mtime {
            self.playlist_mtime = pm;
            self.panel.seq_playlist = Playlist::load();
            let n = self.panel.seq_playlist.entries.len();
            if n == 0 {
                self.seq_state = SeqPlayback::Stopped;
                self.seq_idx = 0;
            } else if self.seq_idx >= n {
                self.seq_idx = n - 1;
            }
            eprintln!("↻ playlist.json recargado ({n} entradas).");
        }

        let hm = file_mtime(&shared::shapes_path());
        if hm != self.shapes_mtime {
            self.shapes_mtime = hm;
            self.shape_store = ShapeStore::load();
            self.panel.saved_shapes = self.shape_store.shapes.clone();
            eprintln!("↻ shapes.json recargado ({} formas).", self.shape_store.shapes.len());
        }
    }

    /// Resuelve el subconjunto de eventos del panel que aplican al visor GPU;
    /// el resto (grabación, formas, lienzo, secuenciador, música, ventana
    /// aparte...) pertenece a la app CPU y se ignora.
    fn handle_event(&mut self, ev: PanelEvent) {
        match ev {
            PanelEvent::Step => {
                self.panel.paused = true;
                self.step_once = true;
            }
            PanelEvent::Clear => self.sim.reseed(&self.queue, 0, &mut rand::thread_rng()),
            PanelEvent::Fill(n) => {
                self.sim.reseed(&self.queue, n as u32, &mut rand::thread_rng())
            }
            PanelEvent::StartTransition(snap) => self.params.start_transition(snap),
            PanelEvent::MatrixBlend(snap) => self.params.start_matrix_blend(snap),
            PanelEvent::SetSpeed(v) => self.params.set_speed(v),
            PanelEvent::SaveScene(name) => {
                self.store.upsert(&name, self.params.settled());
                self.persist_scenes();
            }
            PanelEvent::LoadScene(name) => {
                if let Some(pos) = self.store.scenes.iter().position(|s| s.name == name) {
                    self.scene_idx = pos;
                    let target = self.store.scenes[pos].params.clone();
                    let (old_t, old_i) =
                        (self.params.shape_text.clone(), self.params.shape_image.clone());
                    self.morph = start_scene(
                        &mut self.params,
                        &target,
                        self.panel.scene_smooth,
                        self.panel.scene_transition_duration,
                    );
                    self.apply_scene_shape(&old_t, &old_i);
                }
            }
            PanelEvent::DeleteScene(name) => {
                self.store.remove(&name);
                self.persist_scenes();
            }
            PanelEvent::SetDefaultScene(name) => {
                self.store.set_default(&name);
                self.persist_scenes();
            }
            PanelEvent::NextScene => self.cycle(1),
            PanelEvent::PrevScene => self.cycle(-1),
            PanelEvent::HidePanel => self.panel_visible = false,
            PanelEvent::ToggleRecord => self.toggle_record(),
            PanelEvent::PickVideoDir => {
                if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                    self.panel.video_dir = dir.to_string_lossy().into_owned();
                }
            }
            PanelEvent::PickMusic => {
                if let Some(p) = rfd::FileDialog::new()
                    .add_filter("Audio", &["mp3", "wav", "flac", "ogg", "m4a", "aac", "opus"])
                    .pick_file()
                {
                    self.panel.music_path = p.to_string_lossy().into_owned();
                    // Analiza ya la pista elegida (en el CPU hay que pulsar
                    // "Analizar pista" a mano; aquí lo hacemos solo por comodidad).
                    self.music_rx_path = self.panel.music_path.clone();
                    self.music_rx = Some(shared::music::analyze_async(self.panel.music_path.clone()));
                }
            }
            PanelEvent::MusicAnalyze => {
                if self.panel.music_path.is_empty() {
                    eprintln!("Elige primero una pista de música (sección Grabación).");
                } else if self.music_rx.is_none() {
                    eprintln!("Analizando '{}'…", self.panel.music_path);
                    self.music_rx_path = self.panel.music_path.clone();
                    self.music_rx = Some(shared::music::analyze_async(self.panel.music_path.clone()));
                }
            }
            PanelEvent::MusicPreviewToggle => {
                if self.preview.take().is_none() && !self.panel.music_path.is_empty() {
                    self.preview = shared::music::Preview::start(&self.panel.music_path);
                    // La preescucha arranca la pista en 0: rebobinar beats.
                    self.beat_cursor = 0;
                    self.beat_count = 0;
                }
            }
            PanelEvent::FormText(t) => {
                self.params.shape_text = t;
                self.params.shape_image.clear();
                self.build_shape();
            }
            PanelEvent::FormImagePick => {
                if let Some(p) = rfd::FileDialog::new()
                    .add_filter("Imagen o vídeo", &[
                        "png", "jpg", "jpeg", "webp", "bmp", "gif", "mp4", "mov", "mkv", "webm",
                        "avi", "m4v",
                    ])
                    .pick_file()
                {
                    self.params.shape_image = p.to_string_lossy().into_owned();
                    self.params.shape_text.clear();
                    // El vídeo solo funciona con el efecto de color (mosaico +
                    // overlay animado); lo activamos solo.
                    if is_video_path(&self.params.shape_image) {
                        self.params.shape_photo_color = true;
                        self.params.shape_tint = false;
                    }
                    self.build_shape();
                }
            }
            PanelEvent::FormImagePath(p) => {
                self.params.shape_image = p;
                self.params.shape_text.clear();
                if is_video_path(&self.params.shape_image) {
                    self.params.shape_photo_color = true;
                    self.params.shape_tint = false;
                }
                self.build_shape();
            }
            PanelEvent::ReleaseShape => {
                self.params.shape_text.clear();
                self.params.shape_image.clear();
                if self.photo_loaded {
                    // Salida en reverso: primero fuera la imagen, luego soltar.
                    self.photo_releasing = true;
                    self.overlay_target = 0.0;
                } else {
                    self.shape.target = 0.0;
                }
            }
            PanelEvent::SaveShape => {
                // Guarda el descriptor activo con un nombre derivado (el texto,
                // o el nombre de fichero de la imagen). Sin forma activa, no-op.
                let (name, text, image) = if !self.params.shape_text.trim().is_empty() {
                    let t = self.params.shape_text.trim().to_string();
                    (t.clone(), t, String::new())
                } else if !self.params.shape_image.is_empty() {
                    let name = std::path::Path::new(&self.params.shape_image)
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "imagen".to_string());
                    (name, String::new(), self.params.shape_image.clone())
                } else {
                    eprintln!("No hay forma activa que guardar.");
                    return;
                };
                self.shape_store.upsert(&name, text, image);
                self.persist_shapes();
            }
            PanelEvent::ApplyShape(name) => {
                if let Some(s) = self.shape_store.get(&name) {
                    self.params.shape_text = s.text.clone();
                    self.params.shape_image = s.image.clone();
                    self.build_shape();
                }
            }
            PanelEvent::DeleteShape(name) => {
                self.shape_store.remove(&name);
                self.persist_shapes();
            }
            // --- Secuenciador (show) ---
            PanelEvent::SeqSetPlaylist(pl) => {
                self.panel.seq_playlist = pl;
                let n = self.panel.seq_playlist.entries.len();
                if n == 0 {
                    self.seq_state = SeqPlayback::Stopped;
                    self.seq_idx = 0;
                    self.seq_timer = 0.0;
                } else if self.seq_idx >= n {
                    self.seq_idx = n - 1;
                }
                if let Err(e) = self.panel.seq_playlist.save() {
                    eprintln!("No se pudo guardar playlist.json: {e}");
                }
                self.playlist_mtime = file_mtime(&shared::playlist_path());
            }
            PanelEvent::SeqPlay => match self.seq_state {
                SeqPlayback::Paused => self.seq_state = SeqPlayback::Playing,
                SeqPlayback::Stopped => {
                    if let Some(v) = self.seq_find_valid(0, 1) {
                        self.seq_state = SeqPlayback::Playing;
                        self.seq_launch(v);
                    }
                }
                SeqPlayback::Playing => {}
            },
            PanelEvent::SeqPause => {
                if self.seq_state == SeqPlayback::Playing {
                    self.seq_state = SeqPlayback::Paused;
                }
            }
            PanelEvent::SeqStop => {
                self.seq_state = SeqPlayback::Stopped;
                self.seq_idx = 0;
                self.seq_timer = 0.0;
            }
            PanelEvent::SeqNext => {
                if let Some(v) = self.seq_find_valid(self.seq_idx + 1, 1) {
                    self.seq_launch(v);
                }
            }
            PanelEvent::SeqPrev => {
                let start = self.seq_idx.saturating_sub(1);
                if let Some(v) = self.seq_find_valid(start, -1) {
                    self.seq_launch(v);
                }
            }
            PanelEvent::SeqJump(i) => {
                if self.panel.seq_playlist.entries.get(i).is_some() {
                    if self.seq_state == SeqPlayback::Stopped {
                        self.seq_state = SeqPlayback::Playing;
                    }
                    self.seq_launch(i);
                }
            }
            _ => {}
        }
    }

    fn key_pressed(&mut self, key: Key) {
        match key {
            Key::Named(NamedKey::Space) => self.panel.paused = !self.panel.paused,
            Key::Character(c) if c.eq_ignore_ascii_case("m") => {
                // Aleatorizar la matriz: el cruce es suave SIEMPRE.
                let snap = self.params.current_snapshot();
                self.params.randomize_matrix(&mut rand::thread_rng());
                self.params.start_matrix_blend(snap);
            }
            Key::Character(c) if c.eq_ignore_ascii_case("g") => {
                // Alternar grid ↔ naive (mismo comportamiento estadístico;
                // sirve para comparar corrección y rendimiento).
                self.sim.use_grid = !self.sim.use_grid;
            }
            Key::Character(c) if c.eq_ignore_ascii_case("h") => {
                self.panel_visible = !self.panel_visible;
            }
            Key::Character(c) if c.eq_ignore_ascii_case("r") => self.toggle_record(),
            Key::Character(c) if c.eq_ignore_ascii_case("n") => self.cycle(1),
            Key::Character(c) if c.eq_ignore_ascii_case("p") => self.cycle(-1),
            Key::Character(c) if c.eq_ignore_ascii_case("b") => {
                self.params.boundary = match self.params.boundary {
                    shared::Boundary::Wrap => shared::Boundary::Bounce,
                    shared::Boundary::Bounce => shared::Boundary::Wrap,
                };
            }
            Key::Character(c) if c.eq_ignore_ascii_case("u") => {
                // Anti-aglomeración: disolver bolas hiperdensas (on/off), para
                // comparar con/sin al vuelo.
                self.params.anti_clump = !self.params.anti_clump;
            }
            // --- Paridad de atajos con la app CPU (los que aplican al visor) ---
            // Paso a paso (pausa + avanza un frame).
            Key::Character(c) if c == "." => self.handle_event(PanelEvent::Step),
            // Vaciar todas las partículas.
            Key::Character(c) if c.eq_ignore_ascii_case("c") => {
                self.handle_event(PanelEvent::Clear)
            }
            // Llenar aleatoriamente (con la cantidad del panel).
            Key::Character(c) if c.eq_ignore_ascii_case("f") => {
                self.handle_event(PanelEvent::Fill(self.panel.fill_count.max(0) as usize))
            }
            // Soltar la forma/texto activo (disolución fluida).
            Key::Character(c) if c.eq_ignore_ascii_case("s") => {
                self.handle_event(PanelEvent::ReleaseShape)
            }
            // Atraer las zonas activas al centro (on/off).
            Key::Character(c) if c.eq_ignore_ascii_case("a") => {
                self.params.attract_active = !self.params.attract_active;
            }
            // Auto-aleatorizar la matriz cada X s (on/off).
            Key::Character(c) if c.eq_ignore_ascii_case("x") => {
                self.params.auto_randomize = !self.params.auto_randomize;
            }
            // Deriva lenta y gradual del color/atracción (on/off).
            Key::Character(c) if c.eq_ignore_ascii_case("v") => {
                self.params.gradual = !self.params.gradual;
            }
            // Estelas de movimiento (on/off).
            Key::Character(c) if c.eq_ignore_ascii_case("e") => {
                self.params.trails = !self.params.trails;
            }
            // Resplandor cinematográfico / bloom (on/off).
            Key::Character(c) if c.eq_ignore_ascii_case("y") => {
                self.params.bloom = !self.params.bloom;
            }
            // Formar el texto escrito en el panel (si lo hay).
            Key::Named(NamedKey::Enter) if !self.panel.shape_text.trim().is_empty() => {
                self.handle_event(PanelEvent::FormText(self.panel.shape_text.clone()))
            }
            // Velocidad: 1..9 = 10..90 %, 0 = 100 % (como en la app CPU),
            // y +/- para pasos de ±10 % (hasta 300 %). Va por el sistema de
            // velocidad suave (`set_speed`), como el slider del panel.
            Key::Character(c) if c.len() == 1 && c.chars().all(|ch| ch.is_ascii_digit()) => {
                let d = c.chars().next().unwrap().to_digit(10).unwrap();
                self.params
                    .set_speed(if d == 0 { 1.0 } else { d as f32 / 10.0 });
            }
            Key::Character(c) if c == "+" => {
                self.params
                    .set_speed((self.params.speed_target + 0.1).clamp(0.0, 3.0));
            }
            Key::Character(c) if c == "-" => {
                self.params
                    .set_speed((self.params.speed_target - 0.1).clamp(0.0, 3.0));
            }
            _ => {}
        }
    }

    fn render(&mut self) {
        // Hook de prueba: graba `ENJAMBRE_AUTOREC` segundos y sale (verifica la
        // grabación a la resolución elegida sin teclado). Sin efecto si no está.
        if let Some(left) = self.autorec_left {
            if self.recorder.is_none() {
                self.toggle_record();
            }
            if left == 0 {
                if let Some(r) = self.recorder.take() {
                    self.finish_recording(r);
                }
                std::process::exit(0);
            }
            self.autorec_left = Some(left - 1);
        }
        // dt real del frame para las transiciones (limitado por si hay un
        // parón). Grabando, cada frame = 1/60 s de vídeo: las transiciones y
        // el show salen exactos en el .mp4 aunque el volcado vaya más lento
        // que el tiempo real (misma convención que la app CPU).
        let now = Instant::now();
        let real_dt = (now - self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;
        let dt = if self.recorder.is_some() {
            1.0 / rec::REC_FPS as f32
        } else {
            real_dt
        };

        // Transición de escena en curso: interpola los números; el cruce de
        // interacción lo lleva el blend de `advance_transition`.
        let mut morph_done = false;
        if let Some(m) = self.morph.as_mut() {
            m.blend = (m.blend + dt / m.dur).min(1.0);
            let t = m.blend * m.blend * (3.0 - 2.0 * m.blend);
            self.params.lerp_scene_numeric(&m.from, &m.target, t);
            if m.blend >= 1.0 {
                self.params.smooth = m.target.smooth;
                self.params.transition_duration = m.target.transition_duration;
                self.params.color_transition_duration = m.target.color_transition_duration;
                self.params.speed_transition_duration = m.target.speed_transition_duration;
                morph_done = true;
            }
        }
        if morph_done {
            self.morph = None;
        }
        self.params.advance_transition(dt);
        self.params.advance_speed(dt);
        self.shape
            .advance(dt, self.params.shape_transition_duration);
        // Vídeo: arranque DIFERIDO — la reproducción empieza solo cuando la
        // imagen ya se formó del todo (overlay revelado), para que arranque
        // desde el primer fotograma justo al aparecer nítida.
        if self.photo_loaded
            && !self.photo_releasing
            && self.video.is_none()
            && self.overlay_reveal >= 0.99
        {
            if let Some(path) = self.video_pending.take() {
                // Grabando: anota el offset (frame actual) para muxear su audio.
                if let Some(r) = self.recorder.as_ref() {
                    self.rec_video = Some((path.clone(), r.frames));
                }
                self.video = VideoSource::open(&path, 720);
                self.video_frozen = false;
            }
        }
        // Avanza el vídeo con el `dt` del show (1/60 grabando) y sube el
        // fotograma actual a la textura de la foto (la leen el mosaico y el
        // overlay). Al soltar se congela en el frame visible, que es el que se
        // ve al desvanecerse el overlay en el reverso.
        if self.photo_releasing {
            self.video_frozen = true;
        }
        if !self.video_frozen && self.video.is_some() {
            let new = self.video.as_mut().and_then(|v| v.advance(dt));
            if let Some(bytes) = new {
                self.sim.update_photo_frame(&self.queue, &bytes);
            }
            // Reproducido una vez → salida en reverso automática.
            let ended = self.video.as_ref().map(|v| v.ended()).unwrap_or(false);
            if ended && !self.photo_releasing {
                self.photo_releasing = true;
                self.overlay_target = 0.0;
            }
        }
        // Entrada: la foto se funde encima SOLO cuando las partículas ya se
        // acomodaron (fase A casi completa).
        if self.photo_loaded && !self.photo_releasing && self.shape.blend >= 0.95 {
            self.overlay_target = 1.0;
        }
        let step = if self.params.shape_transition_duration > 1e-3 {
            dt / self.params.shape_transition_duration
        } else {
            1.0
        };
        if self.overlay_reveal < self.overlay_target {
            self.overlay_reveal = (self.overlay_reveal + step).min(self.overlay_target);
        } else if self.overlay_reveal > self.overlay_target {
            self.overlay_reveal = (self.overlay_reveal - step).max(self.overlay_target);
        }
        // Salida (reverso): primero se va la imagen (overlay→0, con la silueta
        // de partículas aún detrás); cuando ya no se ve, SE LIBERAN (shape→0).
        if self.photo_releasing && self.overlay_reveal <= 1e-3 {
            self.shape.target = 0.0;
        }
        // Cuando ya no queda ni mosaico ni overlay, olvida la foto.
        if self.photo_loaded && self.photo_releasing && self.shape.n == 0 && self.overlay_reveal <= 1e-4 {
            self.photo_loaded = false;
            self.photo_releasing = false;
            self.video = None;
            self.video_pending = None;
        }

        // Secuenciador (show): avanza la playlist con la duración por entrada.
        self.seq_advance(dt);
        // Auto-avance de escenas (slideshow): como en la app CPU, pero NO cuando
        // el secuenciador está reproduciendo (tiene prioridad el show).
        if self.panel.scene_autoplay
            && self.seq_state != SeqPlayback::Playing
            && self.morph.is_none()
            && self.store.scenes.len() > 1
        {
            self.autoplay_timer += dt;
            if self.autoplay_timer >= self.panel.scene_autoplay_interval.max(0.5) {
                self.autoplay_timer = 0.0;
                self.cycle(1);
            }
        }
        // Recarga en caliente de scenes/playlist/shapes si cambian en disco.
        self.hot_reload(dt);
        // Refleja el estado del secuenciador en el panel embebido.
        self.panel.seq_state = self.seq_state;
        self.panel.seq_idx = self.seq_idx;
        self.panel.seq_elapsed = self.seq_timer;

        // --- Panel egui (mismo `config_panel` que la app CPU) ---
        let raw = self.egui_state.take_egui_input(&self.window);
        let ctx = self.egui_state.egui_ctx().clone();
        let panel_visible = self.panel_visible;
        let mut events = Vec::new();
        let out = ctx.run(raw, |ctx| {
            if panel_visible {
                egui::SidePanel::left("panel")
                    .default_width(330.0)
                    .show(ctx, |ui| {
                        events = config_panel(ui, &mut self.params, &mut self.panel);
                    });
            }
        });
        self.egui_state
            .handle_platform_output(&self.window, out.platform_output);
        for ev in events {
            self.handle_event(ev);
        }
        let paused = self.panel.paused && !self.step_once;
        self.step_once = false;

        // --- Audio en vivo + sincronía con la pista (port del bucle CPU) ---
        // Resultado del análisis de la música, si hay uno en marcha.
        if let Some(rx) = &self.music_rx {
            match rx.try_recv() {
                Ok(Ok(a)) => {
                    eprintln!(
                        "Música analizada: {} beats{} · {:.0} s",
                        a.onsets.len(),
                        a.bpm.map(|b| format!(" · ~{b:.0} BPM")).unwrap_or_default(),
                        a.duration
                    );
                    self.music = Some(a);
                    self.music_path_analyzed = self.music_rx_path.clone();
                    self.music_rx = None;
                    self.beat_cursor = 0;
                    self.beat_count = 0;
                }
                Ok(Err(e)) => {
                    eprintln!("No se pudo analizar la música: {e}");
                    self.music_rx = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.music_rx = None,
            }
        }
        // Preescucha que terminó por sí sola (fin de la pista).
        if self.preview.as_mut().is_some_and(|p| p.finished()) {
            self.preview = None;
        }

        // Captura en vivo: arrancar/reiniciar si `audio_reactive` está activo y
        // (no hay captura o el usuario cambió de fuente); parar si se apagó.
        let want_capture = self.params.audio_reactive;
        if want_capture
            && (self.audio_in.is_none() || self.audio_source_active != self.params.audio_source)
        {
            self.audio_source_active = self.params.audio_source;
            self.audio_in = shared::audio::start(self.audio_source_active);
            if self.audio_in.is_none() {
                eprintln!("Audio: sin captura para esa fuente.");
            }
        }
        if !want_capture && self.audio_in.is_some() {
            self.audio_in = None;
        }

        // Reloj musical: grabando es el frame de vídeo (exacto por
        // construcción, el audio del .mp4 empieza en 0 con el frame 0); en
        // vivo, el tiempo de la preescucha menos la latencia configurada.
        // `None` = la sincronía no actúa este frame.
        let music_t: Option<f32> = if self.panel.music_sync.enabled
            && self.music.is_some()
            && self.music_path_analyzed == self.panel.music_path
        {
            if let Some(r) = &self.recorder {
                Some(r.frames as f32 / rec::REC_FPS as f32)
            } else {
                self.preview
                    .as_ref()
                    .map(|p| (p.elapsed() - self.panel.music_sync.latency_offset).max(0.0))
            }
        } else {
            None
        };

        // Nivel y bandas de audio en vivo, suavizados (ataque rápido, caída
        // lenta). Se calculan cada frame, aun en pausa (afectan al render).
        let audio_raw = self.audio_in.as_ref().map(|a| a.level()).unwrap_or(0.0);
        let audio_goal = (audio_raw * 6.0).clamp(0.0, 1.0);
        let k = if audio_goal > self.audio_level { 0.5 } else { 0.08 };
        self.audio_level += (audio_goal - self.audio_level) * k;

        let bands_raw = self.audio_in.as_ref().map(|a| a.bands()).unwrap_or([0.0; 3]);
        for (raw, live) in bands_raw.iter().zip(self.audio_bands_live.iter_mut()) {
            let goal = (raw * 8.0).clamp(0.0, 1.0);
            let k = if goal > *live { 0.5 } else { 0.08 };
            *live += (goal - *live) * k;
        }

        // Con sincronía activa, la envolvente/bandas analizadas de la pista
        // sustituyen a la captura en vivo (mismo objetivo/intensidad de audio).
        let env_drive = music_t.is_some() && self.panel.music_sync.envelope_drive;
        let mut bands = self.audio_bands_live;
        if let (Some(t), Some(m)) = (music_t, &self.music) {
            if env_drive {
                self.audio_level = m.envelope_at(t);
            }
            if self.params.audio_bands {
                bands = m.bands_at(t);
            }
        }

        // Beats: al cruzar cada onset, dispara la acción configurada cada
        // `beat_divisor` golpes. El pulso decae en ~0.2 s, la onda en ~0.25 s.
        self.beat_pulse *= (-dt / 0.2).exp();
        self.shock_pulse *= (-dt / 0.25).exp();
        if let Some(t) = music_t {
            let onsets_len = self.music.as_ref().map_or(0, |m| m.onsets.len());
            while self.beat_cursor < onsets_len
                && self.music.as_ref().unwrap().onsets[self.beat_cursor] <= t
            {
                self.beat_cursor += 1;
                self.beat_count += 1;
                if self.beat_count % self.panel.music_sync.beat_divisor.max(1) != 0 {
                    continue;
                }
                match self.panel.music_sync.beat_action {
                    BeatAction::None => {}
                    BeatAction::Pulse => self.beat_pulse = self.panel.music_sync.pulse_gain,
                    BeatAction::Shockwave => {
                        self.shock_pulse = self.panel.music_sync.pulse_gain * SHOCK_GAIN;
                    }
                    BeatAction::RandomizeMatrix => {
                        let snap = self.params.current_snapshot();
                        self.params.randomize_matrix(&mut rand::thread_rng());
                        self.params.start_matrix_blend(snap);
                    }
                    BeatAction::NextScene => {
                        if self.seq_state == SeqPlayback::Playing {
                            // El beat fuerza el paso a la siguiente entrada del
                            // show (el cronómetro se reinicia en seq_launch).
                            let n = self.panel.seq_playlist.entries.len();
                            if n > 0 {
                                if let Some(v) = self.seq_find_valid((self.seq_idx + 1) % n, 1) {
                                    self.seq_launch(v);
                                }
                            }
                        } else {
                            self.cycle(1);
                        }
                    }
                }
            }
        }

        // La modulación de audio actúa con la captura en vivo, con la
        // envolvente de la pista o mientras quede pulso de beat vivo.
        let audio_mod_on = self.params.audio_reactive || env_drive || self.beat_pulse > 1e-3;
        let mut audio_gain = 1.0;
        if self.params.audio_reactive || env_drive {
            audio_gain += self.audio_level * self.params.audio_intensity;
        }
        audio_gain += self.beat_pulse;

        // Telemetría de la música para el panel (solo para mostrar).
        self.panel.music_analyzed =
            self.music.is_some() && self.music_path_analyzed == self.panel.music_path;
        self.panel.music_duration = self.music.as_ref().map_or(0.0, |m| m.duration);
        self.panel.music_onsets = self.music.as_ref().map_or(0, |m| m.onsets.len());
        self.panel.music_bpm = self.music.as_ref().and_then(|m| m.bpm);
        self.panel.music_previewing = self.preview.is_some();

        // Auto-aleatorizado de la matriz (modo Matriz), con cruce suave.
        if !paused
            && self.params.auto_randomize
            && self.params.mode == shared::InteractionMode::Matrix
        {
            self.auto_rand_timer += dt;
            if self.auto_rand_timer >= self.params.auto_randomize_interval.max(0.2) {
                self.auto_rand_timer = 0.0;
                let snap = self.params.current_snapshot();
                self.params.randomize_matrix(&mut rand::thread_rng());
                self.params.start_matrix_blend(snap);
            }
        }

        // Deriva gradual de la MATRIZ: muta los parámetros en la CPU (la del
        // color por partícula corre en la GPU, color.wgsl), igual que
        // `apply_dynamics` en la app CPU.
        if !paused && self.params.gradual {
            let ms = self.params.gradual_matrix_speed * self.params.time_scale.max(0.0);
            if ms > 0.0 {
                let mut rng = rand::thread_rng();
                for i in 0..NUM_COLORS {
                    for j in 0..NUM_COLORS {
                        self.params.matrix[i][j] = (self.params.matrix[i][j]
                            + rng.gen_range(-1.0f32..=1.0) * ms)
                            .clamp(-1.0, 1.0);
                    }
                }
            }
        }

        // Los parámetros (física + render) suben cada frame: los blends y lo
        // que haya tocado el panel se reflejan al instante. `dt` escala el
        // lerp del suavizado de color en el kernel.
        // Fase A: en modo foto forzamos fijación alta para que las reclutadas
        // se asienten en la rejilla. El fondo REPELE a la figura como con el
        // texto (el resto del enjambre choca/rodea la imagen, no la atraviesa).
        let fix = if self.photo_loaded { 0.9 } else { self.params.shape_strength };
        let drive = self.shape.drive(fix);
        // Color del mosaico: sigue la mezcla de aparición de la forma (las
        // reclutadas toman el color de la foto según se acomodan).
        let mb = self.shape.blend.clamp(0.0, 1.0);
        let mosaic = Mosaic {
            on: self.photo_loaded,
            reveal: mb * mb * (3.0 - 2.0 * mb),
            n: self.shape.n,
            center: [self.world[0] * 0.5, self.world[1] * 0.5],
            extent: self.photo_extent,
        };
        // Modulación de audio transitoria (velocidad/fuerza/brillo/tamaño/
        // resplandor): se aplica solo para esta subida y se restaura, para no
        // pisar el valor base ni las transiciones en curso (escena, velocidad).
        let saved = (
            self.params.time_scale,
            self.params.force,
            self.params.brightness,
            self.params.point_size,
            self.params.bloom_intensity,
        );
        if audio_mod_on {
            match self.params.audio_target {
                AudioTarget::Speed => self.params.time_scale *= audio_gain,
                AudioTarget::Force => self.params.force *= audio_gain,
                AudioTarget::Brightness => {
                    self.params.brightness = (saved.2 * audio_gain).min(1.0);
                }
                AudioTarget::Size => {
                    self.params.point_size = (saved.3 * audio_gain).min(80.0);
                }
                AudioTarget::Bloom => {
                    self.params.bloom_intensity = (saved.4 * audio_gain).min(4.0);
                }
            }
        }
        // "Bandas → colores": la ganancia (0 = apagado) solo cuando el efecto
        // está activo y hay una fuente de nivel (captura en vivo o envolvente).
        let audio_drive = AudioDrive {
            bands,
            bands_gain: if self.params.audio_bands && (self.params.audio_reactive || env_drive) {
                self.params.audio_intensity
            } else {
                0.0
            },
            shock: self.shock_pulse,
        };
        self.sim
            .upload_params(&self.queue, &self.params, &drive, &mosaic, &audio_drive, dt);
        self.params.time_scale = saved.0;
        self.params.force = saved.1;
        self.params.brightness = saved.2;
        self.params.point_size = saved.3;
        self.params.bloom_intensity = saved.4;

        // Fase B (superposición): mitad de la caja en NDC y opacidad (ease).
        let r = self.overlay_reveal.clamp(0.0, 1.0);
        let reveal = r * r * (3.0 - 2.0 * r);
        let half = [
            self.photo_extent[0] / self.world[0],
            self.photo_extent[1] / self.world[1],
        ];
        let draw_photo = self.photo_loaded && reveal > 0.001;

        let frame = match self.surface.get_current_texture() {
            Ok(f) => f,
            // Superficie perdida/obsoleta (resize, etc.): reconfigurar y seguir.
            Err(_) => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        self.sim.frame(&mut encoder, &view, paused);
        // La foto se funde encima de la escena (después de las partículas).
        if draw_photo {
            self.sim.draw_photo(&mut encoder, &self.queue, &view, false, half, reveal);
        }

        // Grabación: volcar la escena (sin el panel) a la textura de captura
        // y encolar la copia a staging del frame.
        if let Some(r) = self.recorder.as_ref() {
            self.sim.blit_to(&mut encoder, &r.view);
            if draw_photo {
                self.sim.draw_photo(&mut encoder, &self.queue, &r.view, true, half, reveal);
            }
            r.copy_frame(&mut encoder);
        }

        // Pintar el panel encima de la escena.
        let tris = ctx.tessellate(out.shapes, out.pixels_per_point);
        for (id, delta) in &out.textures_delta.set {
            self.egui_renderer
                .update_texture(&self.device, &self.queue, *id, delta);
        }
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.config.width, self.config.height],
            pixels_per_point: out.pixels_per_point,
        };
        self.egui_renderer
            .update_buffers(&self.device, &self.queue, &mut encoder, &tris, &screen);
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("egui"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                })
                .forget_lifetime();
            self.egui_renderer.render(&mut pass, &tris, &screen);
        }
        for id in &out.textures_delta.free {
            self.egui_renderer.free_texture(id);
        }

        self.queue.submit([encoder.finish()]);
        frame.present();

        // Grabación: mapear el frame recién copiado y volcar los que toquen a
        // ffmpeg. Si la tubería murió, cerramos la grabación.
        if let Some(mut r) = self.recorder.take() {
            match r.after_submit(&self.device) {
                Ok(()) => self.recorder = Some(r),
                Err(e) => {
                    eprintln!("Grabación detenida (error escribiendo a ffmpeg): {e}");
                    self.finish_recording(r);
                }
            }
        }

        // Telemetría del panel + FPS en el título.
        self.panel.particle_count = self.sim.count as usize;
        self.frames += 1;
        let elapsed = self.t0.elapsed().as_secs_f32();
        if elapsed >= 0.5 {
            let fps = self.frames as f32 / elapsed;
            self.panel.fps = fps.round() as i32;
            self.window.set_title(&format!(
                "Enjambre GPU · {} partículas · {fps:.0} FPS · {} · vel {:.0}%{}{}",
                self.sim.count,
                if self.sim.use_grid { "grid" } else { "naive" },
                self.params.time_scale * 100.0,
                if self.recorder.is_some() { " · ● REC" } else { "" },
                if self.panel.paused { " · PAUSA" } else { "" }
            ));
            self.frames = 0;
            self.t0 = Instant::now();
        }
    }
}

struct App {
    state: Option<State>,
    count: u32,
    /// Mundo (horizontal o vertical) y tamaño lógico de ventana, según la
    /// orientación elegida al arrancar.
    world: [f32; 2],
    win_size: (f64, f64),
    /// Calidad de grabación (lado menor en px: 1080 o 1440).
    rec_minor: u32,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.state.is_none() {
            let window = Arc::new(
                el.create_window(
                    Window::default_attributes()
                        .with_title("Enjambre GPU")
                        .with_inner_size(winit::dpi::LogicalSize::new(
                            self.win_size.0,
                            self.win_size.1,
                        )),
                )
                .expect("ventana"),
            );
            let mut state =
                pollster::block_on(State::new(window, self.count, self.world, self.rec_minor));
            // Si la escena predeterminada trae un mensaje/imagen, formarlo ya.
            if !state.params.shape_text.trim().is_empty() || !state.params.shape_image.is_empty()
            {
                state.build_shape();
            }
            self.state = Some(state);
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        // egui ve el evento primero (foco de teclado/ratón del panel); si lo
        // consume (p. ej. escribiendo un nombre de escena), no llega a la app.
        let consumed = state
            .egui_state
            .on_window_event(&state.window, &event)
            .consumed;
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::Resized(size) => state.resize(size.width, size.height),
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key,
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } if !consumed => match logical_key {
                Key::Named(NamedKey::Escape) => el.exit(),
                key => state.key_pressed(key),
            },
            WindowEvent::RedrawRequested => {
                state.render();
                // Animación continua: pedir el siguiente frame ya.
                state.window.request_redraw();
            }
            _ => {}
        }
    }
}

fn main() {
    // Argumentos (en cualquier orden): un número = nº de partículas; una palabra
    // de orientación = vertical (móvil/TikTok 9:16). Ej: `sim-gpu 40000 vertical`.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let count: u32 = args
        .iter()
        .find_map(|a| a.parse().ok())
        .unwrap_or(20_000);
    let lc: Vec<String> = args.iter().map(|a| a.to_ascii_lowercase()).collect();
    let has = |opts: &[&str]| lc.iter().any(|a| opts.contains(&a.as_str()));
    let vertical = has(&["vertical", "v", "-v", "--vertical", "movil", "móvil", "portrait"]);
    // Calidad de grabación (lado menor): 4K = 2160, 2K = 1440, si no Full HD 1080.
    let rec_minor = if has(&["4k", "2160", "2160p", "uhd"]) {
        2160
    } else if has(&["2k", "1440", "1440p", "qhd"]) {
        1440
    } else {
        1080
    };
    let (world, win_size) = if vertical {
        (WORLD_V, WIN_V)
    } else {
        (WORLD_H, WIN_H)
    };
    let (rw, rh) = rec_dims(world, rec_minor);
    eprintln!(
        "Enjambre GPU · {count} partículas · {} · graba {rw}×{rh} ({}) · Espacio pausa · . paso · \
         C vaciar · F llenar · S soltar · Enter formar texto · N/P escena · M aleatoriza · \
         X auto-aleatoriza · V gradual · E estelas · Y bloom · A atraer-centro · \
         U anti-aglomeración · B contorno · R graba · H panel · G grid/naive · 1..9/0 velocidad · \
         +/- ±10 % · Esc sale",
        if vertical { "VERTICAL 9:19.5 (móvil)" } else { "horizontal 16:10" },
        if rec_minor >= 2160 { "4K" } else if rec_minor >= 1440 { "2K" } else { "1080p" }
    );
    let event_loop = EventLoop::new().expect("bucle de eventos");
    event_loop
        .run_app(&mut App { state: None, count, world, win_size, rec_minor })
        .expect("bucle de la app");
}
