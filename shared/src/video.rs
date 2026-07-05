//! Decodificación de vídeo por streaming vía `ffmpeg` como subproceso.
//!
//! Para el efecto foto con vídeo: en vez de una textura fija, alimentamos
//! fotogramas RGBA en el tiempo. Se lanza `ffmpeg` (debe estar en el `PATH`),
//! que escupe vídeo crudo `rgba` por su `stdout`; un hilo lee fotograma a
//! fotograma y guarda siempre el más reciente. El render coge el último con
//! `poll` al ritmo que quiera (pacing por `-re` en tiempo real). Sin
//! dependencias nativas de Rust y aguanta clips de cualquier duración porque
//! nunca carga el vídeo entero en memoria (solo un fotograma).

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Extensiones que tratamos como vídeo (el resto se decodifican como imagen
/// fija con el crate `image`/macroquad). El `gif` se deja como imagen (primer
/// fotograma) por simplicidad.
const VIDEO_EXTS: &[&str] = &["mp4", "mov", "mkv", "webm", "avi", "m4v"];

/// `true` si la ruta apunta a un contenedor de vídeo (por extensión).
pub fn is_video_path(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| VIDEO_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// El fotograma más reciente decodificado, con un contador que se incrementa
/// en cada nuevo fotograma (para que el consumidor sepa si hay algo nuevo).
struct FrameSlot {
    seq: u64,
    bytes: Vec<u8>,
}

/// Fuente de vídeo en streaming: mantiene vivo un `ffmpeg` que produce
/// fotogramas RGBA `w`×`h` en tiempo real, UNA sola vez (sin bucle). Al
/// terminar de leer marca `ended`.
pub struct VideoSource {
    w: u32,
    h: u32,
    latest: Arc<Mutex<FrameSlot>>,
    stop: Arc<AtomicBool>,
    ended: Arc<AtomicBool>,
    child: Child,
    _reader: JoinHandle<()>,
}

impl VideoSource {
    /// Abre `path` con `ffmpeg`, escalando para que el lado mayor no supere
    /// `max_dim` (preservando aspecto, dimensiones pares). Reproduce UNA vez en
    /// tiempo real; al acabar, `ended()` pasa a `true`. `None` si
    /// `ffprobe`/`ffmpeg` fallan.
    pub fn open(path: &str, max_dim: u32) -> Option<VideoSource> {
        let (nw, nh) = probe_dims(path)?;
        let (w, h) = scaled_even(nw, nh, max_dim.max(2));
        let frame_bytes = (w as usize) * (h as usize) * 4;

        // `-re`: leer a la velocidad nativa (pacing en tiempo real por el pipe).
        // `-an`: sin audio. Salida cruda RGBA por stdout. Sin `-stream_loop`:
        // se reproduce una sola vez.
        let mut child = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-re",
                "-i",
                path,
                "-an",
                "-vf",
                &format!("scale={w}:{h}"),
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgba",
                "pipe:1",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| eprintln!("No pude lanzar ffmpeg para '{path}': {e}"))
            .ok()?;

        let mut stdout = child.stdout.take()?;
        let latest = Arc::new(Mutex::new(FrameSlot { seq: 0, bytes: Vec::new() }));
        let stop = Arc::new(AtomicBool::new(false));
        let ended = Arc::new(AtomicBool::new(false));

        let reader = {
            let latest = Arc::clone(&latest);
            let stop = Arc::clone(&stop);
            let ended = Arc::clone(&ended);
            std::thread::spawn(move || {
                let mut buf = vec![0u8; frame_bytes];
                while !stop.load(Ordering::Relaxed) {
                    // Un fotograma completo o se acabó (EOF / proceso muerto).
                    if stdout.read_exact(&mut buf).is_err() {
                        break;
                    }
                    let mut slot = latest.lock().unwrap();
                    slot.seq += 1;
                    slot.bytes.clear();
                    slot.bytes.extend_from_slice(&buf);
                }
                ended.store(true, Ordering::Relaxed);
            })
        };

        Some(VideoSource { w, h, latest, stop, ended, child, _reader: reader })
    }

    /// Decodifica SOLO el primer fotograma (RGBA) con las mismas dimensiones
    /// que produciría `open`. Para arrancar la textura/mosaico sin lanzar el
    /// streaming todavía. Devuelve `(bytes, w, h)`.
    pub fn decode_first_frame(path: &str, max_dim: u32) -> Option<(Vec<u8>, u32, u32)> {
        let (nw, nh) = probe_dims(path)?;
        let (w, h) = scaled_even(nw, nh, max_dim.max(2));
        let out = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-i",
                path,
                "-an",
                "-vf",
                &format!("scale={w}:{h}"),
                "-frames:v",
                "1",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgba",
                "pipe:1",
            ])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .map_err(|e| eprintln!("No pude decodificar el primer fotograma de '{path}': {e}"))
            .ok()?;
        if out.stdout.len() != (w as usize) * (h as usize) * 4 {
            eprintln!("Primer fotograma de '{path}' con tamaño inesperado.");
            return None;
        }
        Some((out.stdout, w, h))
    }

    /// Dimensiones (ya escaladas) del fotograma.
    pub fn dims(&self) -> (u32, u32) {
        (self.w, self.h)
    }

    /// `true` cuando el vídeo terminó de reproducirse (EOF) una vez.
    pub fn ended(&self) -> bool {
        self.ended.load(Ordering::Relaxed)
    }

    /// Bloquea hasta que llegue el primer fotograma (o expire `timeout`).
    /// Devuelve una copia de sus bytes RGBA.
    pub fn first_frame(&self, timeout: Duration) -> Option<Vec<u8>> {
        let start = Instant::now();
        loop {
            {
                let slot = self.latest.lock().unwrap();
                if slot.seq > 0 {
                    return Some(slot.bytes.clone());
                }
            }
            if start.elapsed() >= timeout {
                eprintln!("El primer fotograma de vídeo no llegó a tiempo.");
                return None;
            }
            std::thread::sleep(Duration::from_millis(8));
        }
    }

    /// Si hay un fotograma más nuevo que `*last_seq`, actualiza `*last_seq` y
    /// devuelve una copia de sus bytes RGBA; si no, `None`.
    pub fn poll(&self, last_seq: &mut u64) -> Option<Vec<u8>> {
        let slot = self.latest.lock().unwrap();
        if slot.seq != *last_seq && slot.seq > 0 {
            *last_seq = slot.seq;
            Some(slot.bytes.clone())
        } else {
            None
        }
    }
}

impl Drop for VideoSource {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Ancho/alto nativos del primer stream de vídeo vía `ffprobe`.
fn probe_dims(path: &str) -> Option<(u32, u32)> {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "csv=p=0:s=x",
            path,
        ])
        .output()
        .map_err(|e| eprintln!("No pude lanzar ffprobe para '{path}': {e}"))
        .ok()?;
    if !out.status.success() {
        eprintln!("ffprobe falló con '{path}' (¿no es un vídeo válido?)");
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    let line = s.lines().next()?.trim();
    let (w, h) = line.split_once('x')?;
    Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
}

/// Escala `(w,h)` para que el lado mayor no exceda `max_dim`, preservando el
/// aspecto y con dimensiones pares (yuv/scale lo requiere). Nunca amplía.
fn scaled_even(w: u32, h: u32, max_dim: u32) -> (u32, u32) {
    let (w, h) = (w.max(1), h.max(1));
    let longest = w.max(h);
    let (sw, sh) = if longest > max_dim {
        let s = max_dim as f32 / longest as f32;
        (((w as f32 * s).round() as u32).max(2), ((h as f32 * s).round() as u32).max(2))
    } else {
        (w, h)
    };
    (sw & !1, sh & !1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detecta_extensiones_de_video() {
        assert!(is_video_path("/x/clip.mp4"));
        assert!(is_video_path("CLIP.MOV"));
        assert!(!is_video_path("/x/foto.png"));
        assert!(!is_video_path("/x/sin_extension"));
    }

    #[test]
    fn escala_cap_lado_mayor_par() {
        // Vertical 1080x1920, tope 720 → lado mayor 720, ancho par proporcional.
        assert_eq!(scaled_even(1080, 1920, 720), (404, 720));
        // Ya pequeño: no amplía.
        assert_eq!(scaled_even(320, 200, 720), (320, 200));
        // Impares → se redondean a par.
        assert_eq!(scaled_even(101, 51, 720), (100, 50));
    }

    /// Test end-to-end del streaming real con `ffmpeg`. Solo corre si
    /// `ENJAMBRE_VIDEO_TEST` apunta a un vídeo (se salta en CI normal).
    #[test]
    fn streaming_real_produce_fotogramas() {
        let Ok(path) = std::env::var("ENJAMBRE_VIDEO_TEST") else {
            eprintln!("(saltado: define ENJAMBRE_VIDEO_TEST=/ruta/clip.mp4)");
            return;
        };
        let src = VideoSource::open(&path, 720).expect("abrir vídeo");
        let (w, h) = src.dims();
        assert!(w > 0 && h > 0 && w % 2 == 0 && h % 2 == 0);
        let first = src.first_frame(Duration::from_secs(5)).expect("primer fotograma");
        assert_eq!(first.len(), (w * h * 4) as usize);
        // Debe llegar al menos un fotograma más distinto (el vídeo avanza).
        let mut seq = 1;
        let start = Instant::now();
        let mut got_new = false;
        while start.elapsed() < Duration::from_secs(3) {
            if let Some(f) = src.poll(&mut seq) {
                assert_eq!(f.len(), (w * h * 4) as usize);
                got_new = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(16));
        }
        assert!(got_new, "no llegó ningún fotograma nuevo tras el primero");
    }
}
