//! Código compartido entre el proceso de simulación (`sim`, macroquad) y el
//! proceso del panel de control (`panel`, eframe): los parámetros de la
//! simulación, la UI egui del panel (para no duplicarla) y el canal IPC.

pub mod config;
pub mod ipc;
pub mod panel_ui;
pub mod scenes;

pub use config::*;
pub use ipc::{ControlMsg, ControlState, TelemetryMsg};
pub use panel_ui::{config_panel, PanelEvent, PanelState};
pub use scenes::{example_store, scenes_path, Scene, SceneStore};
