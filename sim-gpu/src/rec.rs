//! Grabación de vídeo desde la GPU: la escena (sin el panel) se blitea a una
//! textura rgba8 del tamaño de la ventana (dimensiones pares), se copia a un
//! anillo de 3 staging buffers (filas alineadas a 256, `map_async`) y se
//! vuelca a ffmpeg por stdin — la lectura va hasta 2 frames por detrás de la
//! escritura para no serializar la GPU en cada frame.
//!
//! Mismos argumentos de ffmpeg que el `Recorder` de `sim` (H.264 CRF 18 +
//! música mezclada con `-shortest`), sin el supersampling ×2: aquí ya se
//! captura a la resolución nativa de la ventana. Cada frame renderizado es
//! 1/60 s de vídeo (el bucle usa dt fijo mientras graba), así el .mp4 sale
//! exacto aunque el volcado vaya más lento que el tiempo real.

use std::collections::VecDeque;
use std::io::{self, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;

pub const REC_FPS: u32 = 60;

pub struct Recorder {
    child: Child,
    stdin: ChildStdin,
    pub frames: u32,
    path: String,
    w: u32,
    h: u32,
    /// Bytes por fila alineados a 256 (requisito de `copy_texture_to_buffer`).
    padded_bpr: u32,
    texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    ring: Vec<wgpu::Buffer>,
    /// Lecturas en vuelo, en orden de frame: (slot, canal del map_async).
    pending: VecDeque<(usize, mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>)>,
    /// Slot del anillo que usará la próxima copia.
    next: usize,
}

impl Recorder {
    /// Arranca `ffmpeg` y la textura/buffers de captura a `w×h` (recortado a
    /// pares: yuv420p lo exige), guardando en `dir` (o el directorio actual).
    /// Si `music` no está vacío se mezcla esa pista (recortada con -shortest).
    pub fn start(
        device: &wgpu::Device,
        w: u32,
        h: u32,
        dir: &str,
        music: &str,
    ) -> io::Result<Recorder> {
        let w = (w & !1).max(2);
        let h = (h & !1).max(2);
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let name = format!("enjambre_{ts}.mp4");
        let path = if dir.is_empty() {
            name
        } else {
            format!("{}/{}", dir.trim_end_matches('/'), name)
        };

        // Argumentos como en el Recorder de `sim` (la música añade un 2º input).
        let mut args: Vec<String> = vec![
            "-y".into(),
            "-f".into(), "rawvideo".into(),
            "-pix_fmt".into(), "rgba".into(),
            "-s".into(), format!("{w}x{h}"),
            "-r".into(), REC_FPS.to_string(),
            "-i".into(), "-".into(),
        ];
        let has_music = !music.is_empty();
        if has_music {
            args.extend(["-i".into(), music.to_string()]);
        }
        args.extend([
            "-c:v".into(), "libx264".into(),
            "-preset".into(), "medium".into(),
            "-crf".into(), "18".into(),
            "-pix_fmt".into(), "yuv420p".into(),
        ]);
        if has_music {
            args.extend([
                "-map".into(), "0:v:0".into(),
                "-map".into(), "1:a:0".into(),
                "-c:a".into(), "aac".into(),
                "-b:a".into(), "192k".into(),
                "-shortest".into(),
            ]);
        }
        args.extend(["-movflags".into(), "+faststart".into(), path.clone()]);

        let mut child = Command::new("ffmpeg")
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child.stdin.take().expect("stdin de ffmpeg");

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("captura de vídeo"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let padded_bpr = (w * 4).div_ceil(256) * 256;
        let ring = (0..3)
            .map(|i| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(&format!("captura staging {i}")),
                    size: (padded_bpr as u64) * (h as u64),
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            })
            .collect();

        let music_note = if has_music { " + música" } else { "" };
        eprintln!("● Grabando en {path} ({w}×{h} @{REC_FPS}fps{music_note}, R para parar)");
        Ok(Recorder {
            child,
            stdin,
            frames: 0,
            path,
            w,
            h,
            padded_bpr,
            texture,
            view,
            ring,
            pending: VecDeque::new(),
            next: 0,
        })
    }

    /// Encola la copia textura→staging del frame recién pintado (slot `next`).
    pub fn copy_frame(&self, encoder: &mut wgpu::CommandEncoder) {
        encoder.copy_texture_to_buffer(
            self.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &self.ring[self.next],
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_bpr),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width: self.w,
                height: self.h,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Tras el `submit`: mapea el slot recién copiado y vuelca los frames que
    /// toquen, dejando como mucho 2 lecturas en vuelo (así el slot de la
    /// próxima copia siempre está libre).
    pub fn after_submit(&mut self, device: &wgpu::Device) -> io::Result<()> {
        let idx = self.next;
        let (tx, rx) = mpsc::channel();
        self.ring[idx].slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.pending.push_back((idx, rx));
        self.next = (self.next + 1) % self.ring.len();
        while self.pending.len() > 2 {
            self.drain_one(device)?;
        }
        Ok(())
    }

    /// Espera el map más antiguo, quita el padding por fila y lo manda a ffmpeg.
    fn drain_one(&mut self, device: &wgpu::Device) -> io::Result<()> {
        let Some((idx, rx)) = self.pending.pop_front() else {
            return Ok(());
        };
        device.poll(wgpu::Maintain::Wait);
        rx.recv()
            .map_err(|e| io::Error::other(e.to_string()))?
            .map_err(|e| io::Error::other(e.to_string()))?;
        {
            let data = self.ring[idx].slice(..).get_mapped_range();
            let row_bytes = (self.w * 4) as usize;
            for row in 0..self.h as usize {
                let start = row * self.padded_bpr as usize;
                self.stdin.write_all(&data[start..start + row_bytes])?;
            }
        }
        self.ring[idx].unmap();
        self.frames += 1;
        Ok(())
    }

    /// Vuelca lo pendiente, cierra la tubería (EOF → ffmpeg finaliza el .mp4)
    /// y espera a que termine de escribir. Devuelve la ruta del `.mp4` (para un
    /// posible post-muxeo del audio del vídeo del efecto foto).
    pub fn finish(mut self, device: &wgpu::Device) -> String {
        while !self.pending.is_empty() {
            if let Err(e) = self.drain_one(device) {
                eprintln!("Grabación: error volcando los últimos frames: {e}");
                break;
            }
        }
        drop(self.stdin);
        let _ = self.child.wait();
        eprintln!(
            "■ Vídeo guardado: {} ({} frames · {:.1}s a {REC_FPS} fps)",
            self.path,
            self.frames,
            self.frames as f32 / REC_FPS as f32
        );
        self.path.clone()
    }
}
