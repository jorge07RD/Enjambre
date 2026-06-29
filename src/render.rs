use crate::config::{color_for_hue, RenderStyle, SimParams};
use crate::simulation::Simulation;
use macroquad::prelude::*;

/// Partículas por lote de dibujo. Se mantiene por debajo de los límites
/// internos del batcher de macroquad para `draw_mesh`.
const CHUNK: usize = 800;

pub struct Renderer {
    glow: Texture2D,
    solid: Texture2D,
    solid_halo: Texture2D,
    mesh: Mesh,
}

impl Renderer {
    pub fn new() -> Self {
        Self {
            glow: make_texture(64, RenderStyle::Glow),
            solid: make_texture(64, RenderStyle::Solid),
            solid_halo: make_texture(64, RenderStyle::SolidHalo),
            mesh: Mesh {
                vertices: Vec::new(),
                indices: Vec::new(),
                texture: None,
            },
        }
    }

    /// Dibuja el fondo negro y todas las partículas, agrupando los vértices en
    /// pocos `draw_mesh` para eficiencia.
    pub fn draw(&mut self, sim: &Simulation, params: &SimParams) {
        clear_background(BLACK);

        // El tamaño del quad depende del estilo: el glow y el halo se extienden
        // un poco más allá del radio nominal.
        let s = match params.style {
            RenderStyle::Solid => params.point_size,
            RenderStyle::Glow => params.point_size * 1.6,
            RenderStyle::SolidHalo => params.point_size * 1.8,
        };
        let tex = match params.style {
            RenderStyle::Glow => &self.glow,
            RenderStyle::Solid => &self.solid,
            RenderStyle::SolidHalo => &self.solid_halo,
        };
        let alpha = params.brightness.clamp(0.0, 1.0);
        self.mesh.texture = Some(tex.clone());

        for chunk in sim.particles.chunks(CHUNK) {
            let verts = &mut self.mesh.vertices;
            let inds = &mut self.mesh.indices;
            verts.clear();
            inds.clear();

            for p in chunk {
                let base = verts.len() as u16;
                let mut c = color_for_hue(p.hue);
                c.a = alpha;
                let (x, y) = (p.pos.x, p.pos.y);
                verts.push(Vertex::new(x - s, y - s, 0.0, 0.0, 0.0, c));
                verts.push(Vertex::new(x + s, y - s, 0.0, 1.0, 0.0, c));
                verts.push(Vertex::new(x + s, y + s, 0.0, 1.0, 1.0, c));
                verts.push(Vertex::new(x - s, y + s, 0.0, 0.0, 1.0, c));
                inds.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
            }

            draw_mesh(&self.mesh);
        }
    }
}

/// Genera la textura del punto según el estilo. El color lo pone el vértice;
/// aquí solo definimos la forma/alfa (en blanco).
fn make_texture(size: u32, style: RenderStyle) -> Texture2D {
    let mut image = Image::gen_image_color(size as u16, size as u16, BLANK);
    let c = size as f32 / 2.0;
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - c + 0.5;
            let dy = y as f32 - c + 0.5;
            let d = ((dx * dx + dy * dy).sqrt() / c).min(1.0);
            let a = match style {
                // Caída suave: brilla más en el centro.
                RenderStyle::Glow => (1.0 - d).powf(2.2),
                // Disco lleno con un borde antialias en el último 8%.
                RenderStyle::Solid => ((0.96 - d) / 0.08).clamp(0.0, 1.0),
                // Núcleo opaco hasta el 45% del radio + halo suave alrededor.
                RenderStyle::SolidHalo => {
                    if d < 0.45 {
                        1.0
                    } else {
                        (1.0 - (d - 0.45) / 0.55).clamp(0.0, 1.0).powf(1.8)
                    }
                }
            };
            image.set_pixel(x, y, Color::new(1.0, 1.0, 1.0, a));
        }
    }
    let tex = Texture2D::from_image(&image);
    tex.set_filter(FilterMode::Linear);
    tex
}
