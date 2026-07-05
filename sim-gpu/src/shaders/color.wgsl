// Dinámica del color por partícula (= `Simulation::apply_dynamics` de la
// CPU): saltos aleatorios de color, deriva gradual del matiz y transición
// suave hacia el matiz objetivo. El azar sale de un hash PCG por índice+frame
// (la CPU usa su rng de hilo; aquí basta ruido blanco con la misma tasa).
//
// La struct `Params` llega antepuesta desde params.wgsl. La deriva de la
// MATRIZ (`gradual_matrix_speed`) no vive aquí: muta los parámetros en la CPU
// y llega por el uniform como en la app CPU.

@group(0) @binding(0) var<uniform> P: Params;
@group(0) @binding(1) var<storage, read_write> hue: array<f32>;
@group(0) @binding(2) var<storage, read_write> target_hue: array<f32>;

// Hash PCG (pcg_hash clásico): u32 → u32 bien mezclado.
fn pcg(v0: u32) -> u32 {
    var v = v0 * 747796405u + 2891336453u;
    let w = ((v >> ((v >> 28u) + 4u)) ^ v) * 277803737u;
    return (w >> 22u) ^ w;
}

fn rand01(s: u32) -> f32 {
    return f32(pcg(s)) * (1.0 / 4294967295.0);
}

// Interpola el matiz por el camino más corto de la rueda (= `lerp_hue`).
// (`from`/`to` son palabras reservadas de WGSL, de ahí los nombres.)
fn lerp_hue(desde: f32, hacia: f32, t: f32) -> f32 {
    let d = fract(hacia - desde + 0.5) - 0.5;
    return fract(desde + d * t);
}

@compute @workgroup_size(256)
fn color_step(@builtin(global_invocation_id) gid: vec3u) {
    let i = gid.x;
    if i >= P.count {
        return;
    }
    var h = hue[i];
    var th = target_hue[i];
    let s0 = pcg(i ^ (P.color_seed * 0x9E3779B9u));

    // Saltos de color aleatorios: con transición suave solo fijan el objetivo
    // (el matiz transita hacia él); si no, cambian el color al instante.
    if P.random_color == 1u && rand01(s0) < P.p_switch {
        let nh = f32(pcg(s0 ^ 0xA511E9B3u) % 6u) / 6.0;
        th = nh;
        if P.color_smooth == 0u {
            h = nh;
        }
    }
    // Deriva lenta y gradual del matiz.
    if P.gradual == 1u {
        let d = (rand01(s0 ^ 0x63D83595u) * 2.0 - 1.0) * P.color_drift;
        if P.color_smooth == 1u {
            th = fract(th + d);
        } else {
            h = fract(h + d);
            th = h;
        }
    }
    // Suavizado: acerca el matiz a su objetivo en tiempo real.
    if P.color_smooth == 1u {
        h = lerp_hue(h, th, P.color_lerp);
    }
    hue[i] = h;
    target_hue[i] = th;
}
