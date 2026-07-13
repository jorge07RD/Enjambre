use crate::grid::Grid;
use shared::{
    hue_bucket, hue_for_index, BoidsScope, Boundary, InteractionMode, SimParams, VideoSource,
    NUM_COLORS,
};
use macroquad::prelude::{Image, Texture2D, Vec2};
use rand::Rng;
use rayon::prelude::*;

#[derive(Clone, Copy)]
pub struct Particle {
    pub pos: Vec2,
    pub vel: Vec2,
    /// Matiz continuo en [0,1) que se muestra/usa.
    pub hue: f32,
    /// Matiz objetivo hacia el que transita `hue` (para cambios suaves).
    pub target_hue: f32,
}

/// Foto activa del efecto en dos fases: (A) las partículas se acomodan a una
/// rejilla que cubre la imagen y toman su color (`color_at` muestrea los
/// píxeles); (B) la textura se superpone encima con un fundido (`tex`).
pub struct Photo {
    pub tex: Texture2D,
    pub(crate) bytes: Vec<u8>,
    pub(crate) w: usize,
    pub(crate) h: usize,
    /// Centro de la caja de la foto (mundo).
    pub center: Vec2,
    /// Tamaño de la caja de la foto (mundo).
    pub extent: Vec2,
    /// Ruta del vídeo, si la foto es un vídeo (`None` = imagen fija). Se guarda
    /// para arrancar el streaming DIFERIDO: la reproducción no empieza hasta que
    /// la imagen se ha formado del todo (overlay revelado).
    video_path: Option<String>,
    /// Fuente de fotogramas activa (solo tras arrancar la reproducción). Al
    /// avanzar (`advance_video`) se sube el fotograma actual a `tex`/`bytes`; al
    /// soltar se congela (`frozen`) en el frame visible.
    video: Option<VideoSource>,
    frozen: bool,
}

impl Photo {
    /// Esquina superior izquierda de la caja (mundo), para dibujar la textura.
    pub fn origin(&self) -> Vec2 {
        self.center - self.extent * 0.5
    }

    /// Sustituye el fotograma (misma resolución): actualiza la textura GPU y
    /// los bytes que muestrea el mosaico (`color_at`).
    fn update_frame(&mut self, bytes: Vec<u8>) {
        if bytes.len() != self.w * self.h * 4 {
            return;
        }
        let img = Image { bytes, width: self.w as u16, height: self.h as u16 };
        self.tex.update(&img);
        self.bytes = img.bytes;
    }

    /// Color RGB [0,1] de la foto en la posición de mundo `p`, o `None` si `p`
    /// cae fuera de la caja. La `v` va invertida porque la cámara del CPU
    /// (from_display_rect) tiene la Y al revés (igual criterio que la silueta
    /// con `flip_y=true` y la textura superpuesta).
    pub fn color_at(&self, p: Vec2) -> Option<[f32; 3]> {
        let u = (p.x - self.center.x) / self.extent.x + 0.5;
        let v = 0.5 - (p.y - self.center.y) / self.extent.y;
        if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
            return None;
        }
        let px = ((u * self.w as f32) as usize).min(self.w - 1);
        let py = ((v * self.h as f32) as usize).min(self.h - 1);
        let idx = (py * self.w + px) * 4;
        Some([
            self.bytes[idx] as f32 / 255.0,
            self.bytes[idx + 1] as f32 / 255.0,
            self.bytes[idx + 2] as f32 / 255.0,
        ])
    }
}

pub struct Simulation {
    pub particles: Vec<Particle>,
    pub world: Vec2,
    /// Punto hacia el que se atraen las zonas activas (centro de la vista).
    pub focus: Vec2,
    /// Posición (mundo) del cursor cuando la herramienta Fuerza está activa.
    pub pointer: Option<Vec2>,
    /// Puntos meta (mundo) que forman un texto/imagen. `None` = sin forma.
    pub shape: Option<Vec<Vec2>>,
    /// Mezcla 0..1 de la forma: 0 = las partículas siguen su animación, 1 = la
    /// forma está totalmente aplicada. Sube al aplicar y baja al soltar para una
    /// aparición/disolución fluida (ver `advance_shape`).
    pub shape_blend: f32,
    /// Objetivo de `shape_blend` (1 mientras hay forma, 0 al soltarla).
    shape_target: f32,
    /// Foto activa del efecto en dos fases (modo recrear colores). Fase A usa
    /// el sistema de forma (`shape`) para acomodar + colorear las partículas;
    /// fase B funde la textura encima con `overlay_reveal`. `None` = sin foto.
    pub photo: Option<Photo>,
    /// Mezcla 0..1 de la superposición (fase B) y su objetivo.
    pub overlay_reveal: f32,
    overlay_target: f32,
    /// En salida: la imagen se desvanece primero y luego se sueltan las
    /// partículas (transición de entrada en reverso).
    photo_releasing: bool,
    grid: Grid,
    /// Aceleraciones acumuladas por partícula (scratch reutilizado).
    forces: Vec<Vec2>,
}

impl Simulation {
    pub fn new(world: Vec2) -> Self {
        Self {
            particles: Vec::new(),
            world,
            focus: world * 0.5,
            pointer: None,
            shape: None,
            shape_blend: 0.0,
            shape_target: 0.0,
            photo: None,
            overlay_reveal: 0.0,
            overlay_target: 0.0,
            photo_releasing: false,
            grid: Grid::new(),
            forces: Vec::new(),
        }
    }

    pub fn clear(&mut self) {
        self.particles.clear();
    }

    /// Fija una forma (nube de puntos meta) hacia la que se agrupan las
    /// partículas, apareciendo de forma fluida (la mezcla sube desde 0). Vacía =
    /// se ignora.
    pub fn set_shape(&mut self, targets: Vec<Vec2>) {
        if targets.is_empty() {
            self.clear_shape();
        } else {
            self.shape = Some(targets);
            self.shape_blend = 0.0;
            self.shape_target = 1.0;
        }
    }

    /// Reemplaza los puntos meta de la forma SIN reiniciar la animación de
    /// aparición (`shape_blend`/`shape_target` no se tocan). Pensado para el
    /// efecto foto/vídeo: al reconstruir el mosaico con el fotograma actual
    /// (en vez de solo el primero), las partículas ya formadas se retargetan
    /// suavemente (por el resorte `shape_k`) hacia la nueva posición en vez de
    /// rehacer la formación desde cero. No-op si `targets` está vacío (se
    /// conserva la forma anterior en vez de soltarla, a diferencia de
    /// `set_shape`).
    pub fn retarget_shape(&mut self, targets: Vec<Vec2>) {
        if !targets.is_empty() {
            self.shape = Some(targets);
        }
    }

    /// Fija la foto (textura + píxeles `bytes` de `w`×`h`) del efecto: encuadra
    /// al 90% del lienzo, centrada y con el aspecto preservado. La fase A
    /// (acomodar + colorear) la arranca `set_shape` con la rejilla; la fase B
    /// (superposición) empieza en 0.
    pub fn set_photo(&mut self, tex: Texture2D, bytes: Vec<u8>, w: usize, h: usize) {
        if w == 0 || h == 0 {
            return;
        }
        let (iw, ih) = (w as f32, h as f32);
        let scale = (self.world.x * 0.9 / iw).min(self.world.y * 0.9 / ih);
        self.photo = Some(Photo {
            tex,
            bytes,
            w,
            h,
            center: self.world * 0.5,
            extent: Vec2::new(iw * scale, ih * scale),
            video_path: None,
            video: None,
            frozen: false,
        });
        self.overlay_reveal = 0.0;
        self.overlay_target = 0.0;
        self.photo_releasing = false;
    }

    /// Marca la foto ya fijada (`set_photo`) como un vídeo cuya reproducción se
    /// arrancará de forma diferida (ver `advance_video`).
    pub fn set_video_path(&mut self, path: String) {
        if let Some(p) = self.photo.as_mut() {
            p.video_path = Some(path);
            p.video = None;
            p.frozen = false;
        }
    }

    /// Avanza el vídeo de la foto (si lo es) `dt` segundos. Arranca la
    /// reproducción SOLO cuando la imagen ya está formada del todo (overlay
    /// revelado), para que empiece desde el primer fotograma justo al aparecer
    /// nítida. Sube el fotograma actual; cuando el vídeo termina (se reprodujo
    /// una vez) dispara la salida en reverso. Al soltar se congela en el frame
    /// visible. Devuelve `true` justo en el frame en que arranca la
    /// reproducción (para marcar el offset de audio al grabar).
    pub fn advance_video(&mut self, dt: f32, key_black: bool) -> bool {
        let releasing = self.photo_releasing;
        let formed = self.overlay_reveal >= 0.99;
        let mut ended = false;
        let mut just_started = false;
        {
            let Some(p) = self.photo.as_mut() else { return false };
            if p.video_path.is_none() {
                return false; // imagen fija
            }
            if releasing {
                p.frozen = true;
            }
            if p.frozen {
                return false;
            }
            // Arranque diferido: abre el streaming al terminar de formarse.
            if p.video.is_none() && formed {
                if let Some(path) = p.video_path.as_deref() {
                    // Antes 720 (se veía borroso con clips de más calidad,
                    // p. ej. Manim en 2K); 1440 cubre grabaciones "2K".
                    p.video = VideoSource::open(path, 1440, key_black);
                    just_started = p.video.is_some();
                }
            }
            if p.video.is_some() {
                let new = p.video.as_mut().and_then(|v| v.advance(dt));
                ended = p.video.as_ref().map(|v| v.ended()).unwrap_or(false);
                if let Some(bytes) = new {
                    p.update_frame(bytes);
                }
            }
        }
        // Reproducido una vez → salir (reverso): la imagen se desvanece y luego
        // se sueltan las partículas.
        if ended && !self.photo_releasing {
            self.photo_releasing = true;
        }
        just_started
    }

    /// Ruta del vídeo activo (para muxear su audio en la grabación), si la foto
    /// es un vídeo y ya está reproduciéndose.
    pub fn video_path(&self) -> Option<&str> {
        self.photo.as_ref().and_then(|p| p.video_path.as_deref())
    }

    /// Suelta la foto en REVERSO: primero se desvanece la imagen (dejando ver
    /// el mosaico de partículas detrás) y luego se sueltan las partículas (lo
    /// gestiona `advance_photo_effect`).
    pub fn clear_photo(&mut self) {
        if self.photo.is_some() {
            self.photo_releasing = true;
        }
    }

    /// Descarta la foto de golpe (cuando una forma nueva la reemplaza).
    pub fn drop_photo(&mut self) {
        self.photo = None;
        self.overlay_reveal = 0.0;
        self.overlay_target = 0.0;
        self.photo_releasing = false;
    }

    /// Secuencia el efecto foto y avanza la superposición `duration` s.
    /// Entrada: la imagen se funde tras acomodarse las partículas. Salida
    /// (reverso): la imagen se va primero y, cuando ya no se ve, se sueltan las
    /// partículas; al terminar, descarta la foto.
    pub fn advance_photo_effect(&mut self, dt: f32, duration: f32) {
        if self.photo.is_none() {
            return;
        }
        if self.photo_releasing {
            self.overlay_target = 0.0;
            if self.overlay_reveal <= 1e-3 {
                self.shape_target = 0.0; // ahora sí, soltar las partículas
            }
        } else if self.shape.is_some() && self.shape_blend >= 0.95 {
            self.overlay_target = 1.0;
        }
        let step = if duration > 1e-3 { dt / duration } else { 1.0 };
        if self.overlay_reveal < self.overlay_target {
            self.overlay_reveal = (self.overlay_reveal + step).min(self.overlay_target);
        } else if self.overlay_reveal > self.overlay_target {
            self.overlay_reveal = (self.overlay_reveal - step).max(self.overlay_target);
        }
        if self.photo_releasing && self.overlay_reveal <= 1e-4 && self.shape.is_none() {
            self.photo = None;
            self.photo_releasing = false;
        }
    }

    /// Superposición (fase B) ya suavizada (ease-in-out) para el render.
    pub fn overlay_ease(&self) -> f32 {
        let r = self.overlay_reveal.clamp(0.0, 1.0);
        r * r * (3.0 - 2.0 * r)
    }

    /// Mezcla de aparición de la forma ya suavizada (ease-in-out): el color del
    /// mosaico (fase A) sigue esta curva a la vez que la posición.
    pub fn shape_ease(&self) -> f32 {
        let b = self.shape_blend.clamp(0.0, 1.0);
        b * b * (3.0 - 2.0 * b)
    }

    /// Suelta la forma de forma fluida: la mezcla baja a 0 y, al llegar, se
    /// descartan los puntos meta (ver `advance_shape`).
    pub fn clear_shape(&mut self) {
        self.shape_target = 0.0;
        if self.shape_blend <= 1e-4 {
            self.shape = None;
        }
    }

    /// Avanza la mezcla de la forma hacia su objetivo durante `duration` s
    /// (0 = instantáneo). Al terminar de disolverse, descarta los puntos meta.
    pub fn advance_shape(&mut self, dt: f32, duration: f32) {
        let step = if duration > 1e-3 { dt / duration } else { 1.0 };
        if self.shape_blend < self.shape_target {
            self.shape_blend = (self.shape_blend + step).min(self.shape_target);
        } else if self.shape_blend > self.shape_target {
            self.shape_blend = (self.shape_blend - step).max(self.shape_target);
            if self.shape_blend <= 1e-4 {
                self.shape = None;
            }
        }
    }

    /// Tiñe del matiz indicado solo las partículas que forman la forma actual
    /// (las primeras `shape.len()`), dejando el resto con su color.
    pub fn tint_shape(&mut self, hue: f32) {
        let n = self.shape.as_ref().map_or(0, |t| t.len());
        for p in self.particles.iter_mut().take(n) {
            p.hue = hue;
            p.target_hue = hue;
        }
    }

    /// Llena el lienzo con `n` partículas de posición y color aleatorios.
    pub fn spawn_random(&mut self, n: usize, rng: &mut impl Rng) {
        self.particles.reserve(n);
        for _ in 0..n {
            let hue = hue_for_index(rng.gen_range(0..NUM_COLORS));
            self.particles.push(Particle {
                pos: Vec2::new(
                    rng.gen_range(0.0..self.world.x.max(1.0)),
                    rng.gen_range(0.0..self.world.y.max(1.0)),
                ),
                vel: Vec2::ZERO,
                hue,
                target_hue: hue,
            });
        }
    }

    pub fn add(&mut self, pos: Vec2, hue: f32) {
        self.particles.push(Particle {
            pos,
            vel: Vec2::ZERO,
            hue,
            target_hue: hue,
        });
    }

    /// Aplica los comportamientos dinámicos opcionales (cambio de color
    /// aleatorio y deriva gradual de color y atracción). Se llama una vez por
    /// paso de simulación.
    pub fn apply_dynamics(&mut self, params: &mut SimParams, rng: &mut impl Rng, frame_seconds: f32) {
        let dt = params.time_scale.max(0.0);
        let smooth = params.color_smooth;

        // Saltos de color aleatorios: con `color_smooth` solo fijan el objetivo
        // (el matiz transita hacia él); si no, cambian el color al instante.
        if params.random_color {
            let p_switch = (params.random_color_rate * dt).clamp(0.0, 1.0);
            for part in &mut self.particles {
                if rng.gen::<f32>() < p_switch {
                    let nh = hue_for_index(rng.gen_range(0..NUM_COLORS));
                    part.target_hue = nh;
                    if !smooth {
                        part.hue = nh;
                    }
                }
            }
        }

        if params.gradual {
            let cs = params.gradual_color_speed * dt;
            for part in &mut self.particles {
                if smooth {
                    part.target_hue = (part.target_hue + rng.gen_range(-1.0..=1.0) * cs).rem_euclid(1.0);
                } else {
                    part.hue = (part.hue + rng.gen_range(-1.0..=1.0) * cs).rem_euclid(1.0);
                    part.target_hue = part.hue;
                }
            }
            let ms = params.gradual_matrix_speed * dt;
            for i in 0..NUM_COLORS {
                for j in 0..NUM_COLORS {
                    let drift = rng.gen_range(-1.0..=1.0) * ms;
                    params.matrix[i][j] = (params.matrix[i][j] + drift).clamp(-1.0, 1.0);
                }
            }
        }

        // Suavizado: acerca cada matiz a su objetivo en tiempo real.
        if smooth {
            let t = (frame_seconds / params.color_transition_duration.max(0.05)).clamp(0.0, 1.0);
            for part in &mut self.particles {
                part.hue = lerp_hue(part.hue, part.target_hue, t);
            }
        }
    }

    /// Borra todas las partículas dentro de `radius` de `pos`.
    pub fn erase_near(&mut self, pos: Vec2, radius: f32) {
        let r2 = radius * radius;
        self.particles
            .retain(|p| (p.pos - pos).length_squared() > r2);
    }

    /// Avanza un paso de física.
    pub fn step(&mut self, params: &SimParams) {
        let n = self.particles.len();
        if n == 0 {
            return;
        }

        self.grid.rebuild(&self.particles, self.world, params.r_max);

        let wrap = params.boundary == Boundary::Wrap;
        let r_max = params.r_max;
        let r_max2 = r_max * r_max;
        let inv_r_max = 1.0 / r_max;
        let beta = params.beta;
        let world = self.world;
        let half = world * 0.5;
        // Recentrado de zonas activas: atracción hacia `focus` proporcional a la
        // densidad local (nº de vecinos), para desapilar los grumos lejanos.
        let attract = params.attract_active;
        let attract_strength = params.attract_active_strength;
        let focus = self.focus;

        // Anti-aglomeración: umbral de vecinos por encima del cual la bola se
        // considera hiperdensa. Relativo a la densidad media (vecinos esperados
        // dentro de r_max con las partículas repartidas), así se adapta al nº
        // de partículas y al radio. Infinito = desactivado.
        let clump_thr = if params.anti_clump {
            let expected =
                n as f32 / (world.x * world.y).max(1.0) * std::f32::consts::PI * r_max2;
            (params.anti_clump_factor.max(1.0) * expected).max(30.0)
        } else {
            f32::INFINITY
        };
        let clump_strength = params.anti_clump_strength;

        // Fuerza del cursor (herramienta Fuerza): atrae o repele alrededor del
        // puntero con caída suave dentro de `pointer_radius`.
        let pointer = self.pointer;
        let ptr_radius = params.pointer_radius.max(1.0);
        let ptr_radius2 = ptr_radius * ptr_radius;
        let ptr_sign = if params.pointer_repel { -1.0 } else { 1.0 };
        let ptr_gain = params.pointer_strength * 6.0;

        // Forma (texto/imagen): solo las PRIMERAS `n_shape` partículas se agrupan
        // en la forma (en el centro); el resto sigue con la animación del modo.
        // Cada partícula de la forma recibe un resorte hacia su punto meta más una
        // pizca de interacción residual (`shape_inter`) para un movimiento
        // orgánico y tranquilo que deje leer el texto. La "fijación" sube la
        // rigidez del resorte y baja la interacción residual.
        let shape = self.shape.as_deref();
        let n_shape = shape.map_or(0, |t| t.len());
        // Mezcla suavizada (ease-in/out) para que la forma aparezca y se disuelva
        // de manera fluida en vez de aparecer de golpe.
        let sb = self.shape_blend.clamp(0.0, 1.0);
        let shape_blend = sb * sb * (3.0 - 2.0 * sb);
        let shape_on = n_shape > 0 && shape_blend > 1e-3;
        let shape_fix = params.shape_strength.clamp(0.0, 1.0);
        // El resorte y la evasión crecen con la mezcla: así las partículas fluyen
        // hacia la forma en lugar de ser tironeadas de inmediato.
        let shape_k = (0.02 + shape_fix * 0.38) * shape_blend; // 0..0.4
        // Interacción residual: plena al inicio (blend 0) y baja según la fijación
        // cuando la forma ya está aplicada, para un texto legible pero orgánico.
        let shape_inter = 1.0 - shape_blend * (1.0 - 0.35 * (1.0 - shape_fix));
        // El texto/figura y el fondo se ignoran entre sí; además el fondo REPELE
        // a la figura (para no invadirla). Ganancia de esa evasión (crece con la
        // mezcla). También aplica en modo foto: el enjambre choca/rodea la
        // imagen, no la atraviesa.
        let shape_avoid = params.shape_avoid_gain.max(0.0) * shape_blend;

        // Bandada (Boids): física vectorial que sustituye a la fuerza radial.
        // Como Boids no usa el coeficiente escalar, no puede mezclarse con el
        // sistema de blend del `coef`. En su lugar cruzamos los DOS modelos de
        // fuerza con un factor global `boids_mix` (0 = radial, 1 = bandada) que
        // sigue el mismo `blend`/ease que `interaction()`. Así una transición o
        // un morph de escena hacia/desde la bandada respeta su duración: la
        // fuerza radial se desvanece (vía `interaction`, cuyo coef objetivo es 0
        // en Boids) mientras la bandada aparece, y viceversa.
        let boids_mix = {
            let to_boids = if params.mode == InteractionMode::Boids { 1.0 } else { 0.0 };
            // Solo el blend gobierna (como `interaction()`): así el cruce
            // forzado de `start_matrix_blend` también queda cubierto.
            if params.blend < 1.0 {
                let from_boids =
                    if params.from_state.mode == InteractionMode::Boids { 1.0 } else { 0.0 };
                let b = params.blend;
                let t = b * b * (3.0 - 2.0 * b); // mismo ease que `interaction()`
                from_boids + (to_boids - from_boids) * t
            } else {
                to_boids
            }
        };
        let need_boids = boids_mix > 0.0;
        let need_radial = boids_mix < 1.0;
        let scope = params.boids_scope;
        let w_sep = params.boids_separation;
        let w_ali = params.boids_alignment;
        let w_coh = params.boids_cohesion;
        let sep_r = (params.boids_sep_radius * r_max).max(1.0);
        let sep_r2 = sep_r * sep_r;
        // Evasión entre grupos (repulsión de otros colores), solo en Híbrido/Por
        // color; en "Todas" no hay grupos distintos que esquivar.
        let group_avoid = !matches!(scope, BoidsScope::All);
        let w_grp = params.boids_group_avoid;
        // Esquive de paredes (solo bandada + borde de rebote): en lugar de
        // rebotar como una pelota, los "pájaros" giran su vector al acercarse.
        let wall_avoid = need_boids && !wrap;
        let wall_margin = r_max; // distancia al borde a la que empieza el giro
        let wall_turn = params.boids_cruise.max(1.0) * 1.5; // fuerza del giro

        let mut forces = std::mem::take(&mut self.forces);
        forces.clear();
        forces.resize(n, Vec2::ZERO);

        // --- Cálculo de fuerzas en paralelo (los 16 hilos) ---
        // Cada partícula escribe solo su propia `forces[i]`; el resto es lectura
        // compartida, así que no hay condiciones de carrera.
        {
            let particles = &self.particles;
            let grid = &self.grid;
            let cols = grid.cols();
            let rows = grid.rows();

            forces.par_iter_mut().enumerate().for_each(|(i, out)| {
                let pi = particles[i];
                let (cx, cy) = grid.cell_coord(pi.pos);
                let i_text = shape_on && i < n_shape;
                let mut acc = Vec2::ZERO;
                let mut neighbors = 0u32;
                // Suma de los vectores a los vecinos: apunta al centro de masa
                // local (para la dispersión anti-aglomeración).
                let mut crowd = Vec2::ZERO;
                // Acumuladores de Boids (solo se usan si `need_boids`).
                let mut sep_acc = Vec2::ZERO;
                let mut ali_acc = Vec2::ZERO;
                let mut coh_acc = Vec2::ZERO;
                let mut grp_acc = Vec2::ZERO;
                let mut flock_n = 0u32;

                for dy in -1..=1 {
                    for dx in -1..=1 {
                        let (nx, ny) = if wrap {
                            ((cx + dx).rem_euclid(cols), (cy + dy).rem_euclid(rows))
                        } else {
                            let nx = cx + dx;
                            let ny = cy + dy;
                            if nx < 0 || nx >= cols || ny < 0 || ny >= rows {
                                continue;
                            }
                            (nx, ny)
                        };

                        for &j in grid.cell_items(nx, ny) {
                            let j = j as usize;
                            if j == i {
                                continue;
                            }
                            let pj = particles[j];
                            let mut d = pj.pos - pi.pos;
                            if wrap {
                                // Imagen mínima: distancia más corta por el toro.
                                if d.x > half.x {
                                    d.x -= world.x;
                                } else if d.x < -half.x {
                                    d.x += world.x;
                                }
                                if d.y > half.y {
                                    d.y -= world.y;
                                } else if d.y < -half.y {
                                    d.y += world.y;
                                }
                            }
                            // Rechazo barato sin sqrt para los que quedan fuera.
                            let d2 = d.length_squared();
                            if d2 > r_max2 || d2 < 1e-8 {
                                continue;
                            }
                            neighbors += 1;
                            crowd += d;
                            let dist = d2.sqrt();
                            // Interacción texto ↔ fondo: se ignoran mutuamente y el
                            // fondo esquiva al texto para no invadir las letras.
                            if shape_on {
                                let j_text = j < n_shape;
                                if i_text {
                                    // El texto ignora al fondo (solo se relaciona con
                                    // el propio texto).
                                    if !j_text {
                                        continue;
                                    }
                                } else if j_text {
                                    // El fondo repele al texto y no lo toma como
                                    // vecino para su física normal.
                                    let push = 1.0 - dist * inv_r_max;
                                    acc -= d * (push * shape_avoid / dist);
                                    continue;
                                }
                            }
                            // Durante una transición pueden correr ambos modelos a
                            // la vez (se combinan luego con `boids_mix`).
                            if need_boids {
                                let same = hue_bucket(pi.hue) == hue_bucket(pj.hue);
                                let sep_ok = !matches!(scope, BoidsScope::SameColor) || same;
                                let flock_ok = matches!(scope, BoidsScope::All) || same;
                                // Separación: huir de los vecinos muy cercanos,
                                // más fuerte cuanto menor la distancia.
                                if sep_ok && d2 < sep_r2 {
                                    sep_acc -= d * ((sep_r - dist) / (sep_r * dist));
                                }
                                // Alineación + cohesión con los vecinos de bandada.
                                if flock_ok {
                                    ali_acc += pj.vel;
                                    coh_acc += d;
                                    flock_n += 1;
                                }
                                // Evasión de otros grupos: repulsión de los vecinos
                                // de distinto color en todo el radio de percepción,
                                // con caída lineal (más fuerte cuanto más cerca).
                                if group_avoid && !same {
                                    grp_acc -= d * ((1.0 - dist * inv_r_max) / dist);
                                }
                            }
                            if need_radial {
                                let r = dist * inv_r_max;
                                let coef = params.interaction(pi.hue, pj.hue);
                                let f = force_fn(r, coef, beta);
                                acc += d * (f / dist);
                            }
                        }
                    }
                }

                // Anti-aglomeración: con muchos más vecinos que la media, la
                // bola es hiperdensa y las fuerzas de pareja se vuelven
                // violentas. Se calman (damping) y se añade un empuje suave
                // hacia fuera del centro local: la bola se disuelve desde el
                // borde, de forma natural, sin teletransportes.
                if (neighbors as f32) > clump_thr {
                    let over = ((neighbors as f32 - clump_thr) / clump_thr).min(1.0);
                    acc *= 1.0 - 0.7 * over;
                    let len = crowd.length();
                    if len > 1e-4 {
                        acc -= (crowd / len) * (over * clump_strength);
                    }
                }

                // Composición de las tres reglas de Boids en un acumulador aparte
                // que luego se mezcla con la parte radial según `boids_mix`.
                if need_boids {
                    let mut b = sep_acc * w_sep + grp_acc * w_grp;
                    if flock_n > 0 {
                        let inv = 1.0 / flock_n as f32;
                        // Alineación: dirigir la velocidad hacia la media local.
                        b += (ali_acc * inv - pi.vel) * w_ali;
                        // Cohesión: hacia el centro de masa (normalizado por r_max
                        // para que el peso sea comparable a las otras reglas).
                        b += (coh_acc * (inv * inv_r_max)) * w_coh;
                    }

                    // Giro para esquivar las paredes: empuje hacia el interior que
                    // crece al acercarse al borde. Con el crucero manteniendo la
                    // rapidez, esto rota el vector de vuelo (esquiva, no rebota).
                    if wall_avoid {
                        let p = pi.pos;
                        if p.x < wall_margin {
                            b.x += wall_turn * (1.0 - p.x / wall_margin);
                        } else if p.x > world.x - wall_margin {
                            b.x -= wall_turn * (1.0 - (world.x - p.x) / wall_margin);
                        }
                        if p.y < wall_margin {
                            b.y += wall_turn * (1.0 - p.y / wall_margin);
                        } else if p.y > world.y - wall_margin {
                            b.y -= wall_turn * (1.0 - (world.y - p.y) / wall_margin);
                        }
                    }
                    acc += b * boids_mix;
                }

                // Atracción leve al centro para las zonas con mucha actividad
                // (densidad alta). Las partículas dispersas casi no se enteran.
                if attract {
                    let mut toward = focus - pi.pos;
                    if wrap {
                        // Imagen mínima: tira por el camino más corto del toro.
                        if toward.x > half.x {
                            toward.x -= world.x;
                        } else if toward.x < -half.x {
                            toward.x += world.x;
                        }
                        if toward.y > half.y {
                            toward.y -= world.y;
                        } else if toward.y < -half.y {
                            toward.y += world.y;
                        }
                    }
                    let d = toward.length();
                    if d > 1.0 {
                        // Densidad normalizada: ~0 para solitarias, satura en 1.
                        let activity = (neighbors as f32 / 30.0).min(1.0);
                        acc += (toward / d) * (attract_strength * activity);
                    }
                }

                // Fuerza del cursor: atrae/repele las partículas cercanas al
                // puntero, con caída lineal hasta `ptr_radius`.
                if let Some(ptr) = pointer {
                    let mut toward = ptr - pi.pos;
                    if wrap {
                        if toward.x > half.x {
                            toward.x -= world.x;
                        } else if toward.x < -half.x {
                            toward.x += world.x;
                        }
                        if toward.y > half.y {
                            toward.y -= world.y;
                        } else if toward.y < -half.y {
                            toward.y += world.y;
                        }
                    }
                    let d2p = toward.length_squared();
                    if d2p < ptr_radius2 && d2p > 1e-6 {
                        let d = d2p.sqrt();
                        let falloff = 1.0 - d / ptr_radius;
                        acc += (toward / d) * (ptr_sign * ptr_gain * falloff);
                    }
                }

                // Forma: solo las partículas asignadas (i < n_shape) van al texto;
                // el resto conserva su `acc` normal (siguen la animación).
                if i < n_shape {
                    let tgt = shape.unwrap()[i];
                    let mut pull = tgt - pi.pos;
                    if wrap {
                        if pull.x > half.x {
                            pull.x -= world.x;
                        } else if pull.x < -half.x {
                            pull.x += world.x;
                        }
                        if pull.y > half.y {
                            pull.y -= world.y;
                        } else if pull.y < -half.y {
                            pull.y += world.y;
                        }
                    }
                    acc = acc * shape_inter + pull * shape_k;
                }
                *out = acc;
            });
        }

        // --- Integración en paralelo ---
        let dt = params.time_scale;
        let friction = params.friction;
        let force_gain = params.force;
        let boundary = params.boundary;
        // Límite de velocidad de seguridad: evita que un pico de fuerza mande
        // una partícula disparada a través de toda la pantalla.
        let max_speed = r_max;
        // Velocidad de crucero (bandada): rapidez mínima para que no se detenga.
        // Escalada por `boids_mix` (transición). No se aplica a las partículas de
        // la forma (deben asentarse), lo que se decide por índice en la integración.
        let cruise = params.boids_cruise * boids_mix;
        // Durante la transición (mix>0) usamos el deslizamiento en las paredes en
        // vez del rebote elástico.
        let boids_bounce = need_boids;

        self.particles
            .par_iter_mut()
            .enumerate()
            .zip(forces.par_iter())
            .for_each(|((i, p), &f)| {
                p.vel = p.vel * friction + f * force_gain * dt;
                let speed = p.vel.length();
                if speed > max_speed {
                    p.vel *= max_speed / speed;
                }
                // Crucero: mantener una rapidez mínima (murmuración que no se para).
                // No se aplica a las partículas de la forma (deben asentarse).
                if cruise > 0.0 && i >= n_shape {
                    let speed = p.vel.length();
                    if speed > 1e-4 {
                        if speed < cruise {
                            p.vel *= cruise / speed;
                        }
                    } else {
                        // En reposo: darle una dirección pseudoaleatoria estable
                        // (derivada de la posición) para que arranque el vuelo.
                        let a = p.pos.x * 12.9898 + p.pos.y * 78.233;
                        p.vel = Vec2::new(a.cos(), a.sin()) * cruise;
                    }
                }
                p.pos += p.vel * dt;

                match boundary {
                    Boundary::Wrap => {
                        if p.pos.x < 0.0 {
                            p.pos.x += world.x;
                        } else if p.pos.x >= world.x {
                            p.pos.x -= world.x;
                        }
                        if p.pos.y < 0.0 {
                            p.pos.y += world.y;
                        } else if p.pos.y >= world.y {
                            p.pos.y -= world.y;
                        }
                    }
                    Boundary::Bounce if boids_bounce => {
                        // Bandada: si un pájaro alcanza la pared, desliza a lo largo
                        // de ella (anula solo la componente hacia fuera) en vez de
                        // rebotar; el giro ya lo estaba curvando hacia el interior.
                        if p.pos.x < 0.0 {
                            p.pos.x = 0.0;
                            p.vel.x = p.vel.x.max(0.0);
                        } else if p.pos.x > world.x {
                            p.pos.x = world.x;
                            p.vel.x = p.vel.x.min(0.0);
                        }
                        if p.pos.y < 0.0 {
                            p.pos.y = 0.0;
                            p.vel.y = p.vel.y.max(0.0);
                        } else if p.pos.y > world.y {
                            p.pos.y = world.y;
                            p.vel.y = p.vel.y.min(0.0);
                        }
                    }
                    Boundary::Bounce => {
                        if p.pos.x < 0.0 {
                            p.pos.x = 0.0;
                            p.vel.x = -p.vel.x * 0.5;
                        } else if p.pos.x > world.x {
                            p.pos.x = world.x;
                            p.vel.x = -p.vel.x * 0.5;
                        }
                        if p.pos.y < 0.0 {
                            p.pos.y = 0.0;
                            p.vel.y = -p.vel.y * 0.5;
                        } else if p.pos.y > world.y {
                            p.pos.y = world.y;
                            p.vel.y = -p.vel.y * 0.5;
                        }
                    }
                }
            });

        self.forces = forces;
    }
}

/// Perfil de fuerza estilo "particle life".
///
/// - `r` es la distancia normalizada en [0, 1] (= dist / r_max).
/// - Para `r < beta` hay repulsión dura independiente del color (evita que las
///   partículas se apilen).
/// - Para `beta <= r <= 1` la fuerza es un triángulo escalado por `coef`
///   (positivo = atracción, negativo = repulsión).
/// Interpola el matiz `from` hacia `to` por la fracción `t`, tomando siempre
/// el camino más corto en la rueda de color (0 y 1 son el mismo punto).
#[inline]
fn lerp_hue(from: f32, to: f32, t: f32) -> f32 {
    let d = (to - from + 0.5).rem_euclid(1.0) - 0.5; // diferencia con signo en [-0.5, 0.5)
    (from + d * t).rem_euclid(1.0)
}

#[inline]
fn force_fn(r: f32, coef: f32, beta: f32) -> f32 {
    if r < beta {
        r / beta - 1.0
    } else {
        let peak = 1.0 - (2.0 * r - 1.0 - beta).abs() / (1.0 - beta);
        coef * peak
    }
}

#[cfg(test)]
mod bench {
    use super::*;
    use shared::SimParams;
    use std::time::Instant;

    #[test]
    fn throughput() {
        let world = Vec2::new(1600.0, 1000.0);
        let mut sim = Simulation::new(world);
        let mut rng = rand::thread_rng();
        let params = SimParams::default();
        for &n in &[5_000usize, 20_000, 50_000] {
            sim.clear();
            sim.spawn_random(n, &mut rng);
            for _ in 0..5 {
                sim.step(&params); // warmup
            }
            let iters = 60;
            let t = Instant::now();
            for _ in 0..iters {
                sim.step(&params);
            }
            let per = t.elapsed().as_secs_f64() / iters as f64;
            println!(
                "N={n:>6}  {:>6.2} ms/step  -> {:>5.0} pasos/s",
                per * 1000.0,
                1.0 / per
            );
        }
    }
}
