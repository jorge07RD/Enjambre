//! Validador de los ficheros de datos de Enjambre contra los tipos REALES de la
//! app (`SceneStore`/`Playlist`/`ShapeStore`), para autorarlos a mano sin riesgo
//! de corromperlos. Deserializa estrictamente y reporta línea/columna del error.
//!
//! Uso:
//!   cargo run -q -p shared --example validate_json -- scenes   ~/.config/enjambre/scenes.json
//!   cargo run -q -p shared --example validate_json -- playlist ~/.config/enjambre/playlist.json
//!   cargo run -q -p shared --example validate_json -- shapes   ~/.config/enjambre/shapes.json
//!
//! Además de validar el JSON, comprueba coherencia entre ficheros: que cada
//! escena de la playlist exista en `scenes.json` (si se pasa como 3er arg), y que
//! las rutas de imagen/vídeo de las formas existan en disco.

use shared::{Playlist, SceneStore, ShapeStore};
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let kind = args.get(1).map(String::as_str).unwrap_or("");
    let path = match args.get(2) {
        Some(p) => p.clone(),
        None => {
            eprintln!("uso: validate_json <scenes|playlist|shapes> <ruta> [scenes.json]");
            std::process::exit(2);
        }
    };
    let data = match std::fs::read_to_string(&path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("No pude leer '{path}': {e}");
            std::process::exit(2);
        }
    };

    let ok = match kind {
        "scenes" => match serde_json::from_str::<SceneStore>(&data) {
            Ok(s) => {
                println!(
                    "✔ scenes.json válido — {} escenas, predeterminada={:?}",
                    s.scenes.len(),
                    s.default
                );
                for sc in &s.scenes {
                    println!("  · {}", sc.name);
                }
                if let Some(def) = &s.default {
                    if !s.scenes.iter().any(|sc| &sc.name == def) {
                        eprintln!("⚠ la escena predeterminada '{def}' no existe en la lista");
                    }
                }
                true
            }
            Err(e) => {
                eprintln!("✗ scenes.json INVÁLIDO: {e}");
                false
            }
        },
        "playlist" => match serde_json::from_str::<Playlist>(&data) {
            Ok(p) => {
                println!(
                    "✔ playlist.json válido — {} entradas, loop={}, start_on_record={}",
                    p.entries.len(),
                    p.loop_at_end,
                    p.start_on_record
                );
                // Comprobación cruzada opcional contra scenes.json.
                if let Some(scenes_path) = args.get(3) {
                    if let Ok(sd) = std::fs::read_to_string(scenes_path) {
                        if let Ok(store) = serde_json::from_str::<SceneStore>(&sd) {
                            for e in &p.entries {
                                let exists = store.scenes.iter().any(|s| s.name == e.scene);
                                let mark = if exists { "·" } else { "⚠ NO EXISTE" };
                                println!("  {mark} {} ({}s)", e.scene, e.duration);
                            }
                        }
                    }
                } else {
                    for e in &p.entries {
                        println!("  · {} ({}s)", e.scene, e.duration);
                    }
                }
                true
            }
            Err(e) => {
                eprintln!("✗ playlist.json INVÁLIDO: {e}");
                false
            }
        },
        "shapes" => match serde_json::from_str::<ShapeStore>(&data) {
            Ok(s) => {
                println!("✔ shapes.json válido — {} formas", s.shapes.len());
                for sh in &s.shapes {
                    if sh.image.is_empty() {
                        println!("  · {} (texto: \"{}\")", sh.name, sh.text);
                    } else {
                        let ok = Path::new(&sh.image).exists();
                        println!(
                            "  · {} → {} {}",
                            sh.name,
                            sh.image,
                            if ok { "" } else { "⚠ FALTA EL ARCHIVO" }
                        );
                    }
                }
                true
            }
            Err(e) => {
                eprintln!("✗ shapes.json INVÁLIDO: {e}");
                false
            }
        },
        other => {
            eprintln!("tipo desconocido '{other}' (usa scenes|playlist|shapes)");
            false
        }
    };

    std::process::exit(if ok { 0 } else { 1 });
}
