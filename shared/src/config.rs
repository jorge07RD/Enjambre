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

/// Presets de resolución/relación de aspecto para el recuadro de grabación.
/// Dimensiones pares (yuv420p las exige). El índice se guarda en `frame_preset`.
pub const FRAME_PRESETS: [(&str, u32, u32); 5] = [
    ("TikTok 9:16", 1080, 1920),
    ("Reel 4:5", 1080, 1350),
    ("Cuadrado 1:1", 1080, 1080),
    ("Horizontal 16:9", 1920, 1080),
    ("9:16 ligero", 720, 1280),
];

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
    /// Bandada (Boids de Craig Reynolds, 1986): cada partícula sigue tres reglas
    /// locales —separación, alineación y cohesión— y emerge una murmuración.
    /// No usa el coeficiente escalar; su física es vectorial (ver `Simulation`).
    Boids,
}

/// Con qué vecinos se agrupa cada partícula en modo Boids.
#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum BoidsScope {
    /// Todas juntas, ignorando el color: una sola gran murmuración.
    All,
    /// Solo con el mismo color: varias bandadas independientes por color.
    SameColor,
    /// Separación frente a todas; alineación y cohesión solo con el mismo color.
    Hybrid,
}

/// Comportamiento en los bordes del lienzo.
#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Boundary {
    Wrap,
    Bounce,
}

/// Modo de la brocha de pintado en el lienzo.
#[derive(Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub enum Brush {
    #[default]
    Add,
    Erase,
}

/// Herramienta activa del botón izquierdo del ratón sobre el lienzo.
#[derive(Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub enum Tool {
    /// Pincel: pinta o borra partículas (comportamiento clásico).
    #[default]
    Brush,
    /// Fuerza: atrae o repele el enjambre alrededor del cursor.
    Force,
}

/// Qué parámetro modula la reactividad al audio.
#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum AudioTarget {
    Speed,
    Force,
    Brightness,
}

/// Acción que dispara cada beat de la música analizada (ver `MusicSync`).
#[derive(Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub enum BeatAction {
    /// Nada (solo la envolvente, si está activa).
    None,
    /// Pulso transitorio sobre el objetivo de audio (destello/empujón).
    #[default]
    Pulse,
    /// Aleatorizar la matriz de atracción (con transición fluida).
    RandomizeMatrix,
    /// Avanzar el show del secuenciador (o la escena siguiente si no suena).
    NextScene,
}

/// Sincronía con la pista de música elegida para el vídeo: la envolvente y los
/// beats salen de un análisis offline de la pista (`sim/src/music.rs`), no del
/// micrófono. Grabando, el reloj musical es el frame de vídeo (k/60 s), así el
/// resultado queda clavado al audio del `.mp4` por construcción.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MusicSync {
    /// Activar la sincronía (grabando y/o en preescucha).
    pub enabled: bool,
    /// La envolvente de la pista sustituye al micrófono como nivel de audio
    /// (usa el mismo `audio_target`/`audio_intensity` de la sección de audio).
    pub envelope_drive: bool,
    pub beat_action: BeatAction,
    /// Actuar cada N beats (1 = todos).
    pub beat_divisor: u32,
    /// Amplitud del pulso de beat sobre la ganancia de audio.
    pub pulse_gain: f32,
    /// Compensación de latencia (s) de la preescucha en vivo (no afecta a la
    /// grabación, que es exacta por construcción).
    pub latency_offset: f32,
}

impl Default for MusicSync {
    fn default() -> Self {
        Self {
            enabled: false,
            envelope_drive: true,
            beat_action: BeatAction::Pulse,
            beat_divisor: 1,
            pulse_gain: 1.5,
            latency_offset: 0.15,
        }
    }
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
        // Boids no usa coeficiente escalar: su física (separación/alineación/
        // cohesión) se calcula aparte en `Simulation::step`.
        InteractionMode::Boids => 0.0,
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
///
/// `#[serde(default)]` a nivel de contenedor: los campos ausentes en un JSON
/// (p. ej. una `scenes.json` guardada antes de añadir campos nuevos) toman su
/// valor de `Default`. Así añadir parámetros no rompe escenas ni IPC antiguos.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
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

    // --- Velocidad suave ---
    // `time_scale` es la velocidad EFECTIVA que usa la física; transita de forma
    // suave hacia `speed_target` (el valor que pide el usuario con el slider o
    // los botones de %). El resto son el estado de esa transición.
    /// Si está activo, los cambios de velocidad se interpolan en vez de saltar.
    pub speed_smooth: bool,
    /// Duración (s) de la transición de velocidad.
    pub speed_transition_duration: f32,
    /// Velocidad objetivo a la que tiende `time_scale`.
    pub speed_target: f32,
    /// Velocidad al inicio de la transición de velocidad actual.
    pub speed_from: f32,
    /// Progreso de la transición de velocidad: 1.0 = ya en el objetivo.
    pub speed_blend: f32,

    // --- Auto-aleatorizado de la matriz ---
    /// Si está activo (y en modo Matriz), la matriz se aleatoriza sola cada
    /// `auto_randomize_interval` segundos. El `sim` conduce el temporizador.
    pub auto_randomize: bool,
    /// Intervalo (s) entre aleatorizados automáticos de la matriz.
    pub auto_randomize_interval: f32,

    // --- Anti-aglomeración ---
    /// Si está activo, las bolas hiperdensas (partículas con muchos más
    /// vecinos que la densidad media) se disuelven con suavidad en vez de
    /// apilarse y vibrar violentamente.
    pub anti_clump: bool,
    /// Umbral de detección, como múltiplo de los vecinos esperados por la
    /// densidad media dentro de `r_max`.
    pub anti_clump_factor: f32,
    /// Fuerza de la dispersión hacia fuera del centro local (0..~3).
    pub anti_clump_strength: f32,

    // --- Recentrado de zonas activas ---
    /// Si está activo, las partículas en zonas densas (mucha actividad) sienten
    /// una atracción leve hacia el centro de la vista, para traer la acción de
    /// vuelta cuando se apila lejos de la cámara.
    pub attract_active: bool,
    /// Intensidad de esa atracción al centro (0 = nada).
    pub attract_active_strength: f32,

    // --- Bandada (Boids) ---
    /// Con qué vecinos se agrupa cada partícula (todas / mismo color / híbrido).
    pub boids_scope: BoidsScope,
    /// Peso de la regla de separación (evitar vecinos muy cercanos).
    pub boids_separation: f32,
    /// Peso de la regla de alineación (igualar la velocidad media de los vecinos).
    pub boids_alignment: f32,
    /// Peso de la regla de cohesión (acercarse al centro del grupo).
    pub boids_cohesion: f32,
    /// Radio de separación como fracción de `r_max` (0..1).
    pub boids_sep_radius: f32,
    /// Velocidad de crucero: rapidez mínima que mantienen los "pájaros" (0 = off).
    pub boids_cruise: f32,
    /// Repulsión entre bandadas de distinto color (solo en ámbito Híbrido/Por
    /// color): hace que los grupos se esquiven. 0 = sin evasión.
    pub boids_group_avoid: f32,

    // --- Estelas de movimiento ---
    /// Si está activo, las partículas dejan rastro (buffer que se desvanece).
    pub trails: bool,
    /// Cantidad de desvanecido por frame (0..1); menor = estela más larga.
    pub trail_fade: f32,

    // --- Orientación de las partículas ---
    /// Mezcla disco↔flecha (0 = disco, 1 = triángulo orientado por la velocidad).
    /// Es continuo para poder transicionar de forma fluida entre escenas.
    pub orient: f32,

    // --- Fuerza con el ratón ---
    /// Intensidad de la fuerza del cursor (herramienta Fuerza).
    pub pointer_strength: f32,
    /// Radio de acción de la fuerza del cursor, en unidades de mundo.
    pub pointer_radius: f32,
    /// `true` = repele (espanta); `false` = atrae.
    pub pointer_repel: bool,

    // --- Formar texto / imagen ---
    /// "Fijación" de la forma: 0 = las partículas fluyen con la física (texto
    /// vivo), 1 = se fijan nítidas en la forma. Solo actúa si hay una forma.
    pub shape_strength: f32,
    /// Duración (s) de la aparición/disolución fluida de la forma al aplicarla o
    /// soltarla. 0 = instantáneo.
    pub shape_transition_duration: f32,
    /// Teñir la forma de un color en vez de mantener los colores actuales.
    pub shape_tint: bool,
    /// Índice de color de la paleta para el tinte de la forma.
    pub shape_color: usize,
    /// Recrear los colores reales de la foto: cada partícula de la forma
    /// migra su matiz hacia el de su píxel de origen (mosaico completo), en
    /// vez de un tinte único. Solo aplica a imágenes (el texto no tiene
    /// color propio); mutuamente excluyente con `shape_tint` en la UI.
    pub shape_photo_color: bool,
    /// Descriptor de la forma activa (para poder guardarla en una escena y
    /// reconstruirla al cargarla). Los posee el `sim`. Vacíos = sin forma.
    /// Mensaje de texto activo (prioritario sobre la imagen).
    pub shape_text: String,
    /// Ruta de la imagen activa (si `shape_text` está vacío).
    pub shape_image: String,

    // --- Reactivo al audio ---
    /// Si está activo, el audio del micrófono modula un parámetro.
    pub audio_reactive: bool,
    /// Qué parámetro modula el audio.
    pub audio_target: AudioTarget,
    /// Intensidad de la modulación (cuánto empuja la amplitud del sonido).
    pub audio_intensity: f32,

    // --- Bloom (resplandor cinematográfico) ---
    /// Añade un halo aditivo alrededor de cada partícula (look de neón/brillo).
    pub bloom: bool,
    /// Intensidad del resplandor (0 = nada).
    pub bloom_intensity: f32,
    /// Radio del halo como múltiplo del tamaño de punto.
    pub bloom_radius: f32,
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
            speed_smooth: true,
            speed_transition_duration: 1.0,
            speed_target: 1.0,
            speed_from: 1.0,
            speed_blend: 1.0,
            auto_randomize: false,
            auto_randomize_interval: 8.0,
            anti_clump: true,
            anti_clump_factor: 3.0,
            anti_clump_strength: 1.0,
            attract_active: false,
            attract_active_strength: 0.4,
            boids_scope: BoidsScope::Hybrid,
            boids_separation: 1.5,
            boids_alignment: 1.0,
            boids_cohesion: 1.0,
            boids_sep_radius: 0.35,
            boids_cruise: 48.0,
            boids_group_avoid: 0.8,
            trails: false,
            trail_fade: 0.12,
            orient: 0.0,
            pointer_strength: 1.0,
            pointer_radius: 160.0,
            pointer_repel: true,
            audio_reactive: false,
            audio_target: AudioTarget::Speed,
            audio_intensity: 1.0,
            shape_strength: 0.5,
            shape_transition_duration: 1.5,
            shape_tint: false,
            shape_color: 0,
            shape_photo_color: false,
            shape_text: String::new(),
            shape_image: String::new(),
            bloom: false,
            bloom_intensity: 0.6,
            bloom_radius: 4.0,
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
        // Solo el blend gobierna: `start_transition` respeta el interruptor de
        // transición fluida, pero `start_matrix_blend` cruza siempre.
        if self.blend >= 1.0 {
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

    /// Arranca el cruce SIEMPRE, aunque la transición fluida global esté
    /// apagada: para aleatorizar/restablecer la matriz, que debe fundirse
    /// suave y no dar un salto brusco.
    pub fn start_matrix_blend(&mut self, from: InteractionSnapshot) {
        self.from_state = from;
        self.blend = 0.0;
    }

    /// Restablece la matriz a su valor por defecto (identidad: cada color se
    /// atrae a sí mismo y es neutral con el resto).
    pub fn reset_matrix(&mut self) {
        self.matrix = [[0.0; NUM_COLORS]; NUM_COLORS];
        for i in 0..NUM_COLORS {
            self.matrix[i][i] = 1.0;
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

    /// Fija una nueva velocidad objetivo. Si la velocidad suave está activa,
    /// arranca una transición desde la velocidad actual; si no, salta.
    pub fn set_speed(&mut self, target: f32) {
        self.speed_target = target.max(0.0);
        if self.speed_smooth {
            self.speed_from = self.time_scale;
            self.speed_blend = 0.0;
        } else {
            self.time_scale = self.speed_target;
            self.speed_blend = 1.0;
        }
    }

    /// Avanza la transición de velocidad y actualiza `time_scale` (la velocidad
    /// efectiva que usa la física). Suavizado ease-in-out, como las demás.
    pub fn advance_speed(&mut self, seconds: f32) {
        if self.speed_blend < 1.0 {
            self.speed_blend = (self.speed_blend
                + seconds.max(0.0) / self.speed_transition_duration.max(0.05))
            .min(1.0);
            let t = self.speed_blend * self.speed_blend * (3.0 - 2.0 * self.speed_blend);
            self.time_scale = self.speed_from + (self.speed_target - self.speed_from) * t;
        } else {
            self.time_scale = self.speed_target;
        }
    }

    /// Copia con el estado de transición ya asentado, para guardar como escena
    /// (sin quedar a medias de un blend de interacción o de velocidad).
    pub fn settled(&self) -> SimParams {
        let mut p = self.clone();
        p.blend = 1.0;
        p.from_state = p.current_snapshot();
        p.speed_target = p.time_scale;
        p.speed_from = p.time_scale;
        p.speed_blend = 1.0;
        p
    }

    /// Interpola los parámetros numéricos "no de interacción" desde `from` hacia
    /// `target` por la fracción `t` (0..1). La interacción (modo/matriz/rango) la
    /// cruza aparte el sistema de transición (`start_transition`), y los campos
    /// discretos y las duraciones se fijan al cargar/terminar la escena.
    pub fn lerp_scene_numeric(&mut self, from: &SimParams, target: &SimParams, t: f32) {
        let l = |a: f32, b: f32| a + (b - a) * t;
        self.force = l(from.force, target.force);
        self.r_max = l(from.r_max, target.r_max);
        self.beta = l(from.beta, target.beta);
        self.friction = l(from.friction, target.friction);
        self.point_size = l(from.point_size, target.point_size);
        self.brightness = l(from.brightness, target.brightness);
        self.random_color_rate = l(from.random_color_rate, target.random_color_rate);
        self.gradual_color_speed = l(from.gradual_color_speed, target.gradual_color_speed);
        self.gradual_matrix_speed = l(from.gradual_matrix_speed, target.gradual_matrix_speed);
        self.attract_active_strength = l(from.attract_active_strength, target.attract_active_strength);
        self.auto_randomize_interval = l(from.auto_randomize_interval, target.auto_randomize_interval);
        self.anti_clump_factor = l(from.anti_clump_factor, target.anti_clump_factor);
        self.anti_clump_strength = l(from.anti_clump_strength, target.anti_clump_strength);
        self.boids_separation = l(from.boids_separation, target.boids_separation);
        self.boids_alignment = l(from.boids_alignment, target.boids_alignment);
        self.boids_cohesion = l(from.boids_cohesion, target.boids_cohesion);
        self.boids_sep_radius = l(from.boids_sep_radius, target.boids_sep_radius);
        self.boids_cruise = l(from.boids_cruise, target.boids_cruise);
        self.boids_group_avoid = l(from.boids_group_avoid, target.boids_group_avoid);
        self.trail_fade = l(from.trail_fade, target.trail_fade);
        self.orient = l(from.orient, target.orient);
        self.pointer_strength = l(from.pointer_strength, target.pointer_strength);
        self.pointer_radius = l(from.pointer_radius, target.pointer_radius);
        self.audio_intensity = l(from.audio_intensity, target.audio_intensity);
        self.shape_strength = l(from.shape_strength, target.shape_strength);
        self.bloom_intensity = l(from.bloom_intensity, target.bloom_intensity);
        self.bloom_radius = l(from.bloom_radius, target.bloom_radius);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_smooth_converges_to_target() {
        let mut p = SimParams::default();
        p.speed_smooth = true;
        p.speed_transition_duration = 1.0;
        p.set_speed(0.1); // 100% -> 10%
        assert!((p.time_scale - 1.0).abs() < 1e-6, "arranca en el valor actual");
        // ~1.2 s a 60 fps debe alcanzar el objetivo (blend llega a 1).
        for _ in 0..72 {
            p.advance_speed(1.0 / 60.0);
        }
        assert!((p.time_scale - 0.1).abs() < 1e-3, "llega a 10%: {}", p.time_scale);
    }

    #[test]
    fn speed_instant_when_not_smooth() {
        let mut p = SimParams::default();
        p.speed_smooth = false;
        p.set_speed(2.5);
        assert!((p.time_scale - 2.5).abs() < 1e-6);
    }
}
