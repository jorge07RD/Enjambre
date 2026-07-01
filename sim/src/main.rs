mod audio;
mod grid;
mod render;
mod simulation;

use egui_macroquad::egui;
use macroquad::prelude::*;
use ::rand::Rng;

use render::Renderer;
use shared::ipc::{read_msg, socket_path, write_msg};
use shared::{
    config_panel, example_store, hue_for_index, scenes_path, AudioTarget, Brush, ControlMsg,
    ControlState, InteractionMode, PanelEvent, PanelState, SceneStore, SimParams, TelemetryMsg,
    Tool, FRAME_PRESETS,
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
fn make_camera(zoom: f32, target: Vec2) -> Camera2D {
    let vw = screen_width() / zoom;
    let vh = screen_height() / zoom;
    Camera2D::from_display_rect(Rect::new(target.x - vw / 2.0, target.y - vh / 2.0, vw, vh))
}

// ----------------------------------------------------------------------------
// Grabación de vídeo vertical (TikTok): render offline a un `render_target` de
// 1080×1920 y volcado crudo (RGBA) a `ffmpeg` por stdin. Cada frame de la
// simulación es un frame del vídeo, así que el `.mp4` sale exacto a 120 fps
// aunque el volcado vaya más lento que el tiempo real.
// ----------------------------------------------------------------------------

const REC_FPS: i32 = 120;

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

/// Cámara que mapea el rectángulo de mundo del recuadro (centro + ancho/alto)
/// al `render_target`, de modo que la grabación capture exactamente esa zona.
fn record_camera(rt: &RenderTarget, center: Vec2, w: f32, h: f32) -> Camera2D {
    let mut cam =
        Camera2D::from_display_rect(Rect::new(center.x - w / 2.0, center.y - h / 2.0, w, h));
    cam.render_target = Some(rt.clone());
    cam
}

struct Recorder {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    rt: RenderTarget,
    frames: u32,
    path: String,
}

impl Recorder {
    /// Arranca `ffmpeg` y el destino de render a la resolución `w×h`, guardando
    /// en `dir` (o el directorio actual si está vacío). Falla si `ffmpeg` no está.
    fn start(w: u32, h: u32, dir: &str) -> std::io::Result<Recorder> {
        let rt = render_target(w, h);
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
        let mut child = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgba",
                "-s",
                &format!("{w}x{h}"),
                "-r",
                &REC_FPS.to_string(),
                "-i",
                "-",
                "-c:v",
                "libx264",
                "-preset",
                "medium",
                "-crf",
                "18",
                "-pix_fmt",
                "yuv420p",
                "-movflags",
                "+faststart",
                &path,
            ])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        let stdin = child.stdin.take().expect("stdin de ffmpeg");
        eprintln!("● Grabando en {path} ({w}×{h}, pulsa R para parar)");
        Ok(Recorder {
            child,
            stdin,
            rt,
            frames: 0,
            path,
        })
    }

    /// Lee los píxeles del `render_target` y los vuelca a `ffmpeg`. El texture
    /// de un render target se lee al revés en vertical, así que invertimos las
    /// filas para que el vídeo salga derecho.
    fn capture(&mut self) -> std::io::Result<()> {
        use std::io::Write;
        let img = self.rt.texture.get_texture_data();
        let w = img.width as usize;
        let h = img.height as usize;
        let stride = w * 4;
        for y in 0..h {
            let row = (h - 1 - y) * stride;
            self.stdin.write_all(&img.bytes[row..row + stride])?;
        }
        self.frames += 1;
        Ok(())
    }

    /// Cierra la tubería para que `ffmpeg` finalice el `.mp4` y espera a que
    /// termine de escribir.
    fn finish(self) {
        drop(self.stdin); // EOF -> ffmpeg cierra el fichero limpiamente
        let mut child = self.child;
        let _ = child.wait();
        eprintln!(
            "■ Vídeo guardado: {} ({} frames · {:.1}s a {REC_FPS} fps)",
            self.path,
            self.frames,
            self.frames as f32 / REC_FPS as f32
        );
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
                    match read_msg::<ControlMsg, _>(&mut reader) {
                        Ok(Some(ControlMsg::State(s))) => inbox_t.lock().unwrap().state = Some(s),
                        Ok(Some(ControlMsg::Event(e))) => inbox_t.lock().unwrap().events.push(e),
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
        scene_smooth: st.scene_smooth,
        scene_transition_duration: st.scene_transition_duration,
        scene_autoplay: st.scene_autoplay,
        scene_autoplay_interval: st.scene_autoplay_interval,
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
    rng: &mut impl Rng,
    step_once: &mut bool,
) {
    let aspect = screen_width() / screen_height();
    let world = Vec2::new(st.canvas_size * aspect, st.canvas_size);
    match ev {
        PanelEvent::Step => *step_once = true,
        PanelEvent::Clear => sim.clear(),
        PanelEvent::Fill(n) => sim.spawn_random(n, rng),
        PanelEvent::StartTransition(snap) => params.start_transition(snap),
        PanelEvent::SetSpeed(v) => params.set_speed(v),
        PanelEvent::FitCanvas => {
            st.zoom_level = (screen_width() / world.x)
                .min(screen_height() / world.y)
                .clamp(0.02, 30.0);
            *pan_target = world * 0.5;
        }
        PanelEvent::CanvasEqualsScreen => {
            // El mundo pasa a medir exactamente la ventana (1 unidad = 1 píxel),
            // así llena el lienzo sea cual sea el tamaño que dé el WM (Hyprland).
            st.canvas_size = screen_height();
            st.zoom_level = 1.0;
            *pan_target = Vec2::new(screen_width(), screen_height()) * 0.5;
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
        | PanelEvent::SaveScene(_)
        | PanelEvent::LoadScene(_)
        | PanelEvent::SetDefaultScene(_)
        | PanelEvent::DeleteScene(_)
        | PanelEvent::NextScene
        | PanelEvent::PrevScene
        | PanelEvent::ExportScenes
        | PanelEvent::ImportScenes => {}
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
    let mut rec: Option<Recorder> = None;
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
    // Captura de audio (mantener vivo el stream) + nivel suavizado 0..1.
    let audio_in = audio::start();
    if audio_in.is_none() {
        eprintln!("Audio: sin dispositivo de entrada; 'Reactivo al audio' no tendrá efecto.");
    }
    let mut audio_level = 0.0f32;
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
    let mut scenes_dirty = false;
    let mut current_scene_idx = 0usize;
    let mut scene_autoplay_timer = 0.0f32;
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

    loop {
        // El lienzo mantiene el aspecto de la ventana; su alto lo fija `st`.
        let aspect = screen_width() / screen_height();
        let world = Vec2::new(st.canvas_size * aspect, st.canvas_size);
        sim.world = world;
        // El recentrado de zonas activas tira hacia el centro de la vista.
        sim.focus = pan_target;

        // La velocidad transita de forma suave hacia su objetivo (aunque esté
        // en pausa, para que al reanudar ya esté en el valor pedido).
        params.advance_speed(get_frame_time());

        // Transición de escena en curso: interpola los números; el cruce de
        // interacción lo lleva el blend de `advance_transition` (más abajo).
        let mut morph_done = false;
        if let Some(m) = scene_morph.as_mut() {
            m.blend = (m.blend + get_frame_time() / m.dur).min(1.0);
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

        // Auto-avance de escenas (slideshow): cambia a la siguiente cada X s.
        if st.scene_autoplay && scene_morph.is_none() && store.scenes.len() > 1 {
            scene_autoplay_timer += get_frame_time();
            if scene_autoplay_timer >= st.scene_autoplay_interval.max(0.5) {
                scene_autoplay_timer = 0.0;
                scene_morph = cycle_scene(
                    1,
                    &store,
                    &mut params,
                    &mut current_scene_idx,
                    st.scene_smooth,
                    st.scene_transition_duration,
                );
                if scene_morph.is_none() && mode == AppMode::Detached {
                    pending_apply = Some(params.clone());
                }
            }
        } else if !st.scene_autoplay {
            scene_autoplay_timer = 0.0;
        }

        // Nivel de audio suavizado (ataque rápido, caída lenta) para la
        // reactividad. Se calcula cada frame, aun en pausa (afecta al brillo).
        let audio_raw = audio_in.as_ref().map(|a| a.level()).unwrap_or(0.0);
        let audio_goal = (audio_raw * 6.0).clamp(0.0, 1.0);
        let k = if audio_goal > audio_level { 0.5 } else { 0.08 };
        audio_level += (audio_goal - audio_level) * k;
        let audio_gain = if params.audio_reactive {
            1.0 + audio_level * params.audio_intensity
        } else {
            1.0
        };

        // Física.
        if !st.paused || step_once {
            // Auto-aleatorizado de la matriz cada X segundos (solo en modo Matriz).
            if params.auto_randomize && params.mode == InteractionMode::Matrix {
                auto_rng_timer += get_frame_time();
                if auto_rng_timer >= params.auto_randomize_interval.max(0.2) {
                    let snap = params.current_snapshot();
                    params.randomize_matrix(&mut rng);
                    params.start_transition(snap);
                    auto_rng_timer = 0.0;
                }
            }
            sim.apply_dynamics(&mut params, &mut rng, get_frame_time());
            // Modulación de audio transitoria sobre velocidad o fuerza: se aplica
            // solo para este `step` y se restaura, para no pisar el valor base ni
            // la transición de velocidad (`advance_transition`).
            let saved_ts = params.time_scale;
            let saved_force = params.force;
            if params.audio_reactive {
                match params.audio_target {
                    AudioTarget::Speed => params.time_scale *= audio_gain,
                    AudioTarget::Force => params.force *= audio_gain,
                    AudioTarget::Brightness => {}
                }
            }
            sim.step(&params);
            params.time_scale = saved_ts;
            params.force = saved_force;
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
                    want_pointer = ctx.wants_pointer_input();
                    want_keyboard = ctx.wants_keyboard_input();
                    egui::SidePanel::right("panel")
                        .default_width(310.0)
                        .show(ctx, |ui| {
                            events = config_panel(ui, &mut params, &mut st);
                        });
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
                        // La carpeta de guardado la elige el usuario en el panel.
                        video_dir = state.video_dir.clone();
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
                params.start_transition(snap);
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
            if is_key_pressed(KeyCode::A) {
                params.attract_active = !params.attract_active;
            }
            if is_key_pressed(KeyCode::N) {
                events.push(PanelEvent::NextScene);
            }
            if is_key_pressed(KeyCode::P) {
                events.push(PanelEvent::PrevScene);
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
                    Some(r) => r.finish(),
                    None => match Recorder::start(preset_w, preset_h, &video_dir) {
                        Ok(r) => rec = Some(r),
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
                    frame_height = screen_height() * 0.8 / st.zoom_level;
                    show_frame = true;
                }
                PanelEvent::PickVideoDir => {
                    if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                        video_dir = dir.to_string_lossy().into_owned();
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
                        scene_morph = start_scene(
                            &mut params,
                            &target,
                            st.scene_smooth,
                            st.scene_transition_duration,
                        );
                        // Carga instantánea: avisamos ya al panel; la suave, al
                        // terminar el morph (ver `morph_done`).
                        if scene_morph.is_none() && mode == AppMode::Detached {
                            pending_apply = Some(params.clone());
                        }
                    }
                }
                PanelEvent::NextScene => {
                    scene_morph = cycle_scene(
                        1,
                        &store,
                        &mut params,
                        &mut current_scene_idx,
                        st.scene_smooth,
                        st.scene_transition_duration,
                    );
                    if scene_morph.is_none() && mode == AppMode::Detached {
                        pending_apply = Some(params.clone());
                    }
                }
                PanelEvent::PrevScene => {
                    scene_morph = cycle_scene(
                        -1,
                        &store,
                        &mut params,
                        &mut current_scene_idx,
                        st.scene_smooth,
                        st.scene_transition_duration,
                    );
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
                other => apply_local_event(
                    other,
                    &mut sim,
                    &mut params,
                    &mut st,
                    &mut pan_target,
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
                    // Sincronización inicial: el panel adopta nuestro estado real.
                    let state = control_state(&params, &st);
                    if let Some(w) = ipc.writer.lock().unwrap().as_mut() {
                        let _ = write_msg(w, &TelemetryMsg::Init(Box::new(state)));
                    }
                    init_sent = true;
                    scenes_dirty = true; // envía la lista de escenas al panel nuevo
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
                if let Some(p) = pending_apply.take() {
                    if let Some(w) = ipc.writer.lock().unwrap().as_mut() {
                        let _ = write_msg(w, &TelemetryMsg::ApplyParams(Box::new(p)));
                    }
                }
            }
        }

        // --- Cámara: zoom y desplazamiento ---
        let mouse = Vec2::from(mouse_position());

        // Zoom con la rueda, hacia el cursor (mantiene fijo el punto bajo él).
        let wheel = mouse_wheel().1;
        if wheel != 0.0 && !want_pointer {
            let world_before = make_camera(st.zoom_level, pan_target).screen_to_world(mouse);
            let factor = if wheel > 0.0 { 1.15 } else { 1.0 / 1.15 };
            st.zoom_level = (st.zoom_level * factor).clamp(0.2, 30.0);
            let world_after = make_camera(st.zoom_level, pan_target).screen_to_world(mouse);
            pan_target += world_before - world_after;
        }

        // Desplazamiento arrastrando con el botón derecho o central.
        if is_mouse_button_down(MouseButton::Right) || is_mouse_button_down(MouseButton::Middle) {
            let cam = make_camera(st.zoom_level, pan_target);
            pan_target += cam.screen_to_world(last_mouse) - cam.screen_to_world(mouse);
        }

        // --- Edición del recuadro de encuadre con el botón izquierdo ---
        // (solo si está visible; si no se agarra, el izquierdo pinta como siempre).
        if show_frame && !want_pointer {
            let fcam = make_camera(st.zoom_level, pan_target);
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
                    .any(|c| (fcam.world_to_screen(*c) - mouse).length() < 14.0);
                let wm = fcam.screen_to_world(mouse);
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
                    let now = fcam.screen_to_world(mouse);
                    match drag {
                        FrameDrag::Move => {
                            frame_center += now - fcam.screen_to_world(last_mouse);
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

        let camera = make_camera(st.zoom_level, pan_target);

        // Herramienta del ratón (fuera del panel y si no movemos el recuadro):
        // Fuerza atrae/repele el enjambre; Pincel pinta o borra.
        sim.pointer = None;
        if frame_drag.is_none() && !want_pointer && is_mouse_button_down(MouseButton::Left) {
            let pos = camera.screen_to_world(mouse);
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

        // Modulación de audio sobre el brillo (transitoria durante el render y la
        // grabación; se restaura tras volcar el frame).
        let saved_brightness = params.brightness;
        if params.audio_reactive && params.audio_target == AudioTarget::Brightness {
            params.brightness = (params.brightness * audio_gain).min(1.0);
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
            let mut tcam = make_camera(st.zoom_level, pan_target);
            tcam.render_target = Some(rt.clone());
            set_camera(&tcam);
            // Desvanecido: rectángulo negro translúcido sobre el mundo visible.
            let tl = tcam.screen_to_world(vec2(0.0, 0.0));
            let br = tcam.screen_to_world(vec2(sw, sh));
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
                rec.take().unwrap().finish();
            }
        }
        // Restaurar el brillo base tras el render/grabación.
        params.brightness = saved_brightness;
        if let Some(r) = &rec {
            draw_text(
                &format!("● REC  {:.1}s", r.frames as f32 / REC_FPS as f32),
                20.0,
                40.0,
                34.0,
                RED,
            );
        }

        if mode == AppMode::Embedded {
            egui_macroquad::draw();
        }
        next_frame().await;
    }
}
