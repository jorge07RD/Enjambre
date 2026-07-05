//! Generación de los puntos meta de una forma (texto o imagen) para el visor
//! GPU: port de `image_to_points`/`text_to_points` de `sim/src/main.rs`, sin
//! macroquad — `ab_glyph` rasteriza el texto con una fuente del sistema y el
//! crate `image` decodifica los ficheros. Los puntos resultantes se suben al
//! buffer de la GPU (ver `GpuSim::upload_shape_targets`).

use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
use rand::Rng;

/// Fisher–Yates parcial: deja `on` con como mucho `count` elementos.
fn partial_shuffle(on: &mut Vec<(usize, usize)>, count: usize, rng: &mut impl Rng) {
    if on.len() > count {
        for k in 0..count {
            let j = rng.gen_range(k..on.len());
            on.swap(k, j);
        }
        on.truncate(count);
    }
}

/// Mapea píxeles (px,py) a puntos de mundo, preservando aspecto, centrado al
/// 90% de la caja. `flip_y` compensa que el origen de fila 0 de una imagen de
/// disco es la parte de ARRIBA de la foto tal como se ve normalmente, pero
/// necesita invertirse para que no aparezca boca abajo en el mundo (el texto,
/// que ya rasteriza con el origen correcto, no lo necesita).
fn map_points(on: &[(usize, usize)], w: usize, h: usize, world: [f32; 2], flip_y: bool) -> Vec<[f32; 2]> {
    let iw = w as f32;
    let ih = h as f32;
    let scale = (world[0] * 0.9 / iw).min(world[1] * 0.9 / ih);
    let (cx, cy) = (world[0] * 0.5, world[1] * 0.5);
    on.iter()
        .map(|&(px, py)| {
            let sx = (px as f32 + 0.5) / iw;
            let mut sy = (py as f32 + 0.5) / ih;
            if flip_y {
                sy = 1.0 - sy;
            }
            [cx + (sx - 0.5) * iw * scale, cy + (sy - 0.5) * ih * scale]
        })
        .collect()
}

/// Máscara de píxeles "encendidos" → puntos de mundo: submuestrea a `count`
/// (Fisher–Yates parcial) y ajusta la caja al lienzo con margen, preservando
/// el aspecto (= final de `image_to_points` de la CPU).
fn points_from_mask(
    mut on: Vec<(usize, usize)>,
    w: usize,
    h: usize,
    world: [f32; 2],
    count: usize,
    rng: &mut impl Rng,
    flip_y: bool,
) -> Vec<[f32; 2]> {
    if on.is_empty() || w == 0 || h == 0 {
        return Vec::new();
    }
    partial_shuffle(&mut on, count.max(1), rng);
    map_points(&on, w, h, world, flip_y)
}

/// Puntos meta (mundo) de una rejilla `~count` sobre la caja de la foto
/// (centro `c`, extensión `e`), pero SOLO en las celdas con píxel opaco: en un
/// PNG sin fondo, las partículas se reclutan únicamente donde hay imagen (las
/// zonas transparentes se quedan sin partículas). `rgba`/`w`/`h` es la imagen.
/// Las coordenadas coinciden con las de la textura superpuesta (fila 0 =
/// arriba), así el mosaico y la imagen quedan alineados.
pub fn mosaic_points(
    rgba: &[u8],
    w: usize,
    h: usize,
    c: [f32; 2],
    e: [f32; 2],
    count: usize,
) -> Vec<[f32; 2]> {
    let count = count.max(1);
    if w == 0 || h == 0 {
        return Vec::new();
    }
    let aspect = (e[0] / e[1].max(1e-3)).max(1e-3);
    let cols = ((count as f32 * aspect).sqrt().round().max(1.0)) as usize;
    let rows = ((count as f32 / aspect).sqrt().round().max(1.0)) as usize;
    let (x0, y0) = (c[0] - e[0] * 0.5, c[1] - e[1] * 0.5);
    let mut pts = Vec::with_capacity(cols * rows);
    for gy in 0..rows {
        let py = (((gy as f32 + 0.5) / rows as f32 * h as f32) as usize).min(h - 1);
        let wy = y0 + (gy as f32 + 0.5) / rows as f32 * e[1];
        for gx in 0..cols {
            let px = (((gx as f32 + 0.5) / cols as f32 * w as f32) as usize).min(w - 1);
            if rgba[(py * w + px) * 4 + 3] > 128 {
                let wx = x0 + (gx as f32 + 0.5) / cols as f32 * e[0];
                pts.push([wx, wy]);
            }
        }
    }
    pts
}

/// Decodifica una imagen de disco a RGBA8 crudo + dimensiones, para subirla
/// como textura y muestrearla bajo el enjambre (modo "recrear los colores de
/// la foto"). `None` si no se pudo abrir/decodificar.
pub fn decode_image_rgba(path: &str) -> Option<(Vec<u8>, u32, u32)> {
    match image::open(path) {
        Ok(img) => {
            let rgba = img.to_rgba8();
            let (w, h) = (rgba.width(), rgba.height());
            Some((rgba.into_raw(), w, h))
        }
        Err(e) => {
            eprintln!("No pude abrir/decodificar la imagen '{path}': {e}");
            None
        }
    }
}

/// Umbraliza una imagen RGBA: por alfa si la imagen usa transparencia de
/// forma significativa (>5% de píxeles con alfa parcial), o por luminancia.
fn mask_from_rgba(bytes: &[u8], w: usize, h: usize) -> Vec<(usize, usize)> {
    let total = w * h;
    let mut transparent = 0usize;
    let mut sampled = 0usize;
    let mut i = 0;
    while i < total {
        if bytes[i * 4 + 3] < 250 {
            transparent += 1;
        }
        sampled += 1;
        i += 7;
    }
    let use_alpha = transparent * 20 > sampled;

    let mut on = Vec::new();
    for py in 0..h {
        for px in 0..w {
            let idx = (py * w + px) * 4;
            let hit = if use_alpha {
                bytes[idx + 3] > 128
            } else {
                let lum = 0.299 * bytes[idx] as f32
                    + 0.587 * bytes[idx + 1] as f32
                    + 0.114 * bytes[idx + 2] as f32;
                lum > 128.0
            };
            if hit {
                on.push((px, py));
            }
        }
    }
    on
}

/// Lee una imagen de disco y devuelve sus puntos meta (o `None` si falla).
pub fn image_points_from_path(
    path: &str,
    world: [f32; 2],
    count: usize,
    rng: &mut impl Rng,
) -> Option<Vec<[f32; 2]>> {
    let img = match image::open(path) {
        Ok(i) => i.to_rgba8(),
        Err(e) => {
            eprintln!("No pude abrir/decodificar la imagen '{path}': {e}");
            return None;
        }
    };
    let (w, h) = (img.width() as usize, img.height() as usize);
    Some(points_from_mask(
        mask_from_rgba(img.as_raw(), w, h),
        w,
        h,
        world,
        count,
        rng,
        true,
    ))
}

/// Fuente del sistema para rasterizar el texto: la que resuelva fontconfig
/// para "sans-serif" y, si no hay `fc-match`, unas rutas típicas de Linux.
fn load_font() -> Option<FontVec> {
    if let Ok(out) = std::process::Command::new("fc-match")
        .args(["--format=%{file}", "sans-serif"])
        .output()
    {
        if out.status.success() {
            let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if let Ok(bytes) = std::fs::read(&path) {
                if let Ok(f) = FontVec::try_from_vec(bytes) {
                    return Some(f);
                }
            }
        }
    }
    for path in [
        "/usr/share/fonts/liberation/LiberationSans-Regular.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/noto/NotoSans-Regular.ttf",
    ] {
        if let Ok(bytes) = std::fs::read(path) {
            if let Ok(f) = FontVec::try_from_vec(bytes) {
                return Some(f);
            }
        }
    }
    None
}

/// Rasteriza `text` (una línea, con kerning) y devuelve sus puntos meta.
pub fn text_to_points(
    text: &str,
    world: [f32; 2],
    count: usize,
    rng: &mut impl Rng,
) -> Vec<[f32; 2]> {
    let Some(font) = load_font() else {
        eprintln!("Sin fuente del sistema para el texto (¿fontconfig instalado?).");
        return Vec::new();
    };
    let scale = PxScale::from(180.0);
    let sf = font.as_scaled(scale);
    let pad = 24.0f32;

    // Layout en una línea: pluma que avanza con el ancho + kerning.
    let mut glyphs = Vec::new();
    let mut pen = 0.0f32;
    let mut last: Option<ab_glyph::GlyphId> = None;
    for ch in text.chars() {
        if ch.is_control() {
            continue;
        }
        let gid = font.glyph_id(ch);
        if let Some(prev) = last {
            pen += sf.kern(prev, gid);
        }
        glyphs.push(gid.with_scale_and_position(scale, ab_glyph::point(pad + pen, pad + sf.ascent())));
        pen += sf.h_advance(gid);
        last = Some(gid);
    }
    let w = (pen + pad * 2.0).ceil().max(8.0) as usize;
    let h = (sf.ascent() - sf.descent() + pad * 2.0).ceil().max(8.0) as usize;

    // Cobertura > 0.5 = píxel del texto (mismo umbral que la textura CPU).
    let mut mask = vec![false; w * h];
    for g in glyphs {
        if let Some(og) = font.outline_glyph(g) {
            let b = og.px_bounds();
            og.draw(|x, y, c| {
                if c > 0.5 {
                    let px = (b.min.x + x as f32) as i32;
                    let py = (b.min.y + y as f32) as i32;
                    if px >= 0 && (px as usize) < w && py >= 0 && (py as usize) < h {
                        mask[py as usize * w + px as usize] = true;
                    }
                }
            });
        }
    }
    let mut on = Vec::new();
    for py in 0..h {
        for px in 0..w {
            if mask[py * w + px] {
                on.push((px, py));
            }
        }
    }
    points_from_mask(on, w, h, world, count, rng, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// El texto rasteriza a puntos dentro del lienzo, en cantidad pedida.
    /// (Necesita una fuente del sistema; en un entorno sin fontconfig ni
    /// fuentes el rasterizador devuelve vacío y este test lo detectaría.)
    #[test]
    fn texto_a_puntos() {
        let world = [1600.0, 1000.0];
        let mut rng = rand::thread_rng();
        let pts = text_to_points("HOLA", world, 5000, &mut rng);
        assert!(!pts.is_empty(), "sin puntos: ¿no hay fuente del sistema?");
        assert!(pts.len() <= 5000);
        for p in &pts {
            assert!(p[0] >= 0.0 && p[0] <= world[0], "x fuera de mundo: {}", p[0]);
            assert!(p[1] >= 0.0 && p[1] <= world[1], "y fuera de mundo: {}", p[1]);
        }
        // Una "H" ancha: los puntos deben cubrir un rango horizontal amplio
        // y quedar centrados en el lienzo.
        let (min_x, max_x) = pts.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| {
            (lo.min(p[0]), hi.max(p[0]))
        });
        assert!(max_x - min_x > world[0] * 0.3, "el texto quedó muy estrecho");
    }
}
