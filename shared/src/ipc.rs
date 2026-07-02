//! Canal IPC entre `sim` (servidor) y `panel` (cliente).
//!
//! Transporte: socket de dominio Unix. Encuadre: longitud `u32` little-endian
//! seguida del cuerpo JSON (`serde_json`). Simple y depurable; suficiente para
//! la frecuencia de un panel de control.

use crate::config::{Brush, SimParams, Tool, NUM_COLORS};
use crate::panel_ui::PanelEvent;
use crate::shapes::SavedShape;
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};
use std::path::PathBuf;

/// Estado completo que el panel envía a la simulación (una vez por frame del
/// panel). La simulación adopta estos campos, salvo los que evoluciona ella
/// misma (ver `sim`: `blend`/`from_state` y la matriz mientras `gradual`).
///
/// `#[serde(default)]` a nivel de contenedor: si el `sim` y el `panel` provienen
/// de compilaciones distintas (campos añadidos), los que falten toman su valor
/// por defecto en vez de romper la conexión IPC (que dejaría el panel colgado).
#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ControlState {
    pub params: SimParams,
    pub paused: bool,
    pub canvas_size: f32,
    pub zoom_level: f32,
    pub tool: Tool,
    pub brush: Brush,
    pub brush_size: f32,
    pub active_color: usize,
    pub fill_count: i32,
    /// Carpeta donde guardar los vídeos (vacío = directorio de trabajo).
    pub video_dir: String,
    /// Si la transición entre escenas debe ser suave.
    pub scene_smooth: bool,
    /// Duración (s) de esa transición.
    pub scene_transition_duration: f32,
    /// Auto-avance (slideshow) entre escenas y su intervalo (s).
    pub scene_autoplay: bool,
    pub scene_autoplay_interval: f32,
}

/// Mensajes panel → sim.
#[derive(Clone, Serialize, Deserialize)]
pub enum ControlMsg {
    /// Estado de los controles (continuo).
    State(ControlState),
    /// Comando discreto disparado por un botón.
    Event(PanelEvent),
}

/// Mensajes sim → panel.
#[derive(Clone, Serialize, Deserialize)]
pub enum TelemetryMsg {
    /// Estado completo enviado una vez al conectar, para que el panel arranque
    /// sincronizado con la simulación (y no la pise con sus valores por defecto).
    Init(Box<ControlState>),
    /// Telemetría de solo lectura para mostrar (continuo).
    Stats {
        particle_count: usize,
        fps: i32,
        blend: f32,
        /// Velocidad efectiva actual (para mostrar el % real mientras transita).
        time_scale: f32,
        /// `true` mientras el `sim` está grabando vídeo.
        recording: bool,
        /// Estado del recuadro de encuadre (lo maneja el ratón en el `sim`).
        show_frame: bool,
        frame_preset: usize,
        /// Resolución de salida del preset actual (para el rótulo del panel).
        frame_w: u32,
        frame_h: u32,
        /// Matriz de atracción tal y como la ve la simulación (puede ir a la
        /// deriva con `gradual`), para que la grilla del panel la refleje.
        matrix: [[f32; NUM_COLORS]; NUM_COLORS],
        canvas_size: f32,
        zoom_level: f32,
    },
    /// Lista de escenas guardadas y la predeterminada (se envía al conectar y
    /// cada vez que cambia el almacén). El `sim` es el dueño de las escenas.
    ScenesList {
        names: Vec<String>,
        default: String,
    },
    /// El `sim` fija los parámetros del panel (tras cargar una escena o la
    /// predeterminada), para que el panel no reenvíe los ajustes anteriores.
    ApplyParams(Box<SimParams>),
    /// Biblioteca de formas/letras guardadas (se envía al conectar y cuando
    /// cambia). El `sim` es su dueño.
    ShapesList(Vec<SavedShape>),
}

/// Ruta del socket. Usa `$XDG_RUNTIME_DIR` (efímero por sesión) y cae a `/tmp`.
pub fn socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    dir.join("puntos_atraccion.sock")
}

/// Escribe un mensaje con encuadre longitud+JSON.
pub fn write_msg<T: Serialize, W: Write>(w: &mut W, msg: &T) -> io::Result<()> {
    let bytes = serde_json::to_vec(msg).map_err(io::Error::other)?;
    let len = bytes.len() as u32;
    w.write_all(&len.to_le_bytes())?;
    w.write_all(&bytes)?;
    w.flush()
}

/// Lee un mensaje con encuadre longitud+JSON. `Ok(None)` = fin de conexión.
pub fn read_msg<T: for<'de> Deserialize<'de>, R: Read>(r: &mut R) -> io::Result<Option<T>> {
    let mut len_buf = [0u8; 4];
    match r.read_exact(&mut len_buf) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    let msg = serde_json::from_slice(&body).map_err(io::Error::other)?;
    Ok(Some(msg))
}
