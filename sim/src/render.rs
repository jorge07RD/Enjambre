use crate::simulation::Simulation;
use macroquad::miniquad::{BlendFactor, BlendState, BlendValue, Equation};
use macroquad::prelude::*;
use shared::{color_for_hue, RenderStyle, SimParams};

/// Partículas por lote de dibujo. Se mantiene por debajo de los límites
/// internos del batcher de macroquad para `draw_mesh`.
const CHUNK: usize = 800;

/// Color de la partícula `i` en fase A del efecto foto: su matiz normal,
/// fundido hacia el color de la foto en su posición según se acomoda
/// (`shape_ease`). Solo las reclutadas (las primeras `shape.len()`); el resto
/// conserva su color. Sin foto o fuera de su caja, color normal.
fn particle_rgb(sim: &Simulation, i: usize, pos: Vec2, hue: f32) -> [f32; 3] {
    let base = color_for_hue(hue);
    let recruited = sim.shape.as_ref().map_or(0, |s| s.len());
    if i < recruited {
        if let Some(photo) = sim.photo.as_ref() {
            if let Some(target) = photo.color_at(pos) {
                let t = sim.shape_ease();
                return [
                    base[0] + (target[0] - base[0]) * t,
                    base[1] + (target[1] - base[1]) * t,
                    base[2] + (target[2] - base[2]) * t,
                ];
            }
        }
    }
    base
}

pub struct Renderer {
    glow: Texture2D,
    solid: Texture2D,
    solid_halo: Texture2D,
    mesh: Mesh,
    /// Material de mezcla ADITIVA para el resplandor (bloom). `None` si el
    /// backend no pudo compilar el shader (el bloom se omite).
    additive: Option<Material>,
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
            additive: make_additive_material(),
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
        // Resplandor (bloom): halo aditivo POR DEBAJO de las partículas.
        if params.bloom && params.bloom_intensity > 0.001 {
            self.draw_bloom(sim, params, alpha);
        }
        let arrow_amt = params.orient.clamp(0.0, 1.0);
        let disc_amt = 1.0 - arrow_amt;
        if disc_amt > 0.003 {
            self.draw_discs(sim, params, alpha * disc_amt);
        }
        if arrow_amt > 0.003 {
            self.draw_arrows(sim, params, alpha * arrow_amt);
        }
        // Fase B: la foto real se funde ENCIMA del mosaico de partículas.
        let reveal = sim.overlay_ease();
        if reveal > 0.001 {
            if let Some(photo) = sim.photo.as_ref() {
                let origin = photo.origin();
                draw_texture_ex(
                    &photo.tex,
                    origin.x,
                    origin.y,
                    Color::new(1.0, 1.0, 1.0, reveal),
                    DrawTextureParams {
                        dest_size: Some(photo.extent),
                        // La cámara del CPU (from_display_rect) tiene la Y
                        // invertida: sin esto la imagen sale boca abajo.
                        flip_y: true,
                        ..Default::default()
                    },
                );
            }
        }
    }

    /// Dibuja un halo suave y grande por partícula con mezcla ADITIVA (los
    /// solapes se suman → brillo tipo neón). Reutiliza la textura de glow. Si el
    /// material aditivo no está disponible, no hace nada.
    fn draw_bloom(&mut self, sim: &Simulation, params: &SimParams, a: f32) {
        let Some(mat) = self.additive.as_ref() else {
            return;
        };
        let s = params.point_size * params.bloom_radius.max(0.1);
        // La intensidad va en el alfa del vértice (la mezcla aditiva pondera por
        // alfa), atenuada para que se acumule con gracia.
        let ga = (a * params.bloom_intensity * 0.5).clamp(0.0, 1.0);
        self.mesh.texture = Some(self.glow.clone());
        gl_use_material(mat);
        for (ci, chunk) in sim.particles.chunks(CHUNK).enumerate() {
            let verts = &mut self.mesh.vertices;
            let inds = &mut self.mesh.indices;
            verts.clear();
            inds.clear();
            for (j, p) in chunk.iter().enumerate() {
                let base = verts.len() as u16;
                let idx = ci * CHUNK + j;
                let [r, g, b] = particle_rgb(sim, idx, p.pos, p.hue);
                let c = Color::new(r, g, b, ga);
                let (x, y) = (p.pos.x, p.pos.y);
                verts.push(Vertex::new(x - s, y - s, 0.0, 0.0, 0.0, c));
                verts.push(Vertex::new(x + s, y - s, 0.0, 1.0, 0.0, c));
                verts.push(Vertex::new(x + s, y + s, 0.0, 1.0, 1.0, c));
                verts.push(Vertex::new(x - s, y + s, 0.0, 0.0, 1.0, c));
                inds.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
            }
            draw_mesh(&self.mesh);
        }
        gl_use_default_material();
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

        for (ci, chunk) in sim.particles.chunks(CHUNK).enumerate() {
            let verts = &mut self.mesh.vertices;
            let inds = &mut self.mesh.indices;
            verts.clear();
            inds.clear();

            for (j, p) in chunk.iter().enumerate() {
                let base = verts.len() as u16;
                let idx = ci * CHUNK + j;
                let [r, g, b] = particle_rgb(sim, idx, p.pos, p.hue);
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
        for (ci, chunk) in sim.particles.chunks(CHUNK).enumerate() {
            let verts = &mut self.mesh.vertices;
            let inds = &mut self.mesh.indices;
            verts.clear();
            inds.clear();

            for (j, p) in chunk.iter().enumerate() {
                let base = verts.len() as u16;
                let idx = ci * CHUNK + j;
                let [r, g, b] = particle_rgb(sim, idx, p.pos, p.hue);
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

/// Crea un material igual al de dibujo 2D por defecto (usa `Model`/`Projection`/
/// `Texture` que macroquad rellena solo) pero con mezcla ADITIVA, para el bloom.
/// Devuelve `None` si el shader no compila (se omite el bloom).
fn make_additive_material() -> Option<Material> {
    // Shaders idénticos al pipeline 2D por defecto de macroquad.
    const VERTEX: &str = r#"#version 100
    attribute vec3 position;
    attribute vec2 texcoord;
    attribute vec4 color0;
    varying lowp vec2 uv;
    varying lowp vec4 color;
    uniform mat4 Model;
    uniform mat4 Projection;
    void main() {
        gl_Position = Projection * Model * vec4(position, 1);
        color = color0 / 255.0;
        uv = texcoord;
    }"#;
    const FRAGMENT: &str = r#"#version 100
    varying lowp vec4 color;
    varying lowp vec2 uv;
    uniform sampler2D Texture;
    void main() {
        gl_FragColor = color * texture2D(Texture, uv);
    }"#;

    load_material(
        ShaderSource::Glsl {
            vertex: VERTEX,
            fragment: FRAGMENT,
        },
        MaterialParams {
            pipeline_params: PipelineParams {
                color_blend: Some(BlendState::new(
                    Equation::Add,
                    BlendFactor::Value(BlendValue::SourceAlpha),
                    BlendFactor::One,
                )),
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .map_err(|e| eprintln!("Bloom desactivado (no compiló el shader): {e}"))
    .ok()
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
