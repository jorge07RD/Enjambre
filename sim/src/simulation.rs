use crate::grid::Grid;
use shared::{hue_bucket, hue_for_index, BoidsScope, Boundary, InteractionMode, SimParams, NUM_COLORS};
use macroquad::prelude::Vec2;
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

pub struct Simulation {
    pub particles: Vec<Particle>,
    pub world: Vec2,
    /// Punto hacia el que se atraen las zonas activas (centro de la vista).
    pub focus: Vec2,
    /// Posición (mundo) del cursor cuando la herramienta Fuerza está activa.
    pub pointer: Option<Vec2>,
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
            grid: Grid::new(),
            forces: Vec::new(),
        }
    }

    pub fn clear(&mut self) {
        self.particles.clear();
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

        // Fuerza del cursor (herramienta Fuerza): atrae o repele alrededor del
        // puntero con caída suave dentro de `pointer_radius`.
        let pointer = self.pointer;
        let ptr_radius = params.pointer_radius.max(1.0);
        let ptr_radius2 = ptr_radius * ptr_radius;
        let ptr_sign = if params.pointer_repel { -1.0 } else { 1.0 };
        let ptr_gain = params.pointer_strength * 6.0;

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
            if params.smooth && params.blend < 1.0 {
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
                let mut acc = Vec2::ZERO;
                let mut neighbors = 0u32;
                // Acumuladores de Boids (solo se usan si `need_boids`).
                let mut sep_acc = Vec2::ZERO;
                let mut ali_acc = Vec2::ZERO;
                let mut coh_acc = Vec2::ZERO;
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
                            let dist = d2.sqrt();
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

                // Composición de las tres reglas de Boids en un acumulador aparte
                // que luego se mezcla con la parte radial según `boids_mix`.
                if need_boids {
                    let mut b = sep_acc * w_sep;
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
        // Escalada por `boids_mix` para que aparezca/desaparezca con la transición.
        let cruise = params.boids_cruise * boids_mix;
        // Durante la transición (mix>0) usamos el deslizamiento en las paredes en
        // vez del rebote elástico.
        let boids_bounce = need_boids;

        self.particles
            .par_iter_mut()
            .zip(forces.par_iter())
            .for_each(|(p, &f)| {
                p.vel = p.vel * friction + f * force_gain * dt;
                let speed = p.vel.length();
                if speed > max_speed {
                    p.vel *= max_speed / speed;
                }
                // Crucero: mantener una rapidez mínima (murmuración que no se para).
                if cruise > 0.0 {
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
