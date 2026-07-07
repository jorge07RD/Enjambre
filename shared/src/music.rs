//! Análisis offline de la pista de música y preescucha en vivo.
//!
//! La decodificación reutiliza `ffmpeg` (misma razón que en `audio.rs`: cpal y
//! macroquad no pueden convivir por ALSA): la pista se vuelca a PCM `f32` mono
//! a 44100 Hz y se analiza entera en memoria. Del análisis salen:
//!
//! - la **envolvente** de energía a exactamente [`ENV_RATE`] muestras/s (igual
//!   al fps del vídeo grabado, así `envelope[frame]` es el valor del frame k
//!   del `.mp4`, sin interpolar), y
//! - los **onsets/beats** por flujo espectral (suma de los incrementos de
//!   magnitud entre ventanas FFT consecutivas) con umbral adaptativo.
//!
//! La preescucha lanza `ffplay` (viene con ffmpeg) como proceso hijo; su reloj
//! es aproximado (arranque + latencia de audio), lo exacto es la grabación.

use realfft::RealFftPlanner;
use std::io::{self, Read};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Instant;

/// Muestras de envolvente/flujo por segundo. Debe coincidir con los fps del
/// vídeo grabado (`REC_FPS`) para que la indexación por frame sea directa.
pub const ENV_RATE: u32 = 60;
/// Frecuencia de muestreo a la que se decodifica la pista.
const SR: u32 = 44100;
/// Salto entre ventanas: `SR / ENV_RATE` = 735 muestras exactas.
const HOP: usize = (SR / ENV_RATE) as usize;
/// Tamaño de la ventana FFT (con solape, ya que HOP < WIN).
const WIN: usize = 2048;

/// Resultado del análisis de una pista.
pub struct MusicAnalysis {
    /// Duración total (s).
    pub duration: f32,
    /// Envolvente de energía (RMS) a [`ENV_RATE`] muestras/s, normalizada a
    /// ~0..1 (percentil 95, robusto a picos aislados).
    pub envelope: Vec<f32>,
    /// Envolventes por banda de frecuencia —`[graves, medios, agudos]`— a
    /// [`ENV_RATE`] muestras/s, cada una normalizada a su propio percentil 95.
    /// Un puñado de muestras más corta que `envelope` (el ancho de la ventana
    /// FFT); fuera de rango, [`Self::bands_at`] devuelve 0.
    pub band_env: [Vec<f32>; 3],
    /// Tiempos (s) de los beats/onsets detectados, ordenados.
    pub onsets: Vec<f32>,
    /// Tempo estimado (autocorrelación del flujo espectral). Informativo.
    pub bpm: Option<f32>,
}

impl MusicAnalysis {
    /// Envolvente en el tiempo `t` (s); 0 fuera de la pista.
    pub fn envelope_at(&self, t: f32) -> f32 {
        if t < 0.0 {
            return 0.0;
        }
        self.envelope
            .get((t * ENV_RATE as f32) as usize)
            .copied()
            .unwrap_or(0.0)
    }

    /// Energía por banda —`[graves, medios, agudos]`— en el tiempo `t` (s);
    /// 0 fuera de la pista (incluida la cola sin cubrir por la ventana FFT).
    pub fn bands_at(&self, t: f32) -> [f32; 3] {
        if t < 0.0 {
            return [0.0; 3];
        }
        let i = (t * ENV_RATE as f32) as usize;
        [
            self.band_env[0].get(i).copied().unwrap_or(0.0),
            self.band_env[1].get(i).copied().unwrap_or(0.0),
            self.band_env[2].get(i).copied().unwrap_or(0.0),
        ]
    }
}

/// Decodifica la pista a PCM `f32` mono 44100 Hz con `ffmpeg`.
fn decode(path: &str) -> io::Result<Vec<f32>> {
    let mut child = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            path,
            "-ac",
            "1",
            "-ar",
            &SR.to_string(),
            "-f",
            "f32le",
            "-",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut bytes = Vec::new();
    child
        .stdout
        .take()
        .expect("stdout de ffmpeg")
        .read_to_end(&mut bytes)?;
    let status = child.wait()?;
    if bytes.len() < 4 {
        let motivo = if status.success() {
            "pista vacía"
        } else {
            "ffmpeg no pudo decodificarla"
        };
        return Err(io::Error::other(format!("'{path}': {motivo}")));
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect())
}

/// Análisis completo de la pista (bloqueante; usar [`analyze_async`]).
pub fn analyze(path: &str) -> io::Result<MusicAnalysis> {
    Ok(analyze_samples(&decode(path)?))
}

/// Lanza el análisis en un hilo y devuelve el canal por el que llegará el
/// resultado (sondear con `try_recv` desde el bucle principal).
pub fn analyze_async(path: String) -> mpsc::Receiver<io::Result<MusicAnalysis>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(analyze(&path));
    });
    rx
}

/// Normaliza `v` in-place a ~0..1 dividiendo por su percentil 95 (robusto a
/// picos aislados); satura a 1.0. Usado por la envolvente y por cada banda.
fn normalize_p95(v: &mut [f32]) {
    let mut sorted = v.to_vec();
    sorted.sort_by(f32::total_cmp);
    let p95 = sorted
        .get(sorted.len() * 95 / 100)
        .copied()
        .unwrap_or(0.0)
        .max(1e-6);
    for x in v.iter_mut() {
        *x = (*x / p95).min(1.0);
    }
}

/// Núcleo del análisis, puro y testeable (recibe las muestras ya decodificadas).
fn analyze_samples(samples: &[f32]) -> MusicAnalysis {
    let duration = samples.len() as f32 / SR as f32;

    // --- Envolvente: RMS por bloque de HOP muestras, normalizada al p95 ---
    let mut envelope: Vec<f32> = samples
        .chunks(HOP)
        .map(|c| (c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32).sqrt())
        .collect();
    normalize_p95(&mut envelope);

    // --- Flujo espectral: FFT con ventana de Hann cada HOP muestras ---
    let n_frames = if samples.len() >= WIN {
        (samples.len() - WIN) / HOP + 1
    } else {
        0
    };
    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(WIN);
    let hann: Vec<f32> = (0..WIN)
        .map(|i| {
            let x = i as f32 / (WIN - 1) as f32;
            0.5 - 0.5 * (std::f32::consts::TAU * x).cos()
        })
        .collect();
    let mut buf = fft.make_input_vec();
    let mut spec = fft.make_output_vec();
    let mut prev_mag = vec![0.0f32; spec.len()];
    let mut flux = Vec::with_capacity(n_frames);
    // Bandas de frecuencia (SR 44100, WIN 2048 → Δf 21.53 Hz): graves bins
    // 1..12 (<250 Hz), medios 12..93 (250–2000 Hz), agudos 93..372 (2000–8000
    // Hz). Energía RMS de las magnitudes del bloque, por banda y por frame.
    let mut band_raw: [Vec<f32>; 3] = [
        Vec::with_capacity(n_frames),
        Vec::with_capacity(n_frames),
        Vec::with_capacity(n_frames),
    ];
    for t in 0..n_frames {
        let s = &samples[t * HOP..t * HOP + WIN];
        for i in 0..WIN {
            buf[i] = s[i] * hann[i];
        }
        let _ = fft.process(&mut buf, &mut spec);
        // Solo los incrementos de magnitud (energía que APARECE) cuentan: así
        // los finales de nota no disparan beats.
        let mut f = 0.0;
        let mut band_sq = [0.0f32; 3];
        let mut band_n = [0u32; 3];
        for (k, c) in spec.iter().enumerate() {
            let m = c.norm();
            let d = m - prev_mag[k];
            if d > 0.0 {
                f += d;
            }
            prev_mag[k] = m;
            let bi = if (1..12).contains(&k) {
                Some(0)
            } else if (12..93).contains(&k) {
                Some(1)
            } else if (93..372).contains(&k) {
                Some(2)
            } else {
                None
            };
            if let Some(bi) = bi {
                band_sq[bi] += m * m;
                band_n[bi] += 1;
            }
        }
        flux.push(f);
        for bi in 0..3 {
            let amp = if band_n[bi] > 0 {
                (band_sq[bi] / band_n[bi] as f32).sqrt()
            } else {
                0.0
            };
            band_raw[bi].push(amp);
        }
    }
    let fmax = flux.iter().cloned().fold(0.0f32, f32::max).max(1e-6);
    for v in &mut flux {
        *v /= fmax;
    }
    for band in &mut band_raw {
        normalize_p95(band);
    }

    // --- Onsets: pico local sobre umbral adaptativo, con refractario ---
    // Umbral = media local (±0.5 s) * 1.5 + suelo, para adaptarse a pasajes
    // fuertes y flojos; el refractario evita dobles disparos del mismo golpe.
    let w = (ENV_RATE / 2) as usize;
    let refract = (0.12 * ENV_RATE as f32) as isize; // ~120 ms
    let mut onsets = Vec::new();
    let mut last = -(refract + 1);
    for t in 1..flux.len().saturating_sub(1) {
        let a = t.saturating_sub(w);
        let b = (t + w).min(flux.len());
        let mean = flux[a..b].iter().sum::<f32>() / (b - a) as f32;
        let thr = mean * 1.5 + 0.05;
        if flux[t] > thr
            && flux[t] >= flux[t - 1]
            && flux[t] >= flux[t + 1]
            && (t as isize - last) > refract
        {
            onsets.push(t as f32 / ENV_RATE as f32);
            last = t as isize;
        }
    }

    let bpm = estimate_bpm(&flux);
    MusicAnalysis {
        duration,
        envelope,
        band_env: band_raw,
        onsets,
        bpm,
    }
}

/// Tempo por autocorrelación del flujo espectral, buscando el mejor periodo
/// entre 60 y 200 BPM. `None` con menos de ~4 s de material.
fn estimate_bpm(flux: &[f32]) -> Option<f32> {
    let n = flux.len();
    if n < 4 * ENV_RATE as usize {
        return None;
    }
    let mean = flux.iter().sum::<f32>() / n as f32;
    let x: Vec<f32> = flux.iter().map(|v| v - mean).collect();
    // A ENV_RATE muestras/s: lag = 3600 / BPM → 200 BPM = 18, 60 BPM = 60.
    let (mut best_lag, mut best) = (0usize, 0.0f32);
    for lag in 18..=60usize {
        let mut acc = 0.0;
        for t in 0..n - lag {
            acc += x[t] * x[t + lag];
        }
        if acc > best {
            best = acc;
            best_lag = lag;
        }
    }
    (best > 0.0).then(|| 3600.0 / best_lag as f32)
}

/// Preescucha de la pista con `ffplay` (viene con ffmpeg). Mientras viva, la
/// música suena; al soltarla se mata el proceso.
pub struct Preview {
    child: Child,
    started: Instant,
}

impl Preview {
    pub fn start(path: &str) -> Option<Preview> {
        let child = Command::new("ffplay")
            .args(["-nodisp", "-autoexit", "-loglevel", "error", path])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| eprintln!("No se pudo lanzar ffplay para la preescucha: {e}"))
            .ok()?;
        Some(Preview {
            child,
            started: Instant::now(),
        })
    }

    /// Segundos desde el arranque (reloj aproximado de la preescucha).
    pub fn elapsed(&self) -> f32 {
        self.started.elapsed().as_secs_f32()
    }

    /// `true` si `ffplay` ya terminó (fin de la pista o error).
    pub fn finished(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
    }
}

impl Drop for Preview {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Señal sintética: silencio con ráfagas de ruido de 50 ms cada 0.5 s.
    /// Los onsets deben detectarse todos, cada uno a ±1 hop (~17 ms) del golpe,
    /// y la envolvente debe quedar a ~0 en el silencio.
    #[test]
    fn onsets_de_senal_sintetica() {
        let dur_s = 5.0f32;
        let n = (SR as f32 * dur_s) as usize;
        let mut samples = vec![0.0f32; n];
        let burst = (SR as f32 * 0.05) as usize;
        let mut golpes = Vec::new();
        let mut t = 0.5f32;
        // Ruido determinista barato (LCG) para no depender de `rand` en el test.
        let mut seed = 0x12345678u32;
        let mut noise = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) as f32 / (1 << 24) as f32 * 1.6 - 0.8
        };
        while t < dur_s - 0.1 {
            let start = (SR as f32 * t) as usize;
            for i in 0..burst {
                samples[start + i] = noise();
            }
            golpes.push(t);
            t += 0.5;
        }

        let a = analyze_samples(&samples);

        assert_eq!(
            a.onsets.len(),
            golpes.len(),
            "onsets detectados: {:?} vs golpes {:?}",
            a.onsets,
            golpes
        );
        let tol = 2.0 / ENV_RATE as f32; // ±2 hops de margen
        for (o, g) in a.onsets.iter().zip(&golpes) {
            assert!(
                (o - g).abs() <= tol,
                "onset {o:.3} lejos del golpe {g:.3}"
            );
        }
        // Envolvente ~0 en un tramo de silencio (justo antes del 2º golpe).
        let silencio = a.envelope_at(0.95);
        assert!(silencio < 0.05, "envolvente en silencio = {silencio}");
        // Y alta durante una ráfaga.
        assert!(a.envelope_at(0.51) > 0.5);
        assert!((a.duration - dur_s).abs() < 0.05);
    }

    /// End-to-end con ffmpeg real: genera una pista de clics a 120 BPM y la
    /// decodifica + analiza por el camino completo. Ignorada por defecto
    /// (requiere ffmpeg); correr con `cargo test -p shared -- --ignored`.
    #[test]
    #[ignore]
    fn analiza_pista_generada_con_ffmpeg() {
        let path = std::env::temp_dir().join(format!("enjambre_clicks_{}.wav", std::process::id()));
        let path_s = path.to_string_lossy().into_owned();
        // Ráfagas de 60 ms de un tono de 880 Hz cada 0.5 s (= 120 BPM), 6 s.
        let status = Command::new("ffmpeg")
            .args([
                "-y", "-hide_banner", "-loglevel", "error",
                "-f", "lavfi",
                "-i", "aevalsrc=if(lt(mod(t\\,0.5)\\,0.06)\\,0.8*sin(880*2*PI*t)\\,0):d=6",
                path_s.as_str(),
            ])
            .status()
            .expect("¿está ffmpeg en el PATH?");
        assert!(status.success(), "ffmpeg no pudo generar la pista");

        let a = analyze(&path_s).expect("análisis");
        let _ = std::fs::remove_file(&path);

        assert!((a.duration - 6.0).abs() < 0.1, "duración {}", a.duration);
        // 12 clics; tolera ±1 en los bordes.
        assert!(
            (11..=13).contains(&a.onsets.len()),
            "onsets: {:?}",
            a.onsets
        );
        let bpm = a.bpm.expect("bpm");
        assert!((bpm - 120.0).abs() < 10.0, "bpm = {bpm}");
    }

    /// Señal sintética de 6 s con 3 tramos de 2 s a 100 Hz / 1 kHz / 5 kHz (misma
    /// amplitud): en cada tramo debe dominar su banda (graves/medios/agudos).
    #[test]
    fn bandas_de_senal_sintetica() {
        let dur_s = 6.0f32;
        let n = (SR as f32 * dur_s) as usize;
        let mut samples = vec![0.0f32; n];
        let freqs = [100.0f32, 1000.0, 5000.0];
        for (i, s) in samples.iter_mut().enumerate() {
            let t = i as f32 / SR as f32;
            let tramo = ((t / 2.0) as usize).min(2);
            *s = 0.8 * (std::f32::consts::TAU * freqs[tramo] * t).sin();
        }

        let a = analyze_samples(&samples);

        let dominant = |t: f32| -> usize {
            let b = a.bands_at(t);
            let mut best = 0;
            for i in 1..3 {
                if b[i] > b[best] {
                    best = i;
                }
            }
            best
        };
        let b1 = a.bands_at(1.0);
        assert_eq!(dominant(1.0), 0, "t=1.0 bandas={b1:?}");
        assert!(b1[0] > 0.7 && b1[1] < 0.3 && b1[2] < 0.3, "t=1.0 bandas={b1:?}");

        let b3 = a.bands_at(3.0);
        assert_eq!(dominant(3.0), 1, "t=3.0 bandas={b3:?}");
        assert!(b3[1] > 0.7 && b3[0] < 0.3 && b3[2] < 0.3, "t=3.0 bandas={b3:?}");

        let b5 = a.bands_at(5.0);
        assert_eq!(dominant(5.0), 2, "t=5.0 bandas={b5:?}");
        assert!(b5[2] > 0.7 && b5[0] < 0.3 && b5[1] < 0.3, "t=5.0 bandas={b5:?}");
    }
}
