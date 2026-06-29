use rand::Rng;
use serde::{Deserialize, Serialize};

/// Número de "tipos" discretos de color (para los modos de matriz / mismo color
/// y para los botones de la paleta). El color real de cada partícula es un
/// matiz continuo (`hue` en [0,1)), así que entre los 6 tipos pueden existir
/// tonos intermedios como el naranja.
pub const NUM_COLORS: usize = 6;

/// Matiz representativo del tipo discreto `i` (rojo, amarillo, verde, cyan,
/// azul, magenta — repartidos uniformemente por la rueda).
pub fn hue_for_index(i: usize) -> f32 {
    i as f32 / NUM_COLORS as f32
}

/// Convierte un matiz [0,1) en color RGB vivo (HSV con s=v=1). Devuelve los
/// canales `[r, g, b]` en [0,1] para no depender de ningún tipo gráfico
/// concreto (macroquad o egui los envuelven a su gusto).
pub fn color_for_hue(h: f32) -> [f32; 3] {
    let h6 = h.rem_euclid(1.0) * 6.0;
    let i = h6.floor() as i32;
    let f = h6 - i as f32;
    match i.rem_euclid(6) {
        0 => [1.0, f, 0.0],
        1 => [1.0 - f, 1.0, 0.0],
        2 => [0.0, 1.0, f],
        3 => [0.0, 1.0 - f, 1.0],
        4 => [f, 0.0, 1.0],
        _ => [1.0, 0.0, 1.0 - f],
    }
}

/// Colores de la paleta (uno por tipo discreto), para la interfaz.
pub fn palette() -> [[f32; 3]; NUM_COLORS] {
    let mut p = [[0.0f32; 3]; NUM_COLORS];
    for i in 0..NUM_COLORS {
        p[i] = color_for_hue(hue_for_index(i));
    }
    p
}

pub const COLOR_NAMES: [&str; NUM_COLORS] =
    ["Rojo", "Amarillo", "Verde", "Cyan", "Azul", "Magenta"];

/// Tipo discreto al que pertenece un matiz (para los modos de matriz).
#[inline]
pub fn hue_bucket(h: f32) -> usize {
    ((h.rem_euclid(1.0) * NUM_COLORS as f32) as usize).min(NUM_COLORS - 1)
}

/// Distancia circular entre dos matices, en [0, 0.5].
#[inline]
pub fn hue_distance(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(1.0);
    d.min(1.0 - d)
}

/// Modo de interacción entre colores.
#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum InteractionMode {
    /// Solo el mismo tipo de color se atrae.
    SameColorOnly,
    /// Matriz completa 6×6 estilo "particle life".
    Matrix,
    /// Por similitud: cuanto más parecido el matiz, más atracción; los muy
    /// distintos se repelen. (El naranja se siente atraído por rojo y amarillo.)
    Similarity,
    /// Cíclico (piedra-papel-tijera): cada color persigue al siguiente de la
    /// rueda y huye del anterior. Surgen persecuciones, espirales y ondas.
    Cyclic,
    /// Opuestos se atraen: los matices complementarios (opuestos en la rueda)
    /// se atraen y los parecidos se repelen.
    Opposite,
    /// Depredador–presa: los colores pares cazan a los impares (y estos huyen);
    /// cada bando se mantiene cohesionado. Manadas que se desplazan.
    PredatorPrey,
    /// Repulsión propia / atracción ajena: el mismo color se repele y los
    /// distintos se atraen. Espumas y mosaicos homogéneos.
    SelfRepel,
}

/// Comportamiento en los bordes del lienzo.
#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Boundary {
    Wrap,
    Bounce,
}

/// Modo de la brocha de pintado en el lienzo.
#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Brush {
    Add,
    Erase,
}

/// Aspecto visual de cada punto.
#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RenderStyle {
    /// Disco de brillo radial (cuanto más grande/solapado, más brilla).
    Glow,
    /// Disco de color sólido y opaco, con borde suavizado.
    Solid,
    /// Núcleo sólido rodeado de un halo brillante.
    SolidHalo,
}

/// Coeficiente de interacción para un par de matices, dada una configuración
/// de interacción concreta. Compartido por el estado actual y el congelado.
#[inline]
fn coef_raw(
    mode: InteractionMode,
    matrix: &[[f32; NUM_COLORS]; NUM_COLORS],
    sim_range: f32,
    same_repel: bool,
    same_strength: f32,
    hue_a: f32,
    hue_b: f32,
) -> f32 {
    match mode {
        InteractionMode::SameColorOnly => {
            if hue_bucket(hue_a) == hue_bucket(hue_b) {
                1.0
            } else if same_repel {
                -same_strength
            } else {
                0.0
            }
        }
        InteractionMode::Matrix => matrix[hue_bucket(hue_a)][hue_bucket(hue_b)],
        InteractionMode::Similarity => {
            let dh = hue_distance(hue_a, hue_b);
            (1.0 - dh / sim_range.max(1e-3)).clamp(-1.0, 1.0)
        }
        InteractionMode::Cyclic => {
            // Persigue al siguiente color de la rueda, huye del anterior y se
            // agrupa un poco con su propio color.
            let a = hue_bucket(hue_a);
            let b = hue_bucket(hue_b);
            let n = NUM_COLORS;
            if a == b {
                0.6
            } else if b == (a + 1) % n {
                1.0
            } else if b == (a + n - 1) % n {
                -1.0
            } else {
                0.0
            }
        }
        InteractionMode::Opposite => {
            // dh ∈ [0, 0.5]: parecidos (dh≈0) se repelen, complementarios
            // (dh≈0.5) se atraen, con el cruce neutro en el cuarto de rueda.
            let dh = hue_distance(hue_a, hue_b);
            (dh * 4.0 - 1.0).clamp(-1.0, 1.0)
        }
        InteractionMode::PredatorPrey => {
            // Dos bandos según la paridad del tipo: los pares (depredadores)
            // cazan a los impares (presas) y estos huyen; cada bando se cohesiona.
            let a = hue_bucket(hue_a);
            let b = hue_bucket(hue_b);
            if a % 2 == b % 2 {
                0.5
            } else if a % 2 == 0 {
                1.0
            } else {
                -1.0
            }
        }
        InteractionMode::SelfRepel => {
            if hue_bucket(hue_a) == hue_bucket(hue_b) {
                -1.0
            } else {
                0.5
            }
        }
    }
}

/// Instantánea de la configuración de interacción, para congelar el estado de
/// origen al iniciar una transición y poder mezclar de forma continua.
#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct InteractionSnapshot {
    pub mode: InteractionMode,
    pub matrix: [[f32; NUM_COLORS]; NUM_COLORS],
    pub sim_range: f32,
    pub same_repel_others: bool,
    pub same_repel_strength: f32,
}

impl InteractionSnapshot {
    #[inline]
    fn coef(&self, hue_a: f32, hue_b: f32) -> f32 {
        coef_raw(
            self.mode,
            &self.matrix,
            self.sim_range,
            self.same_repel_others,
            self.same_repel_strength,
            hue_a,
            hue_b,
        )
    }
}

/// Parámetros ajustables de la simulación.
#[derive(Clone, Serialize, Deserialize)]
pub struct SimParams {
    pub force: f32,
    pub r_max: f32,
    pub beta: f32,
    pub friction: f32,
    pub time_scale: f32,
    pub boundary: Boundary,
    pub mode: InteractionMode,
    pub matrix: [[f32; NUM_COLORS]; NUM_COLORS],
    pub point_size: f32,
    pub style: RenderStyle,
    /// Multiplicador de brillo (alfa) del punto, independiente del tamaño.
    pub brightness: f32,

    /// Modo "mismo color": si está activo, los colores distintos se repelen
    /// (en vez de ignorarse).
    pub same_repel_others: bool,
    /// Intensidad de esa repulsión (0..1).
    pub same_repel_strength: f32,

    /// Modo similitud: ancho de tolerancia de matiz. Colores cuya distancia de
    /// matiz sea menor que esto se atraen; más allá, se repelen.
    pub sim_range: f32,

    /// Cambio de color totalmente aleatorio (saltos).
    pub random_color: bool,
    /// Probabilidad por frame de que una partícula salte a otro color.
    pub random_color_rate: f32,

    /// Deriva lenta y gradual del color y de la atracción.
    pub gradual: bool,
    /// Velocidad de deriva del matiz de cada partícula.
    pub gradual_color_speed: f32,
    /// Velocidad de deriva de la matriz de atracción.
    pub gradual_matrix_speed: f32,
    /// Si está activo, los cambios de color (saltos aleatorios) se interpolan.
    pub color_smooth: bool,
    /// Duración (s) de la transición de color cuando `color_smooth` está activo.
    pub color_transition_duration: f32,

    /// Si está activo, los cambios de interacción se interpolan en vez de saltar.
    pub smooth: bool,
    /// Duración de la transición fluida, en segundos reales.
    pub transition_duration: f32,
    /// Progreso de la transición actual: 1.0 = sin transición (estado objetivo).
    pub blend: f32,
    /// Configuración de interacción congelada al iniciar la transición (origen).
    pub from_state: InteractionSnapshot,
}

impl Default for SimParams {
    fn default() -> Self {
        let mut matrix = [[0.0f32; NUM_COLORS]; NUM_COLORS];
        for i in 0..NUM_COLORS {
            matrix[i][i] = 1.0;
        }
        Self {
            force: 0.7,
            r_max: 80.0,
            beta: 0.30,
            friction: 0.85,
            time_scale: 1.0,
            boundary: Boundary::Wrap,
            mode: InteractionMode::SameColorOnly,
            matrix,
            point_size: 4.0,
            style: RenderStyle::Glow,
            brightness: 1.0,
            same_repel_others: false,
            same_repel_strength: 0.5,
            sim_range: 0.15,
            random_color: false,
            random_color_rate: 0.05,
            gradual: false,
            gradual_color_speed: 0.02,
            gradual_matrix_speed: 0.02,
            color_smooth: false,
            color_transition_duration: 2.0,
            smooth: false,
            transition_duration: 4.0,
            blend: 1.0,
            from_state: InteractionSnapshot {
                mode: InteractionMode::SameColorOnly,
                matrix,
                sim_range: 0.15,
                same_repel_others: false,
                same_repel_strength: 0.5,
            },
        }
    }
}

impl SimParams {
    /// Coeficiente "objetivo" del modo actual: lo que se aplicaría de forma
    /// instantánea. Positivo = atracción, negativo = repulsión.
    #[inline]
    pub fn target_coef(&self, hue_a: f32, hue_b: f32) -> f32 {
        coef_raw(
            self.mode,
            &self.matrix,
            self.sim_range,
            self.same_repel_others,
            self.same_repel_strength,
            hue_a,
            hue_b,
        )
    }

    /// Coeficiente que realmente usa la física. Si hay una transición en curso
    /// (`blend < 1`), mezcla de forma continua entre la interacción congelada
    /// (`from_state`) y el objetivo del modo actual.
    #[inline]
    pub fn interaction(&self, hue_a: f32, hue_b: f32) -> f32 {
        let target = self.target_coef(hue_a, hue_b);
        if self.blend >= 1.0 || !self.smooth {
            return target;
        }
        let from = self.from_state.coef(hue_a, hue_b);
        // Suavizado ease-in-out para una mezcla más orgánica.
        let t = self.blend * self.blend * (3.0 - 2.0 * self.blend);
        from + (target - from) * t
    }

    /// Instantánea de la configuración de interacción actual.
    pub fn current_snapshot(&self) -> InteractionSnapshot {
        InteractionSnapshot {
            mode: self.mode,
            matrix: self.matrix,
            sim_range: self.sim_range,
            same_repel_others: self.same_repel_others,
            same_repel_strength: self.same_repel_strength,
        }
    }

    /// Arranca una transición desde el estado efectivo `from`. Si la transición
    /// fluida está desactivada, salta de inmediato (instantáneo).
    pub fn start_transition(&mut self, from: InteractionSnapshot) {
        if self.smooth {
            self.from_state = from;
            self.blend = 0.0;
        } else {
            self.blend = 1.0;
        }
    }

    /// Avanza la transición en curso. `seconds` = tiempo real transcurrido en
    /// el frame, así la duración es predecible independientemente de los FPS.
    pub fn advance_transition(&mut self, seconds: f32) {
        if self.blend < 1.0 {
            self.blend =
                (self.blend + seconds.max(0.0) / self.transition_duration.max(0.05)).min(1.0);
        }
    }

    /// Rellena la matriz con coeficientes aleatorios en [-1, 1].
    pub fn randomize_matrix(&mut self, rng: &mut impl Rng) {
        for i in 0..NUM_COLORS {
            for j in 0..NUM_COLORS {
                self.matrix[i][j] = rng.gen_range(-1.0..=1.0);
            }
        }
    }
}
