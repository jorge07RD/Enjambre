//! Decodificación de vídeo por streaming vía `ffmpeg` como subproceso.
//!
//! Para el efecto foto con vídeo: en vez de una textura fija, alimentamos
//! fotogramas RGBA en el tiempo. Se lanza `ffmpeg` (debe estar en el `PATH`),
//! que escupe vídeo crudo `rgba` por su `stdout`; un hilo lee fotograma a
//! fotograma y los mete en una cola acotada (con contrapresión: si nadie
//! consume, `ffmpeg` se pausa). El consumidor avanza la reproducción con
//! `advance(dt)`, sacando fotogramas al ritmo de los FPS del vídeo pero
//! gobernado por el `dt` de la simulación. Así funciona igual en vivo (dt real)
//! que grabando (dt fijo 1/60): el vídeo avanza en tiempo de simulación, no de
//! reloj de pared, y el audio muxeado a posteriori cuadra con la imagen.
//!
//! Sin dependencias nativas de Rust y aguanta clips de cualquier duración
//! porque nunca carga el vídeo entero en memoria.

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, TryRecvError};
use std::sync::Arc;
use std::thread::JoinHandle;

/// Extensiones que tratamos como vídeo (el resto se decodifican como imagen
/// fija con el crate `image`/macroquad). El `gif` se deja como imagen (primer
/// fotograma) por simplicidad.
const VIDEO_EXTS: &[&str] = &["mp4", "mov", "mkv", "webm", "avi", "m4v"];

/// FPS por defecto si `ffprobe` no reporta uno válido.
const DEFAULT_FPS: f32 = 30.0;

/// Luminancia (BT.709) por debajo de la cual un píxel se considera "fondo
/// negro" y se vuelve transparente del todo con `key_out_black`.
const BLACK_KEY_THRESHOLD: f32 = 0.06;
/// Ancho de la rampa suave entre transparente y opaco por encima del umbral
/// (evita bordes duros/aliasing en los contornos del contenido).
const BLACK_KEY_SOFTNESS: f32 = 0.10;

/// Vuelve transparentes (alfa→0) los píxeles casi negros de un fotograma RGBA
/// `in-place`, con una transición suave entre `threshold` y
/// `threshold+softness` de luminancia. Pensado para vídeos con fondo negro
/// puro sin canal alfa real (p. ej. renders de Manim sin `-t`): al aplicarlo,
/// el fotograma se comporta como un PNG con fondo transparente, así que el
/// resto de la tubería (mosaico + overlay) deja ver las partículas donde no
/// hay contenido, sin más cambios.
pub fn key_out_black(rgba: &mut [u8], threshold: f32, softness: f32) {
    let t0 = threshold.clamp(0.0, 1.0);
    let t1 = (t0 + softness.max(0.0)).clamp(t0, 1.0);
    for px in rgba.chunks_exact_mut(4) {
        let r = px[0] as f32 / 255.0;
        let g = px[1] as f32 / 255.0;
        let b = px[2] as f32 / 255.0;
        let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        let k = if t1 > t0 {
            ((luma - t0) / (t1 - t0)).clamp(0.0, 1.0)
        } else if luma >= t1 {
            1.0
        } else {
            0.0
        };
        px[3] = ((px[3] as f32 / 255.0) * k * 255.0).round() as u8;
    }
}

/// `true` si la ruta apunta a un contenedor de vídeo (por extensión).
pub fn is_video_path(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| VIDEO_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Fuente de vídeo en streaming: mantiene vivo un `ffmpeg` que produce
/// fotogramas RGBA `w`×`h` UNA vez (sin bucle). El consumidor los saca con
/// `advance(dt)`; al agotarse la entrada, `ended()` pasa a `true`.
pub struct VideoSource {
    w: u32,
    h: u32,
    fps: f32,
    rx: Receiver<Vec<u8>>,
    stop: Arc<AtomicBool>,
    child: Child,
    _reader: JoinHandle<()>,
    /// Acumulador de tiempo para decidir cuántos fotogramas sacar por `dt`.
    acc: f32,
    /// El emisor se desconectó y la cola se vació: el vídeo terminó.
    finished: bool,
    /// Si está activo, cada fotograma pasa por `key_out_black` antes de
    /// entregarse (fondo negro → transparente, ver `advance`).
    key_black: bool,
}

impl VideoSource {
    /// Abre `path` con `ffmpeg`, escalando para que el lado mayor no supere
    /// `max_dim` (aspecto preservado, dimensiones pares). Se reproduce UNA vez.
    /// `key_black`: si es `true`, los píxeles casi negros de cada fotograma se
    /// vuelven transparentes (ver `key_out_black`), para dejar ver las
    /// partículas donde el vídeo no tiene contenido. `None` si `ffprobe`/
    /// `ffmpeg` fallan.
    pub fn open(path: &str, max_dim: u32, key_black: bool) -> Option<VideoSource> {
        let (nw, nh) = probe_dims(path)?;
        let (w, h) = scaled_even(nw, nh, max_dim.max(2));
        let fps = probe_fps(path).unwrap_or(DEFAULT_FPS);
        let frame_bytes = (w as usize) * (h as usize) * 4;

        // Sin `-re`: el pacing lo lleva el consumidor (`advance`). `ffmpeg`
        // decodifica tan rápido como se vacíe la cola (contrapresión del pipe).
        // `-an`: sin audio. Salida cruda RGBA por stdout.
        let mut child = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
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
        // Cola acotada: unos pocos fotogramas de holgura; el `send` bloquea
        // cuando está llena → `ffmpeg` se pausa (no decodifica de más).
        let (tx, rx) = sync_channel::<Vec<u8>>(8);
        let stop = Arc::new(AtomicBool::new(false));

        let reader = {
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                loop {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let mut buf = vec![0u8; frame_bytes];
                    // Un fotograma completo o se acabó (EOF / proceso muerto).
                    if stdout.read_exact(&mut buf).is_err() {
                        break;
                    }
                    // Bloquea si la cola está llena (contrapresión). Si el
                    // receptor se soltó, termina.
                    if tx.send(buf).is_err() {
                        break;
                    }
                }
                // Al salir se suelta `tx`: el receptor verá `Disconnected`.
            })
        };

        Some(VideoSource {
            w,
            h,
            fps,
            rx,
            stop,
            child,
            _reader: reader,
            acc: 0.0,
            finished: false,
            key_black,
        })
    }

    /// Decodifica SOLO el primer fotograma (RGBA) con las mismas dimensiones
    /// que produciría `open`. Para arrancar la textura/mosaico sin lanzar el
    /// streaming todavía. `key_black`: igual que en `open`, aplica
    /// `key_out_black` al fotograma antes de devolverlo. Devuelve
    /// `(bytes, w, h)`.
    pub fn decode_first_frame(path: &str, max_dim: u32, key_black: bool) -> Option<(Vec<u8>, u32, u32)> {
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
        let mut bytes = out.stdout;
        if key_black {
            key_out_black(&mut bytes, BLACK_KEY_THRESHOLD, BLACK_KEY_SOFTNESS);
        }
        Some((bytes, w, h))
    }

    /// Dimensiones (ya escaladas) del fotograma.
    pub fn dims(&self) -> (u32, u32) {
        (self.w, self.h)
    }

    /// `true` cuando el vídeo terminó de reproducirse (EOF) y la cola se vació.
    pub fn ended(&self) -> bool {
        self.finished
    }

    /// Avanza la reproducción `dt` segundos de tiempo de simulación y devuelve
    /// el fotograma que debe mostrarse ahora (el más nuevo de los que tocaba
    /// sacar), o `None` si no hay fotograma nuevo este paso. Si la entrada se
    /// agotó, marca `ended`.
    pub fn advance(&mut self, dt: f32) -> Option<Vec<u8>> {
        if self.finished {
            return None;
        }
        let spf = 1.0 / self.fps.max(1.0);
        self.acc += dt.max(0.0);
        // Acota el "catch-up" tras un parón (p.ej. el arranque del decodificador)
        // a unos pocos fotogramas: evita saltarse el principio del vídeo de golpe.
        self.acc = self.acc.min(spf * 4.0);
        let mut latest = None;
        while self.acc >= spf {
            match self.rx.try_recv() {
                Ok(frame) => {
                    latest = Some(frame);
                    self.acc -= spf;
                }
                // Aún no hay fotograma listo (decodificando): esperamos al
                // próximo paso sin gastar el acumulador.
                Err(TryRecvError::Empty) => break,
                // El hilo lector terminó y la cola está vacía: fin del vídeo.
                Err(TryRecvError::Disconnected) => {
                    self.finished = true;
                    break;
                }
            }
        }
        if self.key_black {
            if let Some(frame) = latest.as_mut() {
                key_out_black(frame, BLACK_KEY_THRESHOLD, BLACK_KEY_SOFTNESS);
            }
        }
        latest
    }
}

impl Drop for VideoSource {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// `true` si `path` tiene al menos una pista de audio (para no intentar muxear
/// el audio de un vídeo mudo).
pub fn has_audio(path: &str) -> bool {
    let out = Command::new("ffprobe")
        .args([
            "-v", "error", "-select_streams", "a", "-show_entries", "stream=index", "-of",
            "csv=p=0", path,
        ])
        .output();
    matches!(out, Ok(o) if o.status.success() && !o.stdout.is_empty())
}

/// Mezcla en el `.mp4` grabado (`recorded`) el audio del vídeo `src` empezando
/// en `offset_secs` (cuando el vídeo apareció en el show). Si el grabado ya
/// tiene audio (música), se mezclan (`amix`); si no, se añade el audio del
/// vídeo con silencio antes del offset. Reencoda solo el audio (`-c:v copy`).
/// No hace nada si `src` no tiene audio. Sobrescribe `recorded` al terminar.
pub fn overlay_audio(recorded: &str, src: &str, offset_secs: f32, mix_existing: bool) {
    if !has_audio(src) {
        return;
    }
    let delay_ms = (offset_secs.max(0.0) * 1000.0).round() as i64;
    let tmp = format!("{recorded}.mux.mp4");
    // Retrasa el audio del vídeo (input 1) al offset en que apareció y lo
    // RELLENA con silencio (`apad`) para que la pista más corta sea siempre el
    // vídeo grabado: así `-shortest` conserva TODA la grabación (si no, el
    // `.mp4` se cortaría justo al acabar el audio del vídeo). Con música (input
    // 0 con audio) los mezcla; `amix` con `duration=first` ya dura lo que el
    // audio grabado (= la grabación entera).
    let filter = if mix_existing {
        format!(
            "[1:a]adelay={delay_ms}:all=1[da];[0:a][da]amix=inputs=2:duration=first:normalize=0[a]"
        )
    } else {
        format!("[1:a]adelay={delay_ms}:all=1,apad[a]")
    };
    let status = Command::new("ffmpeg")
        .args([
            "-y", "-i", recorded, "-i", src, "-filter_complex", &filter, "-map", "0:v:0", "-map",
            "[a]", "-c:v", "copy", "-c:a", "aac", "-b:a", "192k", "-shortest", "-movflags",
            "+faststart", &tmp,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match status {
        Ok(s) if s.success() => {
            if let Err(e) = std::fs::rename(&tmp, recorded) {
                eprintln!("No pude reemplazar el vídeo con el audio muxeado: {e}");
                let _ = std::fs::remove_file(&tmp);
            } else {
                eprintln!("♪ Audio del vídeo añadido a la grabación.");
            }
        }
        _ => {
            eprintln!("No pude muxear el audio del vídeo en la grabación.");
            let _ = std::fs::remove_file(&tmp);
        }
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

/// FPS del stream de vídeo (`r_frame_rate`, p.ej. "30000/1001").
fn probe_fps(path: &str) -> Option<f32> {
    let out = Command::new("ffprobe")
        .args([
            "-v", "error", "-select_streams", "v:0", "-show_entries", "stream=r_frame_rate", "-of",
            "csv=p=0", path,
        ])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    let r = s.lines().next()?.trim();
    let fps = match r.split_once('/') {
        Some((n, d)) => n.trim().parse::<f32>().ok()? / d.trim().parse::<f32>().ok()?.max(1.0),
        None => r.parse::<f32>().ok()?,
    };
    if fps.is_finite() && fps > 1.0 {
        Some(fps)
    } else {
        None
    }
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
        // Primer fotograma one-shot.
        let (first, w, h) =
            VideoSource::decode_first_frame(&path, 720, false).expect("primer fotograma");
        assert!(w % 2 == 0 && h % 2 == 0);
        assert_eq!(first.len(), (w * h * 4) as usize);

        // Streaming: avanzando ~1 s de simulación deben salir fotogramas.
        let mut src = VideoSource::open(&path, 720, false).expect("abrir vídeo");
        assert_eq!(src.dims(), (w, h));
        let mut got = 0;
        let start = std::time::Instant::now();
        while got == 0 && start.elapsed() < std::time::Duration::from_secs(3) {
            if let Some(f) = src.advance(1.0 / 60.0) {
                assert_eq!(f.len(), (w * h * 4) as usize);
                got += 1;
            }
            std::thread::sleep(std::time::Duration::from_millis(8));
        }
        assert!(got > 0, "no salió ningún fotograma del streaming");
    }

    #[test]
    fn key_out_black_vuelve_transparente_el_negro_y_respeta_el_resto() {
        // Negro puro, blanco puro, y un gris a mitad de la rampa entre
        // BLACK_KEY_THRESHOLD y BLACK_KEY_SOFTNESS.
        let mid_luma = BLACK_KEY_THRESHOLD + BLACK_KEY_SOFTNESS * 0.5;
        let mid_byte = (mid_luma * 255.0).round() as u8;
        let mut rgba = vec![
            0, 0, 0, 255, // negro → transparente
            255, 255, 255, 255, // blanco → sin cambio
            mid_byte, mid_byte, mid_byte, 200, // gris a mitad de rampa, alfa original 200
        ];
        key_out_black(&mut rgba, BLACK_KEY_THRESHOLD, BLACK_KEY_SOFTNESS);
        assert_eq!(rgba[3], 0, "negro debe quedar totalmente transparente");
        assert_eq!(rgba[7], 255, "blanco debe conservar su alfa");
        assert!(
            rgba[11] > 0 && rgba[11] < 200,
            "gris a mitad de rampa debe quedar parcialmente transparente, fue {}",
            rgba[11]
        );
    }
}
