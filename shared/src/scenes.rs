//! Escenas guardadas: instantáneas con nombre de toda la configuración
//! (`SimParams`) para reproducir un escenario y cambiar entre ellos. Se
//! persisten en JSON en el directorio de configuración del usuario.

use crate::config::SimParams;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Una escena = un nombre + una instantánea de los ajustes.
#[derive(Clone, Serialize, Deserialize)]
pub struct Scene {
    pub name: String,
    pub params: SimParams,
}

/// Colección de escenas persistida en disco, con la predeterminada marcada.
#[derive(Default, Clone, Serialize, Deserialize)]
pub struct SceneStore {
    /// Nombre de la escena que se carga al arrancar (si existe).
    pub default: Option<String>,
    pub scenes: Vec<Scene>,
}

/// Ruta del fichero de escenas: `$XDG_CONFIG_HOME/enjambre/scenes.json`
/// (con caída a `~/.config/...`).
pub fn scenes_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("enjambre").join("scenes.json")
}

impl SceneStore {
    /// Carga las escenas del disco; si no hay fichero o está corrupto, devuelve
    /// una colección vacía (no es un error de uso).
    pub fn load() -> SceneStore {
        let path = scenes_path();
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Err(_) => SceneStore::default(),
        }
    }

    /// Persiste al disco (creando el directorio si falta). Devuelve error de E/S.
    pub fn save(&self) -> std::io::Result<()> {
        let path = scenes_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let bytes = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(&path, bytes)
    }

    pub fn names(&self) -> Vec<String> {
        self.scenes.iter().map(|s| s.name.clone()).collect()
    }

    pub fn get(&self, name: &str) -> Option<&Scene> {
        self.scenes.iter().find(|s| s.name == name)
    }

    /// Inserta o reemplaza la escena `name` con `params` (ya asentados).
    pub fn upsert(&mut self, name: &str, params: SimParams) {
        if let Some(s) = self.scenes.iter_mut().find(|s| s.name == name) {
            s.params = params;
        } else {
            self.scenes.push(Scene {
                name: name.to_string(),
                params,
            });
        }
    }

    pub fn remove(&mut self, name: &str) {
        self.scenes.retain(|s| s.name != name);
        if self.default.as_deref() == Some(name) {
            self.default = None;
        }
    }

    pub fn set_default(&mut self, name: &str) {
        if self.get(name).is_some() {
            self.default = Some(name.to_string());
        }
    }

    /// Exporta toda la colección a un archivo JSON elegido por el usuario.
    pub fn export_to(&self, path: &std::path::Path) -> std::io::Result<()> {
        let bytes = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, bytes)
    }

    /// Importa una colección desde un archivo JSON.
    pub fn import_from(path: &std::path::Path) -> std::io::Result<SceneStore> {
        let bytes = std::fs::read(path)?;
        serde_json::from_slice(&bytes).map_err(std::io::Error::other)
    }

    /// Fusiona las escenas de `other` en esta colección (upsert por nombre). Si
    /// esta no tiene predeterminada, adopta la de `other`.
    pub fn merge(&mut self, other: SceneStore) {
        for s in other.scenes {
            self.upsert(&s.name, s.params);
        }
        if self.default.is_none() {
            self.default = other.default;
        }
    }
}

/// Colección de escenas de ejemplo para sembrar en el primer arranque, una por
/// modo de interacción, con parámetros ajustados para que luzcan distintas.
pub fn example_store() -> SceneStore {
    use crate::config::{BoidsScope, Boundary, InteractionMode, RenderStyle};
    let base = SimParams::default();

    let scene = |name: &str, p: SimParams| Scene {
        name: name.to_string(),
        params: p.settled(),
    };

    let enjambres = SimParams {
        mode: InteractionMode::SameColorOnly,
        same_repel_others: true,
        same_repel_strength: 0.45,
        force: 0.9,
        friction: 0.82,
        r_max: 85.0,
        ..base.clone()
    };
    let celulas = SimParams {
        mode: InteractionMode::Matrix,
        matrix: [
            [1.0, 0.3, -0.6, 0.2, 0.5, -0.4],
            [-0.5, 1.0, 0.4, -0.3, 0.2, 0.6],
            [0.6, -0.5, 1.0, 0.3, -0.4, 0.2],
            [0.2, 0.5, -0.6, 1.0, 0.3, -0.5],
            [-0.4, 0.3, 0.5, -0.6, 1.0, 0.4],
            [0.5, -0.4, 0.2, 0.4, -0.6, 1.0],
        ],
        force: 0.8,
        friction: 0.86,
        r_max: 90.0,
        style: RenderStyle::Glow,
        ..base.clone()
    };
    let cazadores = SimParams {
        mode: InteractionMode::PredatorPrey,
        force: 1.0,
        r_max: 110.0,
        friction: 0.88,
        point_size: 5.0,
        ..base.clone()
    };
    let ciclico = SimParams {
        mode: InteractionMode::Cyclic,
        force: 0.85,
        r_max: 95.0,
        friction: 0.85,
        ..base.clone()
    };
    let espuma = SimParams {
        mode: InteractionMode::SelfRepel,
        force: 0.7,
        r_max: 70.0,
        friction: 0.84,
        style: RenderStyle::SolidHalo,
        point_size: 5.0,
        ..base.clone()
    };
    let opuestos = SimParams {
        mode: InteractionMode::Opposite,
        force: 0.8,
        r_max: 100.0,
        friction: 0.86,
        ..base.clone()
    };
    let murmuracion = SimParams {
        mode: InteractionMode::Boids,
        boundary: Boundary::Wrap,
        boids_scope: BoidsScope::Hybrid,
        boids_separation: 1.6,
        boids_alignment: 1.1,
        boids_cohesion: 0.9,
        boids_sep_radius: 0.35,
        boids_cruise: 55.0,
        force: 1.0,
        friction: 0.82,
        r_max: 110.0,
        point_size: 3.5,
        ..base.clone()
    };

    SceneStore {
        default: Some("Enjambres".to_string()),
        scenes: vec![
            scene("Enjambres", enjambres),
            scene("Células", celulas),
            scene("Cazadores", cazadores),
            scene("Cíclico", ciclico),
            scene("Espuma", espuma),
            scene("Opuestos", opuestos),
            scene("Murmuración", murmuracion),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_default_and_persist_roundtrip() {
        let _env = crate::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Aísla el fichero en un directorio temporal para no tocar la config real.
        let dir = std::env::temp_dir().join(format!("enjambre_test_{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let _ = std::fs::remove_dir_all(&dir);

        let mut store = SceneStore::load();
        assert!(store.scenes.is_empty(), "arranca vacío");

        let mut a = SimParams::default();
        a.force = 2.5;
        store.upsert("caos", a.clone());
        store.upsert("caos", a.clone()); // upsert no duplica
        assert_eq!(store.scenes.len(), 1);

        store.set_default("caos");
        store.set_default("no-existe"); // ignorada
        assert_eq!(store.default.as_deref(), Some("caos"));
        store.save().unwrap();

        let reloaded = SceneStore::load();
        assert_eq!(reloaded.names(), vec!["caos".to_string()]);
        assert_eq!(reloaded.default.as_deref(), Some("caos"));
        assert!((reloaded.get("caos").unwrap().params.force - 2.5).abs() < 1e-6);

        let mut r2 = reloaded;
        r2.remove("caos");
        assert!(r2.scenes.is_empty() && r2.default.is_none(), "borrar limpia default");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
