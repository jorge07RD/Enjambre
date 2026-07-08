//! Arma la secuencia (playlist) real de Enjambre para el vídeo explicativo:
//! añade las escenas de ejemplo (`example_store`, sin tocar las que ya tenga
//! el usuario) y escribe una playlist que recorre Células → Cíclico →
//! Murmuración → Enjambres, con `start_on_record` para que al grabar
//! (`ENJAMBRE_AUTOREC` o tecla R) la secuencia arranque siempre desde el
//! principio. Hace backup de scenes.json/playlist.json antes de tocarlos
//! (mismo patrón `.bak-<epoch>` que ya usa la app).
//!
//! Uso: cargo run -q -p shared --example build_video_sequence
use shared::playlist::{Playlist, PlaylistEntry};
use shared::scenes::{example_store, scenes_path};
use shared::playlist::playlist_path;
use shared::SceneStore;

fn backup(path: &std::path::Path) {
    if path.exists() {
        let epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let bak = path.with_extension(format!("json.bak-{epoch}"));
        std::fs::copy(path, &bak).expect("backup");
        println!("Backup: {}", bak.display());
    }
}

fn main() {
    let scenes_p = scenes_path();
    let playlist_p = playlist_path();

    backup(&scenes_p);
    backup(&playlist_p);

    let mut store = SceneStore::load();
    let before = store.names().len();
    store.merge(example_store());
    println!(
        "Escenas: {} -> {} ({})",
        before,
        store.names().len(),
        store.names().join(", ")
    );
    store.save().expect("guardar scenes.json");

    let playlist = Playlist {
        entries: vec![
            PlaylistEntry { scene: "Células".into(), duration: 12.0, transition: Some(2.5) },
            PlaylistEntry { scene: "Cíclico".into(), duration: 12.0, transition: Some(2.5) },
            PlaylistEntry { scene: "Murmuración".into(), duration: 12.0, transition: Some(2.5) },
            PlaylistEntry { scene: "Enjambres".into(), duration: 10.0, transition: Some(2.5) },
        ],
        loop_at_end: true,
        start_on_record: true,
    };
    println!(
        "Playlist: {} paradas, {:.1}s por vuelta, loop_at_end={}, start_on_record={}",
        playlist.entries.len(),
        playlist.total_duration(),
        playlist.loop_at_end,
        playlist.start_on_record
    );
    playlist.save().expect("guardar playlist.json");
    println!("Listo.");
}
