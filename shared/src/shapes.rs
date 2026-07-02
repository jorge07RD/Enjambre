//! Biblioteca de formas/letras guardadas: descriptores ligeros (un texto o la
//! ruta de una imagen) con nombre, para tenerlos a mano y aplicarlos cuando se
//! quiera. Se persisten en JSON junto a las escenas. El `sim` es su dueño; el
//! panel recibe la lista por telemetría y pide aplicar/guardar/borrar.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Una forma guardada = nombre + descriptor. Si `text` no está vacío es una
/// letra/mensaje; en otro caso `image` es la ruta de una imagen.
#[derive(Clone, Serialize, Deserialize)]
pub struct SavedShape {
    pub name: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub image: String,
}

impl SavedShape {
    pub fn is_image(&self) -> bool {
        self.text.trim().is_empty() && !self.image.is_empty()
    }
}

/// Colección persistida de formas.
#[derive(Default, Clone, Serialize, Deserialize)]
pub struct ShapeStore {
    pub shapes: Vec<SavedShape>,
}

/// Ruta del fichero: `$XDG_CONFIG_HOME/enjambre/shapes.json`.
pub fn shapes_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("enjambre").join("shapes.json")
}

impl ShapeStore {
    pub fn load() -> ShapeStore {
        match std::fs::read(shapes_path()) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Err(_) => ShapeStore::default(),
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = shapes_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let bytes = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(&path, bytes)
    }

    /// Inserta o reemplaza la forma `name`.
    pub fn upsert(&mut self, name: &str, text: String, image: String) {
        if let Some(s) = self.shapes.iter_mut().find(|s| s.name == name) {
            s.text = text;
            s.image = image;
        } else {
            self.shapes.push(SavedShape {
                name: name.to_string(),
                text,
                image,
            });
        }
    }

    pub fn remove(&mut self, name: &str) {
        self.shapes.retain(|s| s.name != name);
    }

    pub fn get(&self, name: &str) -> Option<&SavedShape> {
        self.shapes.iter().find(|s| s.name == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_and_remove() {
        let mut store = ShapeStore::default();
        store.upsert("hola", "HOLA".into(), String::new());
        store.upsert("hola", "HOLA".into(), String::new()); // no duplica
        store.upsert("logo", String::new(), "/x/logo.png".into());
        assert_eq!(store.shapes.len(), 2);
        assert!(!store.get("hola").unwrap().is_image());
        assert!(store.get("logo").unwrap().is_image());
        store.remove("hola");
        assert_eq!(store.shapes.len(), 1);
    }
}
