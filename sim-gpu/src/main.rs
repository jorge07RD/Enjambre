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
//! Uso: `cargo run --release -p sim-gpu [n_partículas]` (por defecto 20000).
//! Teclas: Espacio = pausa · H = panel · R = grabar · M = aleatorizar la
//! matriz · U = anti-aglomeración · N/P = escena siguiente/anterior ·
//! B = contorno · G = grid/naive · 1..9/0 = velocidad 10..90/100 % ·
//! +/- = ±10 % · Esc = salir.

mod gpu_sim;
mod rec;
mod shape;

use gpu_sim::{GpuSim, Mosaic, ShapeDrive};
use rand::Rng;
use shared::{
    config_panel, example_store, hue_for_index, is_video_path, ui_theme, PanelEvent, PanelState,
    SceneStore, ShapeStore, SimParams, VideoSource, NUM_COLORS,
};
use std::sync::Arc;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

/// Mundo fijo (como el bench de la CPU); la ventana lo estira a su tamaño.
const WORLD: [f32; 2] = [1600.0, 1000.0];

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
    // Escenas: la misma biblioteca (`scenes.json`) que la app CPU. Aquí solo
    // se lee/escribe bajo demanda; el `sim` sigue siendo el dueño habitual.
    store: SceneStore,
    scene_idx: usize,
    morph: Option<SceneMorph>,
    autoplay_timer: f32,
    auto_rand_timer: f32,
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
    video_seq: u64,
    video_frozen: bool,
    recorder: Option<rec::Recorder>,
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
    async fn new(window: Arc<Window>, count: u32) -> State {
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
            WORLD,
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

        State {
            window,
            surface,
            device,
            queue,
            config,
            sim,
            params,
            store,
            scene_idx,
            morph: None,
            autoplay_timer: 0.0,
            auto_rand_timer: 0.0,
            shape_store,
            shape: ShapeState::default(),
            overlay_reveal: 0.0,
            overlay_target: 0.0,
            photo_extent: [WORLD[0], WORLD[1]],
            photo_loaded: false,
            photo_releasing: false,
            video_pending: None,
            video: None,
            video_seq: 0,
            video_frozen: false,
            recorder: None,
            panel,
            panel_visible: true,
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
                r.finish(&self.device);
                self.panel.recording = false;
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
            shape::text_to_points(&self.params.shape_text, WORLD, count, &mut rng)
        } else if !self.params.shape_image.is_empty() {
            shape::image_points_from_path(&self.params.shape_image, WORLD, count, &mut rng)
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
            match VideoSource::open(&self.params.shape_image, 720) {
                Some(v) => {
                    let (w, h) = v.dims();
                    let Some(first) = v.first_frame(std::time::Duration::from_secs(5)) else {
                        self.overlay_target = 0.0;
                        return;
                    };
                    self.video = Some(v);
                    self.video_seq = 0;
                    self.video_frozen = false;
                    (first, w, h)
                }
                None => {
                    self.overlay_target = 0.0;
                    return;
                }
            }
        } else {
            self.video = None;
            let Some(frame) = shape::decode_image_rgba(&self.params.shape_image) else {
                self.overlay_target = 0.0;
                return;
            };
            frame
        };
        self.sim.upload_photo(&self.device, &self.queue, &rgba, w, h);
        // Caja de la foto en mundo: 90% del lienzo, centrada, aspecto preservado.
        let (iw, ih) = (w as f32, h as f32);
        let scale = (WORLD[0] * 0.9 / iw).min(WORLD[1] * 0.9 / ih);
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
        let center = [WORLD[0] * 0.5, WORLD[1] * 0.5];
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

    /// Arranca o detiene la grabación de vídeo (tecla R o botón del panel).
    fn toggle_record(&mut self) {
        if let Some(r) = self.recorder.take() {
            r.finish(&self.device);
            self.panel.recording = false;
            return;
        }
        match rec::Recorder::start(
            &self.device,
            self.config.width,
            self.config.height,
            &self.panel.video_dir,
            &self.panel.music_path,
        ) {
            Ok(r) => {
                self.recorder = Some(r);
                self.panel.recording = true;
            }
            Err(e) => eprintln!("No se pudo iniciar la grabación (¿está ffmpeg?): {e}"),
        }
    }

    /// Refresca la lista de escenas que muestra el panel.
    fn refresh_scenes(&mut self) {
        self.panel.scenes = self.store.names();
        self.panel.default_scene = self.store.default.clone().unwrap_or_default();
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
                if let Err(e) = self.store.save() {
                    eprintln!("No se pudo guardar scenes.json: {e}");
                }
                self.refresh_scenes();
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
                if let Err(e) = self.store.save() {
                    eprintln!("No se pudo guardar scenes.json: {e}");
                }
                self.refresh_scenes();
            }
            PanelEvent::SetDefaultScene(name) => {
                self.store.set_default(&name);
                if let Err(e) = self.store.save() {
                    eprintln!("No se pudo guardar scenes.json: {e}");
                }
                self.refresh_scenes();
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
                if let Err(e) = self.shape_store.save() {
                    eprintln!("No se pudo guardar la forma '{name}': {e}");
                }
                self.panel.saved_shapes = self.shape_store.shapes.clone();
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
                if let Err(e) = self.shape_store.save() {
                    eprintln!("No se pudo borrar la forma '{name}': {e}");
                }
                self.panel.saved_shapes = self.shape_store.shapes.clone();
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
        // Vídeo: sube el fotograma más reciente a la textura de la foto (la leen
        // el mosaico y el overlay). Al soltar se congela en el frame visible,
        // que es el que se ve al desvanecerse el overlay en el reverso.
        if self.photo_releasing {
            self.video_frozen = true;
        }
        if !self.video_frozen && self.video.is_some() {
            let mut seq = self.video_seq;
            let new = self.video.as_ref().and_then(|v| v.poll(&mut seq));
            self.video_seq = seq;
            if let Some(bytes) = new {
                self.sim.update_photo_frame(&self.queue, &bytes);
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
        }

        // Auto-avance de escenas (slideshow), como en la app CPU.
        if self.panel.scene_autoplay && self.morph.is_none() && self.store.scenes.len() > 1 {
            self.autoplay_timer += dt;
            if self.autoplay_timer >= self.panel.scene_autoplay_interval.max(0.5) {
                self.autoplay_timer = 0.0;
                self.cycle(1);
            }
        }

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
            center: [WORLD[0] * 0.5, WORLD[1] * 0.5],
            extent: self.photo_extent,
        };
        self.sim
            .upload_params(&self.queue, &self.params, &drive, &mosaic, dt);

        // Fase B (superposición): mitad de la caja en NDC y opacidad (ease).
        let r = self.overlay_reveal.clamp(0.0, 1.0);
        let reveal = r * r * (3.0 - 2.0 * r);
        let half = [
            self.photo_extent[0] / WORLD[0],
            self.photo_extent[1] / WORLD[1],
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
                    r.finish(&self.device);
                    self.panel.recording = false;
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
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.state.is_none() {
            let window = Arc::new(
                el.create_window(
                    Window::default_attributes()
                        .with_title("Enjambre GPU")
                        .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 800.0)),
                )
                .expect("ventana"),
            );
            let mut state = pollster::block_on(State::new(window, self.count));
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
    let count: u32 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(20_000);
    eprintln!(
        "Enjambre GPU · {count} partículas · Espacio pausa · H panel · R graba · M aleatoriza · \
         U anti-aglomeración · N/P escena · B contorno · G grid/naive · 1..9/0 velocidad · \
         +/- ±10 % · Esc sale"
    );
    let event_loop = EventLoop::new().expect("bucle de eventos");
    event_loop
        .run_app(&mut App { state: None, count })
        .expect("bucle de la app");
}
