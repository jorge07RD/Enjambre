//! Secuenciador de escenas: una lista ordenada (playlist) de escenas con
//! duración propia por entrada, para montar "shows" reproducibles y grabables.
//! Se persiste en JSON junto a las escenas. Las entradas referencian la escena
//! por nombre: sobreviven a borrados (en reproducción se saltan las huérfanas).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Una parada del show: qué escena y cuánto tiempo (transición incluida).
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaylistEntry {
    /// Nombre de la escena en el `SceneStore`.
    pub scene: String,
    /// Segundos totales en esta entrada (la transición cuenta dentro), de modo
    /// que la duración del show = suma de duraciones.
    pub duration: f32,
    /// Duración de transición propia de esta entrada; `None` = usar la global
    /// del panel (`scene_transition_duration`).
    pub transition: Option<f32>,
}

impl Default for PlaylistEntry {
    fn default() -> Self {
        Self {
            scene: String::new(),
            duration: 10.0,
            transition: None,
        }
    }
}

/// La playlist completa más sus opciones de reproducción.
#[derive(Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Playlist {
    pub entries: Vec<PlaylistEntry>,
    /// Al terminar la última entrada: volver a la primera (si no, parar).
    pub loop_at_end: bool,
    /// Al empezar a grabar vídeo, rearrancar la secuencia desde el principio
    /// (y, sin `loop_at_end`, detener la grabación al terminarla).
    pub start_on_record: bool,
}

/// Estado de reproducción del secuenciador (viaja por IPC en la telemetría).
#[derive(Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub enum SeqPlayback {
    #[default]
    Stopped,
    Playing,
    Paused,
}

/// Ruta del fichero: `$XDG_CONFIG_HOME/enjambre/playlist.json` (hermano de
/// `scenes.json`; ver [`crate::scenes::scenes_path`]).
pub fn playlist_path() -> PathBuf {
    crate::scenes::scenes_path().with_file_name("playlist.json")
}

impl Playlist {
    /// Carga la playlist del disco; sin fichero o corrupto = vacía.
    pub fn load() -> Playlist {
        match std::fs::read(playlist_path()) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Err(_) => Playlist::default(),
        }
    }

    /// Persiste al disco (creando el directorio si falta).
    pub fn save(&self) -> std::io::Result<()> {
        let path = playlist_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let bytes = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(&path, bytes)
    }

    /// Duración total del show en segundos (suma de las entradas).
    pub fn total_duration(&self) -> f32 {
        self.entries.iter().map(|e| e.duration).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_persistencia() {
        // Aísla el fichero en un directorio temporal para no tocar la config real.
        let dir = std::env::temp_dir().join(format!("enjambre_pl_test_{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let _ = std::fs::remove_dir_all(&dir);

        let vacio = Playlist::load();
        assert!(vacio.entries.is_empty(), "arranca vacía");

        let pl = Playlist {
            entries: vec![
                PlaylistEntry { scene: "Enjambres".into(), duration: 5.0, transition: None },
                PlaylistEntry { scene: "Células".into(), duration: 8.0, transition: Some(1.5) },
            ],
            loop_at_end: true,
            start_on_record: true,
        };
        pl.save().unwrap();

        let re = Playlist::load();
        assert_eq!(re, pl);
        assert!((re.total_duration() - 13.0).abs() < 1e-6);
        assert_eq!(re.entries[1].transition, Some(1.5));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
