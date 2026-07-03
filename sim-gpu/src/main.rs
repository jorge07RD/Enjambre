//! Enjambre en GPU (experimental, hito 1): la física corre entera en compute
//! shaders (wgpu) y el render lee los buffers sin pasar por la CPU. No toca la
//! app principal (`sim`); comparte `shared` (parámetros y, más adelante,
//! escenas y panel).
//!
//! Uso: `cargo run --release -p sim-gpu [n_partículas]` (por defecto 20000).
//! Teclas: Espacio = pausa · M = aleatorizar la matriz · G = grid/naive ·
//! 1..9/0 = velocidad 10..90/100 % · +/- = ±10 % · Esc = salir.

mod gpu_sim;

use gpu_sim::GpuSim;
use shared::SimParams;
use std::sync::Arc;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

/// Mundo fijo (como el bench de la CPU); la ventana lo estira a su tamaño.
const WORLD: [f32; 2] = [1600.0, 1000.0];

struct State {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    sim: GpuSim,
    params: SimParams,
    paused: bool,
    // FPS en el título (media sobre una ventana de frames).
    frames: u32,
    t0: Instant,
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
        // Sin V-sync artificial más allá del presente por defecto (Fifo es lo
        // más compatible; el FPS del título mide el paso real).
        config.present_mode = wgpu::PresentMode::AutoVsync;
        surface.configure(&device, &config);

        // Parámetros iniciales: modo Matriz con reglas aleatorias (lo único que
        // entienden los kernels de los hitos 1-2).
        let mut rng = rand::thread_rng();
        let mut params = SimParams::default();
        params.randomize_matrix(&mut rng);
        let sim = GpuSim::new(&device, config.format, &params, WORLD, count, &mut rng);

        // Validar el counting sort del grid antes de fiar las fuerzas a él.
        match sim.validate_grid(&device, &queue) {
            Ok(()) => eprintln!("Grid GPU validado (prefix sum: total == {count})."),
            Err(e) => panic!("Validación del grid GPU fallida: {e}"),
        }

        State {
            window,
            surface,
            device,
            queue,
            config,
            sim,
            params,
            paused: false,
            frames: 0,
            t0: Instant::now(),
        }
    }

    /// Fija la escala de tiempo de la física (el `dt` del kernel) y la sube a
    /// la GPU. Sin transición suave: en la GPU el cambio es instantáneo.
    fn set_speed(&mut self, v: f32) {
        self.params.time_scale = v.clamp(0.0, 3.0);
        self.sim.upload_params(&self.queue, &self.params);
    }

    fn resize(&mut self, w: u32, h: u32) {
        if w > 0 && h > 0 {
            self.config.width = w;
            self.config.height = h;
            self.surface.configure(&self.device, &self.config);
        }
    }

    fn render(&mut self) {
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
        self.sim.frame(&mut encoder, &view, self.paused);
        self.queue.submit([encoder.finish()]);
        frame.present();

        self.frames += 1;
        let dt = self.t0.elapsed().as_secs_f32();
        if dt >= 0.5 {
            let fps = self.frames as f32 / dt;
            self.window.set_title(&format!(
                "Enjambre GPU · {} partículas · {fps:.0} FPS · {} · vel {:.0}%{}",
                self.sim.count,
                if self.sim.use_grid { "grid" } else { "naive" },
                self.params.time_scale * 100.0,
                if self.paused { " · PAUSA" } else { "" }
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
            self.state = Some(pollster::block_on(State::new(window, self.count)));
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
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
            } => match logical_key {
                Key::Named(NamedKey::Escape) => el.exit(),
                Key::Named(NamedKey::Space) => state.paused = !state.paused,
                Key::Character(c) if c.eq_ignore_ascii_case("m") => {
                    state.params.randomize_matrix(&mut rand::thread_rng());
                    state.sim.upload_params(&state.queue, &state.params);
                }
                Key::Character(c) if c.eq_ignore_ascii_case("g") => {
                    // Alternar grid ↔ naive (mismo comportamiento estadístico;
                    // sirve para comparar corrección y rendimiento).
                    state.sim.use_grid = !state.sim.use_grid;
                }
                // Velocidad: 1..9 = 10..90 %, 0 = 100 % (como en la app CPU),
                // y +/- para pasos de ±10 % (hasta 300 %).
                Key::Character(c) if c.len() == 1 && c.chars().all(|ch| ch.is_ascii_digit()) => {
                    let d = c.chars().next().unwrap().to_digit(10).unwrap();
                    state.set_speed(if d == 0 { 1.0 } else { d as f32 / 10.0 });
                }
                Key::Character(c) if c == "+" => {
                    state.set_speed(state.params.time_scale + 0.1);
                }
                Key::Character(c) if c == "-" => {
                    state.set_speed(state.params.time_scale - 0.1);
                }
                _ => {}
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
        "Enjambre GPU · {count} partículas · Espacio pausa · M aleatoriza · G grid/naive · \
         1..9/0 velocidad · +/- ±10 % · Esc sale"
    );
    let event_loop = EventLoop::new().expect("bucle de eventos");
    event_loop
        .run_app(&mut App { state: None, count })
        .expect("bucle de la app");
}
