//! Código compartido entre el proceso de simulación (`sim`, macroquad) y el
//! proceso del panel de control (`panel`, eframe): los parámetros de la
//! simulación, la UI egui del panel (para no duplicarla) y el canal IPC.

pub mod audio;
pub mod config;
pub mod dialog_dirs;
pub mod ipc;
pub mod music;
pub mod panel_ui;
pub mod playlist;
pub mod scenes;
pub mod shapes;
pub mod ui_theme;
pub mod video;

pub use config::*;
pub use dialog_dirs::{dialog_dirs_path, DialogDirs, DirKind};
pub use ipc::{ControlMsg, ControlState, TelemetryMsg};
pub use panel_ui::{config_panel, PanelEvent, PanelState};
pub use playlist::{playlist_path, Playlist, PlaylistEntry, SeqPlayback};
pub use scenes::{example_store, scenes_path, Scene, SceneStore};
pub use shapes::{shapes_path, SavedShape, ShapeStore};
pub use video::{is_video_path, VideoSource};

/// Serializa los tests que mutan `XDG_CONFIG_HOME` (variable de proceso
/// global): sin esto, correr en paralelo los hace pisarse los directorios.
#[cfg(test)]
pub(crate) static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
