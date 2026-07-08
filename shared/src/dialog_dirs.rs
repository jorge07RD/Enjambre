//! Recuerda la última carpeta usada en cada tipo de diálogo nativo (`rfd`),
//! para no tener que volver a navegar cada vez que se elige un vídeo, una
//! imagen, la música o la carpeta de grabación. Persiste en disco junto a
//! `scenes.json`/`playlist.json`; lo comparten los tres binarios (`sim`,
//! `sim-gpu`, `panel`), que pueden abrir diálogos en procesos separados.

use rfd::FileDialog;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A qué tipo de diálogo pertenece la carpeta recordada. Cada uno se guarda
/// por separado (la carpeta de música no tiene por qué ser la de vídeos).
#[derive(Clone, Copy)]
pub enum DirKind {
    Video,
    Music,
    Image,
    Scenes,
}

#[derive(Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DialogDirs {
    pub video_dir: Option<PathBuf>,
    pub music_dir: Option<PathBuf>,
    pub image_dir: Option<PathBuf>,
    pub scenes_dir: Option<PathBuf>,
}

pub fn dialog_dirs_path() -> PathBuf {
    crate::scenes::scenes_path().with_file_name("dialog_dirs.json")
}

impl DialogDirs {
    pub fn load() -> Self {
        match std::fs::read(dialog_dirs_path()) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) {
        let path = dialog_dirs_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(bytes) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::write(&path, bytes);
        }
    }

    fn get(&self, kind: DirKind) -> Option<&PathBuf> {
        match kind {
            DirKind::Video => self.video_dir.as_ref(),
            DirKind::Music => self.music_dir.as_ref(),
            DirKind::Image => self.image_dir.as_ref(),
            DirKind::Scenes => self.scenes_dir.as_ref(),
        }
    }

    fn set(&mut self, kind: DirKind, dir: PathBuf) {
        match kind {
            DirKind::Video => self.video_dir = Some(dir),
            DirKind::Music => self.music_dir = Some(dir),
            DirKind::Image => self.image_dir = Some(dir),
            DirKind::Scenes => self.scenes_dir = Some(dir),
        }
        self.save();
    }
}

fn with_remembered(dirs: &DialogDirs, kind: DirKind) -> FileDialog {
    let dlg = FileDialog::new();
    match dirs.get(kind) {
        Some(d) if d.is_dir() => dlg.set_directory(d),
        _ => dlg,
    }
}

/// Elige una carpeta (p. ej. destino de grabación), recordando el resultado.
pub fn pick_folder(dirs: &mut DialogDirs, kind: DirKind) -> Option<PathBuf> {
    let picked = with_remembered(dirs, kind).pick_folder();
    if let Some(p) = &picked {
        dirs.set(kind, p.clone());
    }
    picked
}

/// Elige un archivo, recordando la carpeta que lo contiene. `exts` vacío =
/// sin filtro de extensión.
pub fn pick_file(dirs: &mut DialogDirs, kind: DirKind, filter_name: &str, exts: &[&str]) -> Option<PathBuf> {
    let mut dlg = with_remembered(dirs, kind);
    if !exts.is_empty() {
        dlg = dlg.add_filter(filter_name, exts);
    }
    let picked = dlg.pick_file();
    if let Some(p) = &picked {
        if let Some(parent) = p.parent() {
            dirs.set(kind, parent.to_path_buf());
        }
    }
    picked
}

/// Elige VARIOS archivos a la vez, recordando la carpeta del primero.
pub fn pick_files(
    dirs: &mut DialogDirs,
    kind: DirKind,
    filter_name: &str,
    exts: &[&str],
) -> Option<Vec<PathBuf>> {
    let mut dlg = with_remembered(dirs, kind);
    if !exts.is_empty() {
        dlg = dlg.add_filter(filter_name, exts);
    }
    let picked = dlg.pick_files();
    if let Some(paths) = &picked {
        if let Some(parent) = paths.first().and_then(|p| p.parent()) {
            dirs.set(kind, parent.to_path_buf());
        }
    }
    picked
}

/// Diálogo de "guardar como", recordando la carpeta elegida.
pub fn save_file(
    dirs: &mut DialogDirs,
    kind: DirKind,
    filter_name: &str,
    exts: &[&str],
    default_name: &str,
) -> Option<PathBuf> {
    let mut dlg = with_remembered(dirs, kind);
    if !exts.is_empty() {
        dlg = dlg.add_filter(filter_name, exts);
    }
    if !default_name.is_empty() {
        dlg = dlg.set_file_name(default_name);
    }
    let picked = dlg.save_file();
    if let Some(p) = &picked {
        if let Some(parent) = p.parent() {
            dirs.set(kind, parent.to_path_buf());
        }
    }
    picked
}
