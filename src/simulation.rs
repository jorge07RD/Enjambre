use crate::config::{hue_for_index, Boundary, SimParams, NUM_COLORS};
use crate::grid::Grid;
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
    grid: Grid,
    /// Aceleraciones acumuladas por partícula (scratch reutilizado).
    forces: Vec<Vec2>,
}

impl Simulation {
    pub fn new(world: Vec2) -> Self {
        Self {
            particles: Vec::new(),
            world,
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
                            let dist = d2.sqrt();
                            let r = dist * inv_r_max;
                            let coef = params.interaction(pi.hue, pj.hue);
                            let f = force_fn(r, coef, beta);
                            acc += d * (f / dist);
                        }
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

        self.particles
            .par_iter_mut()
            .zip(forces.par_iter())
            .for_each(|(p, &f)| {
                p.vel = p.vel * friction + f * force_gain * dt;
                let speed = p.vel.length();
                if speed > max_speed {
                    p.vel *= max_speed / speed;
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
    use crate::config::SimParams;
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
