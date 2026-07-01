//! Panel de control en ventana del SO aparte (eframe). Es el **cliente** IPC:
//! se conecta al socket que abre el proceso `sim`, le envía los cambios de los
//! controles y muestra la telemetría que recibe. Al ser una ventana nativa,
//! Hyprland puede tilearla y redimensionarla por separado del lienzo.

use shared::ipc::{read_msg, socket_path, write_msg};
use shared::{config_panel, ControlMsg, ControlState, PanelEvent, PanelState, SimParams, TelemetryMsg};

use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::Arc;

fn main() -> eframe::Result<()> {
    let stream = connect_with_retry();

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([360.0, 760.0])
            .with_min_inner_size([300.0, 400.0])
            .with_title("Panel — Puntos de Atracción"),
        ..Default::default()
    };

    eframe::run_native(
        "puntos_atraccion_panel",
        options,
        Box::new(|_cc| Ok(Box::new(PanelApp::new(stream)))),
    )
}

/// Intenta conectar al socket del `sim` con unos reintentos (el `sim` puede
/// tardar un instante en crear el socket tras pulsar "Separar panel").
fn connect_with_retry() -> UnixStream {
    let path = socket_path();
    for _ in 0..50 {
        if let Ok(s) = UnixStream::connect(&path) {
            return s;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    eprintln!("No se pudo conectar al socket de la simulación: {path:?}");
    std::process::exit(1);
}

struct PanelApp {
    params: SimParams,
    st: PanelState,
    write_stream: UnixStream,
    /// Cola de telemetría recibida del sim. Es una cola (no un slot único) para
    /// no perder el `Init`, que llega una vez y queda enterrado bajo los `Stats`.
    tele_rx: Receiver<TelemetryMsg>,
    /// Lo marca el hilo lector (o un fallo de escritura) cuando el sim cierra;
    /// el bucle de UI lo ve y cierra la ventana ordenadamente.
    disconnected: Arc<AtomicBool>,
    /// Hasta recibir el `Init` no enviamos nada, para no pisar el estado del sim.
    initialized: bool,
}

impl PanelApp {
    fn new(stream: UnixStream) -> Self {
        // Hilo lector: vuelca la última telemetría; si el sim cierra, salimos.
        let read_stream = stream.try_clone().expect("clonar socket");
        let (tele_tx, tele_rx) = std::sync::mpsc::channel::<TelemetryMsg>();
        let disconnected = Arc::new(AtomicBool::new(false));
        let disconnected_t = disconnected.clone();
        std::thread::spawn(move || {
            let mut r = read_stream;
            loop {
                match read_msg::<TelemetryMsg, _>(&mut r) {
                    Ok(Some(m)) => {
                        if tele_tx.send(m).is_err() {
                            break;
                        }
                    }
                    Ok(None) | Err(_) => break,
                }
            }
            // La simulación se cerró: avisamos al bucle de UI para cerrar.
            disconnected_t.store(true, Ordering::SeqCst);
        });

        Self {
            params: SimParams::default(),
            st: PanelState {
                standalone: true,
                ..PanelState::default()
            },
            write_stream: stream,
            tele_rx,
            disconnected,
            initialized: false,
        }
    }

    /// Aplica TODA la telemetría pendiente del sim (drena la cola para no
    /// perder el `Init`).
    fn apply_telemetry(&mut self) {
        loop {
            let msg = match self.tele_rx.try_recv() {
                Ok(m) => m,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.disconnected.store(true, Ordering::SeqCst);
                    break;
                }
            };
            self.apply_one(msg);
        }
    }

    fn apply_one(&mut self, msg: TelemetryMsg) {
        match msg {
            TelemetryMsg::Init(state) => {
                let ControlState {
                    params,
                    paused,
                    canvas_size,
                    zoom_level,
                    brush,
                    brush_size,
                    active_color,
                    fill_count,
                } = *state;
                self.params = params;
                self.st.paused = paused;
                self.st.canvas_size = canvas_size;
                self.st.zoom_level = zoom_level;
                self.st.brush = brush;
                self.st.brush_size = brush_size;
                self.st.active_color = active_color;
                self.st.fill_count = fill_count;
                self.initialized = true;
            }
            TelemetryMsg::Stats {
                particle_count,
                fps,
                blend,
                time_scale,
                recording,
                matrix,
                ..
            } => {
                self.st.particle_count = particle_count;
                self.st.fps = fps;
                self.st.recording = recording;
                self.params.blend = blend;
                // Velocidad efectiva real, para mostrar el % en vivo (el sim es
                // quien conduce la transición de velocidad).
                self.params.time_scale = time_scale;
                // La matriz a la deriva (modo `gradual`) la manda el sim; solo
                // entonces la reflejamos, para no pisar las ediciones manuales.
                if self.params.gradual {
                    self.params.matrix = matrix;
                }
            }
        }
    }
}

impl eframe::App for PanelApp {
    fn update(&mut self, ctx: &eframe::egui::Context, _frame: &mut eframe::Frame) {
        // Si el sim cerró la conexión, cerramos la ventana ordenadamente.
        if self.disconnected.load(Ordering::SeqCst) {
            ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Close);
            return;
        }

        self.apply_telemetry();

        let mut events = Vec::new();
        eframe::egui::CentralPanel::default().show(ctx, |ui| {
            events = config_panel(ui, &mut self.params, &mut self.st);
        });

        if self.initialized {
            let state = ControlState {
                params: self.params.clone(),
                paused: self.st.paused,
                canvas_size: self.st.canvas_size,
                zoom_level: self.st.zoom_level,
                brush: self.st.brush,
                brush_size: self.st.brush_size,
                active_color: self.st.active_color,
                fill_count: self.st.fill_count,
            };
            let mut closing = false;
            if write_msg(&mut self.write_stream, &ControlMsg::State(state)).is_err() {
                closing = true;
            }
            for ev in events {
                if matches!(ev, PanelEvent::Reattach) {
                    closing = true;
                }
                let _ = write_msg(&mut self.write_stream, &ControlMsg::Event(ev));
            }
            if closing {
                ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Close);
                return;
            }
        }

        // Repintar de forma continua para que la telemetría (FPS, partículas,
        // transición) se vea en vivo.
        ctx.request_repaint();
    }
}
