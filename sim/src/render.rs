use crate::simulation::Simulation;
use macroquad::prelude::*;
use shared::{color_for_hue, RenderStyle, SimParams};

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

    /// Limpia el fondo a negro y dibuja todas las partículas. Camino normal (sin
    /// estelas).
    pub fn draw(&mut self, sim: &Simulation, params: &SimParams) {
        clear_background(BLACK);
        self.draw_particles(sim, params);
    }

    /// Dibuja las partículas SIN limpiar el fondo (para el buffer de estelas).
    /// Mezcla disco↔flecha según `params.orient`: con 0 solo discos, con 1 solo
    /// flechas, y en medio ambos con alfa cruzado (transición fluida entre
    /// escenas). Agrupa los vértices en pocos `draw_mesh` para eficiencia.
    pub fn draw_particles(&mut self, sim: &Simulation, params: &SimParams) {
        let alpha = params.brightness.clamp(0.0, 1.0);
        let arrow_amt = params.orient.clamp(0.0, 1.0);
        let disc_amt = 1.0 - arrow_amt;
        if disc_amt > 0.003 {
            self.draw_discs(sim, params, alpha * disc_amt);
        }
        if arrow_amt > 0.003 {
            self.draw_arrows(sim, params, alpha * arrow_amt);
        }
    }

    /// Discos texturizados según el estilo, con alfa `a`.
    fn draw_discs(&mut self, sim: &Simulation, params: &SimParams, a: f32) {
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
        self.mesh.texture = Some(tex.clone());

        for chunk in sim.particles.chunks(CHUNK) {
            let verts = &mut self.mesh.vertices;
            let inds = &mut self.mesh.indices;
            verts.clear();
            inds.clear();

            for p in chunk {
                let base = verts.len() as u16;
                let [r, g, b] = color_for_hue(p.hue);
                let c = Color::new(r, g, b, a);
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

    /// Triángulos orientados por la velocidad (flechas), con alfa `a`.
    fn draw_arrows(&mut self, sim: &Simulation, params: &SimParams, a: f32) {
        self.mesh.texture = None;
        let s = params.point_size * 1.3;
        for chunk in sim.particles.chunks(CHUNK) {
            let verts = &mut self.mesh.vertices;
            let inds = &mut self.mesh.indices;
            verts.clear();
            inds.clear();

            for p in chunk {
                let base = verts.len() as u16;
                let [r, g, b] = color_for_hue(p.hue);
                let c = Color::new(r, g, b, a);
                let dir = p.vel.normalize_or(Vec2::X);
                let perp = Vec2::new(-dir.y, dir.x);
                let tip = p.pos + dir * (s * 1.8);
                let l = p.pos - dir * (s * 0.9) + perp * s;
                let rr = p.pos - dir * (s * 0.9) - perp * s;
                verts.push(Vertex::new(tip.x, tip.y, 0.0, 0.5, 0.0, c));
                verts.push(Vertex::new(l.x, l.y, 0.0, 0.0, 1.0, c));
                verts.push(Vertex::new(rr.x, rr.y, 0.0, 1.0, 1.0, c));
                inds.extend_from_slice(&[base, base + 1, base + 2]);
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
