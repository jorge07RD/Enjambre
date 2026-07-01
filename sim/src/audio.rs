//! Captura de audio para la reactividad al sonido.
//!
//! En lugar de enlazar una librería de audio nativa (cpal/ALSA entra en
//! conflicto con la ALSA que ya usa macroquad: dos paquetes no pueden declarar
//! `links = "alsa"`), reutilizamos `ffmpeg` —que ya es dependencia para grabar—
//! para capturar el dispositivo de entrada por defecto y volcar PCM `f32le` por
//! su stdout. Un hilo lee ese flujo y publica la amplitud (RMS) en un atómico.
//!
//! Requiere `ffmpeg` con soporte de entrada de audio (PulseAudio/PipeWire o
//! ALSA). Si no hay entrada, el nivel se queda en 0 y la opción no hace nada.

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

/// Nivel de audio compartido (RMS instantáneo, ~0..1) guardado como bits de f32.
type Level = Arc<AtomicU32>;

/// Handle de captura. Mientras viva, `ffmpeg` sigue capturando; al soltarlo se
/// mata el proceso.
pub struct AudioIn {
    level: Level,
    child: Child,
}

impl AudioIn {
    /// Amplitud actual (RMS suavizado por bloques), típicamente 0..~0.5.
    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }
}

impl Drop for AudioIn {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Arranca la captura probando primero PulseAudio/PipeWire y luego ALSA.
pub fn start() -> Option<AudioIn> {
    for (fmt, dev) in [("pulse", "default"), ("alsa", "default")] {
        if let Some(a) = spawn(fmt, dev) {
            return Some(a);
        }
    }
    None
}

fn spawn(fmt: &str, dev: &str) -> Option<AudioIn> {
    let mut child = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            fmt,
            "-i",
            dev,
            "-ac",
            "1",
            "-ar",
            "16000",
            "-f",
            "f32le",
            "-",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    // Si el backend elegido no existe, ffmpeg muere enseguida: lo detectamos para
    // pasar al siguiente candidato en vez de quedarnos con un flujo muerto.
    std::thread::sleep(std::time::Duration::from_millis(150));
    if let Ok(Some(_)) = child.try_wait() {
        return None;
    }

    let mut out = child.stdout.take()?;
    let level: Level = Arc::new(AtomicU32::new(0));
    let level_t = level.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        let mut acc = 0.0f64;
        let mut n = 0u32;
        loop {
            match out.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(k) => {
                    for chunk in buf[..k - (k % 4)].chunks_exact(4) {
                        let v = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) as f64;
                        acc += v * v;
                        n += 1;
                        if n >= 1024 {
                            let rms = (acc / n as f64).sqrt() as f32;
                            level_t.store(rms.to_bits(), Ordering::Relaxed);
                            acc = 0.0;
                            n = 0;
                        }
                    }
                }
            }
        }
    });
    Some(AudioIn { level, child })
}
