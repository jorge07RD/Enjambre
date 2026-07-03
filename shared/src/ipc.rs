//! Canal IPC entre `sim` (servidor) y `panel` (cliente).
//!
//! Transporte: socket de dominio Unix. Encuadre: longitud `u32` little-endian
//! seguida del cuerpo JSON (`serde_json`). Simple y depurable; suficiente para
//! la frecuencia de un panel de control.

use crate::config::{Brush, MusicSync, SimParams, Tool, NUM_COLORS};
use crate::panel_ui::PanelEvent;
use crate::playlist::{Playlist, SeqPlayback};
use crate::shapes::SavedShape;
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};
use std::path::PathBuf;

/// Versión del protocolo IPC. Sube cuando cambian los mensajes de forma
/// incompatible. El `sim` la anuncia (`TelemetryMsg::Version`) y el panel avisa
/// si no coincide (indicio de binarios de compilaciones distintas).
pub const IPC_VERSION: u32 = 1;

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
    /// Pista de música a mezclar en el vídeo grabado (vacío = sin audio).
    pub music_path: String,
    /// Si la transición entre escenas debe ser suave.
    pub scene_smooth: bool,
    /// Duración (s) de esa transición.
    pub scene_transition_duration: f32,
    /// Auto-avance (slideshow) entre escenas y su intervalo (s).
    pub scene_autoplay: bool,
    pub scene_autoplay_interval: f32,
    /// Sincronía con la música analizada (envolvente + beats).
    pub music_sync: MusicSync,
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
    /// Anuncio de versión del protocolo (se envía el primero al conectar). Un
    /// panel de otra versión lo usa para avisar; los binarios muy viejos que no
    /// conozcan esta variante simplemente la ignoran (ver `read_frame`).
    Version(u32),
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
    /// El `sim` fija la pausa en el panel. Se envía UNA vez cuando el usuario
    /// pulsa Espacio en la ventana del lienzo estando el panel separado (si no,
    /// el `State` del panel pisaría el cambio local). No es continuo, así que no
    /// interfiere con el botón de pausa del panel.
    SetPaused(bool),
    /// Playlist del secuenciador (el `sim` es su dueño). Solo se envía al
    /// conectar: los cambios posteriores nacen en el panel (`SeqSetPlaylist`),
    /// así que devolverlos en eco pisaría una edición en curso.
    SeqPlaylist(Playlist),
    /// Estado de reproducción del secuenciador (continuo, junto a `Stats`).
    SeqStatus {
        state: SeqPlayback,
        idx: usize,
        elapsed: f32,
    },
    /// Resultado del análisis de la pista de música (al conectar y cuando
    /// termina un análisis) y si la preescucha está sonando.
    MusicInfo {
        analyzed: bool,
        duration: f32,
        onsets: usize,
        bpm: Option<f32>,
        previewing: bool,
    },
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

/// Lee un frame crudo (longitud + cuerpo JSON) del transporte. `Ok(None)` = fin
/// de conexión. Un `Err` aquí es un fallo real del socket (no de decodificación),
/// así el llamante distingue "se cerró" de "no entiendo este mensaje".
pub fn read_frame<R: Read>(r: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut len_buf = [0u8; 4];
    match r.read_exact(&mut len_buf) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    Ok(Some(body))
}

/// Decodifica el cuerpo de un frame. `None` si no encaja con `T` (p. ej. una
/// variante que este binario no conoce): el llamante debe **ignorarlo y seguir**,
/// no cerrar la conexión (el encuadre por longitud mantiene el flujo alineado).
pub fn decode<T: for<'de> Deserialize<'de>>(body: &[u8]) -> Option<T> {
    serde_json::from_slice(body).ok()
}

/// Lee un mensaje con encuadre longitud+JSON. `Ok(None)` = fin de conexión.
/// Azúcar sobre `read_frame`+`decode`; los bucles que quieran tolerar mensajes
/// desconocidos deben usar directamente esas dos piezas.
pub fn read_msg<T: for<'de> Deserialize<'de>, R: Read>(r: &mut R) -> io::Result<Option<T>> {
    match read_frame(r)? {
        None => Ok(None),
        Some(body) => match serde_json::from_slice(&body) {
            Ok(msg) => Ok(Some(msg)),
            Err(e) => Err(io::Error::other(e)),
        },
    }
}
