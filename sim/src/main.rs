mod grid;
mod render;
mod simulation;

use egui_macroquad::egui;
use macroquad::prelude::*;
use ::rand::Rng;

use render::Renderer;
use shared::ipc::{read_msg, socket_path, write_msg};
use shared::{
    config_panel, hue_for_index, Brush, ControlMsg, ControlState, PanelEvent, PanelState, SimParams,
    TelemetryMsg,
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
        brush: st.brush,
        brush_size: st.brush_size,
        active_color: st.active_color,
        fill_count: st.fill_count,
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
        PanelEvent::Detach | PanelEvent::Reattach => {}
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

        // La velocidad transita de forma suave hacia su objetivo (aunque esté
        // en pausa, para que al reanudar ya esté en el valor pedido).
        params.advance_speed(get_frame_time());

        // Física.
        if !st.paused || step_once {
            sim.apply_dynamics(&mut params, &mut rng, get_frame_time());
            sim.step(&params);
            params.advance_transition(get_frame_time());
            step_once = false;
        }

        let mut want_pointer = false;
        let mut events: Vec<PanelEvent> = Vec::new();

        match mode {
            AppMode::Embedded => {
                st.standalone = false;
                st.particle_count = sim.particles.len();
                st.fps = get_fps();
                egui_macroquad::ui(|ctx| {
                    want_pointer = ctx.wants_pointer_input();
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
                        st.brush = state.brush;
                        st.brush_size = state.brush_size;
                        st.active_color = state.active_color;
                        st.fill_count = state.fill_count;
                        st.paused = state.paused;
                        // El zoom lo puede mover tanto el slider del panel como
                        // la rueda en esta ventana: solo adoptamos el del panel
                        // cuando cambia de verdad.
                        if (state.zoom_level - prev_incoming_zoom).abs() > 1e-6 {
                            st.zoom_level = state.zoom_level;
                        }
                        prev_incoming_zoom = state.zoom_level;

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
                        if params.gradual {
                            p.matrix = params.matrix;
                        }
                        params = p;
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
                    mode = AppMode::Embedded;
                    panel_was_connected = false;
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
                }
                let tele = TelemetryMsg::Stats {
                    particle_count: sim.particles.len(),
                    fps: get_fps(),
                    blend: params.blend,
                    time_scale: params.time_scale,
                    matrix: params.matrix,
                    canvas_size: st.canvas_size,
                    zoom_level: st.zoom_level,
                };
                if let Some(w) = ipc.writer.lock().unwrap().as_mut() {
                    let _ = write_msg(w, &tele);
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
        last_mouse = mouse;

        let camera = make_camera(st.zoom_level, pan_target);

        // Pintar/borrar (solo fuera del panel) usando coordenadas del mundo.
        if !want_pointer && is_mouse_button_down(MouseButton::Left) {
            let pos = camera.screen_to_world(mouse);
            match st.brush {
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
            }
        }

        // Render del mundo con la cámara, y luego el panel encima (si embebido).
        set_camera(&camera);
        renderer.draw(&sim, &params);
        // Borde del lienzo (grosor constante en pantalla).
        draw_rectangle_lines(
            0.0,
            0.0,
            world.x,
            world.y,
            2.0 / st.zoom_level,
            Color::new(0.3, 0.3, 0.35, 1.0),
        );
        set_default_camera();
        if mode == AppMode::Embedded {
            egui_macroquad::draw();
        }
        next_frame().await;
    }
}
