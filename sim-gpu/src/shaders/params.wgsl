// Parámetros de la física en GPU (espejo binario de `GpuParams` en
// gpu_sim.rs). Este fichero se ANTEPONE a sim.wgsl y grid.wgsl al crear los
// módulos, para que ambos compartan la misma definición sin duplicarla.
//
// Interacción: el kernel evalúa el coeficiente del modo `mode` y, si hay una
// transición en curso (`blend_t < 1`), lo cruza con el del estado congelado
// `from_*` — el mismo esquema que `SimParams::interaction()` en la CPU, con
// el ease-in-out ya aplicado en la CPU.

struct Params {
    world: vec2f,
    dt: f32,
    friction: f32,
    force: f32,
    r_max: f32,
    beta: f32,
    count: u32,
    cols: i32,
    rows: i32,
    inv_cell: f32,
    // 0 = toroidal (wrap), 1 = rebote (bounce).
    boundary: u32,
    // Interacción objetivo (códigos en `mode_code`, gpu_sim.rs).
    mode: u32,
    sim_range: f32,
    same_repel: u32,
    same_strength: f32,
    // Interacción congelada al iniciar la transición (origen del cruce).
    from_mode: u32,
    from_sim_range: f32,
    from_same_repel: u32,
    from_same_strength: f32,
    // Progreso YA suavizado (ease-in-out) del cruce; 1 = sin transición.
    blend_t: f32,
    // Bandada: mezcla radial↔boids (0..1, sigue el mismo ease) y sus pesos.
    boids_mix: f32,
    // 0 = todas, 1 = por color, 2 = híbrido.
    boids_scope: u32,
    w_sep: f32,
    w_ali: f32,
    w_coh: f32,
    w_grp: f32,
    // Radio de separación absoluto (boids_sep_radius · r_max).
    sep_r: f32,
    cruise: f32,
    wall_turn: f32,
    // Recentrado de zonas activas (atracción al centro según densidad).
    attract: u32,
    attract_strength: f32,
    // Forma (texto/imagen): las primeras `n_shape` partículas van a sus puntos
    // meta (buffer `shape_tgt`) con un resorte. Factores precocinados en la
    // CPU, ya escalados por la mezcla de aparición; n_shape = 0 → sin forma.
    n_shape: u32,
    shape_k: f32,
    shape_inter: f32,
    shape_avoid: f32,
    // Dinámica del color por partícula (ver color.wgsl); las tasas llegan ya
    // escaladas por el paso (`time_scale`) desde la CPU.
    random_color: u32,
    p_switch: f32,
    gradual: u32,
    color_drift: f32,
    color_smooth: u32,
    color_lerp: f32,
    color_seed: u32,
    _pad2: u32,
    // Anti-aglomeración: umbral ABSOLUTO de vecinos (precalculado de la
    // densidad media en la CPU; 0 = desactivado) y fuerza de dispersión.
    clump_thr: f32,
    clump_strength: f32,
    _pad3: f32,
    _pad4: f32,
    // Matrices 6×6 empaquetadas en 9 vec4 (alineación de uniform): la del
    // modo objetivo y la congelada de la transición.
    matrix: array<vec4f, 9>,
    from_matrix: array<vec4f, 9>,
};
