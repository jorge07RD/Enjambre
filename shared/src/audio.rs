//! Captura de audio para la reactividad al sonido.
//!
//! En lugar de enlazar una librería de audio nativa (cpal/ALSA entra en
//! conflicto con la ALSA que ya usa macroquad: dos paquetes no pueden declarar
//! `links = "alsa"`), reutilizamos `ffmpeg` —que ya es dependencia para grabar—
//! para capturar el dispositivo de entrada por defecto (o el monitor del audio
//! del sistema) y volcar PCM `f32le` por su stdout. Un hilo lee ese flujo y
//! publica la amplitud (RMS) y, por bloque de 1024 muestras, la energía en
//! tres bandas de frecuencia (graves/medios/agudos) vía FFT, todo en atómicos.
//!
//! Requiere `ffmpeg` con soporte de entrada de audio (PulseAudio/PipeWire o
//! ALSA). Si no hay entrada, el nivel se queda en 0 y la opción no hace nada.

use crate::config::AudioSource;
use realfft::num_complex::Complex;
use realfft::RealFftPlanner;
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

/// Muestras por bloque de análisis (RMS + FFT). A 16 kHz, Δf = 15.625 Hz.
const BLOCK: usize = 1024;

/// Nivel de audio compartido (~0..1) guardado como bits de f32.
type Level = Arc<AtomicU32>;

/// Handle de captura. Mientras viva, `ffmpeg` sigue capturando; al soltarlo se
/// mata el proceso.
pub struct AudioIn {
    level: Level,
    bands: [Level; 3],
    child: Child,
}

impl AudioIn {
    /// Amplitud actual (RMS suavizado por bloques), típicamente 0..~0.5.
    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }

    /// Energía por banda del último bloque analizado: `[graves, medios, agudos]`.
    pub fn bands(&self) -> [f32; 3] {
        [
            f32::from_bits(self.bands[0].load(Ordering::Relaxed)),
            f32::from_bits(self.bands[1].load(Ordering::Relaxed)),
            f32::from_bits(self.bands[2].load(Ordering::Relaxed)),
        ]
    }
}

impl Drop for AudioIn {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Arranca la captura en vivo para la fuente pedida: micrófono (entrada por
/// defecto, probando PulseAudio/PipeWire y luego ALSA) o audio del sistema
/// (monitor del sink por defecto). `None` si no se pudo abrir ninguna.
pub fn start(source: AudioSource) -> Option<AudioIn> {
    match source {
        AudioSource::Mic => {
            for (fmt, dev) in [("pulse", "default"), ("alsa", "default")] {
                if let Some(a) = spawn(fmt, dev) {
                    return Some(a);
                }
            }
            None
        }
        AudioSource::System => {
            if let Some(mon) = monitor_source() {
                if let Some(a) = spawn("pulse", &mon) {
                    return Some(a);
                }
            }
            eprintln!("Audio: no encontré el monitor del sistema; probando el micrófono.");
            spawn("pulse", "default")
        }
    }
}

/// Nombre de la fuente "monitor" (loopback de la salida) del sink por defecto
/// de PulseAudio/PipeWire, vía `pactl`. `None` si no hay `pactl` o no hay
/// ninguna fuente `.monitor`.
fn monitor_source() -> Option<String> {
    let sink = Command::new("pactl")
        .args(["get-default-sink"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty());
    if let Some(sink) = sink {
        return Some(format!("{sink}.monitor"));
    }
    // Sin `get-default-sink` (versión vieja de pactl o sin PulseAudio):
    // primera fuente que termine en ".monitor" de la lista.
    let out = Command::new("pactl")
        .args(["list", "short", "sources"])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| line.split_whitespace().nth(1))
        .find(|name| name.ends_with(".monitor"))
        .map(str::to_string)
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

    // Si el backend/dispositivo elegido no existe, ffmpeg muere enseguida: lo
    // detectamos para pasar al siguiente candidato en vez de quedarnos con un
    // flujo muerto.
    std::thread::sleep(std::time::Duration::from_millis(150));
    if let Ok(Some(_)) = child.try_wait() {
        return None;
    }

    let mut out = child.stdout.take()?;
    let level: Level = Arc::new(AtomicU32::new(0));
    let bands: [Level; 3] = [
        Arc::new(AtomicU32::new(0)),
        Arc::new(AtomicU32::new(0)),
        Arc::new(AtomicU32::new(0)),
    ];
    let level_t = level.clone();
    let bands_t = bands.clone();
    std::thread::spawn(move || {
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(BLOCK);
        let mut fft_in = fft.make_input_vec();
        let mut fft_out = fft.make_output_vec();

        let mut buf = [0u8; 4096];
        let mut block = Vec::with_capacity(BLOCK);
        loop {
            match out.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(k) => {
                    for chunk in buf[..k - (k % 4)].chunks_exact(4) {
                        let v = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                        block.push(v);
                        if block.len() >= BLOCK {
                            let rms = (block.iter().map(|v| (*v as f64) * (*v as f64)).sum::<f64>()
                                / BLOCK as f64)
                                .sqrt() as f32;
                            level_t.store(rms.to_bits(), Ordering::Relaxed);

                            fft_in.copy_from_slice(&block);
                            if fft.process(&mut fft_in, &mut fft_out).is_ok() {
                                let amps = band_amps(&fft_out);
                                for (a, b) in amps.iter().zip(bands_t.iter()) {
                                    b.store(a.to_bits(), Ordering::Relaxed);
                                }
                            }
                            block.clear();
                        }
                    }
                }
            }
        }
    });
    Some(AudioIn { level, bands, child })
}

/// Amplitud por banda del espectro de un bloque de [`BLOCK`] muestras a
/// 16 kHz (Δf = 15.625 Hz): graves bins 1..16 (<250 Hz), medios 16..128
/// (250–2000 Hz), agudos 128..=512 (2000–8000 Hz, hasta Nyquist). Pura y
/// testeable.
fn band_amps(spec: &[Complex<f32>]) -> [f32; 3] {
    let amp = |range: std::ops::Range<usize>| -> f32 {
        let hi = range.end.min(spec.len());
        if range.start >= hi {
            return 0.0;
        }
        let sum_sq: f32 = spec[range.start..hi].iter().map(|c| c.norm_sqr()).sum();
        (sum_sq / (hi - range.start) as f32).sqrt() * 2.0 / BLOCK as f32
    };
    [amp(1..16), amp(16..128), amp(128..513)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    /// Genera el espectro de un seno de `freq` Hz sobre un bloque de [`BLOCK`]
    /// muestras a 16 kHz.
    fn spectrum_of_sine(freq: f32) -> Vec<Complex<f32>> {
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(BLOCK);
        let mut input = fft.make_input_vec();
        for (i, s) in input.iter_mut().enumerate() {
            *s = (TAU * freq * i as f32 / 16000.0).sin();
        }
        let mut out = fft.make_output_vec();
        fft.process(&mut input, &mut out).unwrap();
        out
    }

    #[test]
    fn banda_graves_domina_con_tono_bajo() {
        let spec = spectrum_of_sine(100.0);
        let [b, m, a] = band_amps(&spec);
        assert!(b > m && b > a, "graves={b} medios={m} agudos={a}");
    }

    #[test]
    fn banda_medios_domina_con_tono_medio() {
        let spec = spectrum_of_sine(1000.0);
        let [b, m, a] = band_amps(&spec);
        assert!(m > b && m > a, "graves={b} medios={m} agudos={a}");
    }

    #[test]
    fn banda_agudos_domina_con_tono_alto() {
        let spec = spectrum_of_sine(5000.0);
        let [b, m, a] = band_amps(&spec);
        assert!(a > b && a > m, "graves={b} medios={m} agudos={a}");
    }
}
