// Física "particle life" en compute: port literal de `sim/src/simulation.rs`
// (`force_fn` + integración con fricción y límite de velocidad), modo Matriz,
// contorno toroidal. Dos entradas: `step` (naive O(n²), referencia) y
// `step_grid` (vecinos por el grid CSR de grid.wgsl, O(n·k)); deben producir
// comportamiento estadísticamente idéntico.

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
    _pad: u32,
    // Matriz 6×6 de atracción entre colores, empaquetada en 9 vec4 (alineación
    // de uniform). Fila = recibe, columna = ejerce (como en la CPU).
    matrix: array<vec4f, 9>,
};

@group(0) @binding(0) var<uniform> P: Params;
@group(0) @binding(1) var<storage, read> pos_in: array<vec2f>;
@group(0) @binding(2) var<storage, read> vel_in: array<vec2f>;
@group(0) @binding(3) var<storage, read_write> pos_out: array<vec2f>;
@group(0) @binding(4) var<storage, read_write> vel_out: array<vec2f>;
@group(0) @binding(5) var<storage, read> hue: array<f32>;
// Grid CSR (lo construye grid.wgsl): solo lo usa `step_grid`.
@group(1) @binding(0) var<storage, read> starts: array<u32>;
@group(1) @binding(1) var<storage, read> items: array<u32>;

fn mat_coef(i: u32, j: u32) -> f32 {
    let idx = i * 6u + j;
    return P.matrix[idx / 4u][idx % 4u];
}

// Cubo de color del matiz continuo (= `hue_bucket` de la CPU, NUM_COLORS = 6).
fn bucket(h: f32) -> u32 {
    return min(u32(fract(h) * 6.0), 5u);
}

// Perfil de fuerza "particle life" (= `force_fn` de la CPU): repulsión dura
// bajo `beta`, triángulo escalado por el coeficiente por encima.
fn force_fn(r: f32, coef: f32, beta: f32) -> f32 {
    if r < beta {
        return r / beta - 1.0;
    }
    let peak = 1.0 - abs(2.0 * r - 1.0 - beta) / (1.0 - beta);
    return coef * peak;
}

// Aportación del vecino `j` a la aceleración de la partícula `i`.
fn pair_force(i: u32, pi: vec2f, bi: u32, j: u32) -> vec2f {
    if j == i {
        return vec2f(0.0);
    }
    let half = P.world * 0.5;
    var d = pos_in[j] - pi;
    // Imagen mínima: la distancia más corta por el toro.
    if d.x > half.x { d.x -= P.world.x; } else if d.x < -half.x { d.x += P.world.x; }
    if d.y > half.y { d.y -= P.world.y; } else if d.y < -half.y { d.y += P.world.y; }
    let d2 = dot(d, d);
    if d2 > P.r_max * P.r_max || d2 < 1e-8 {
        return vec2f(0.0);
    }
    let dist = sqrt(d2);
    let coef = mat_coef(bi, bucket(hue[j]));
    return d * (force_fn(dist / P.r_max, coef, P.beta) / dist);
}

// Integración (= CPU): fricción + límite de velocidad + envoltura toroidal.
fn integrate(i: u32, pi: vec2f, acc: vec2f) {
    var v = vel_in[i] * P.friction + acc * P.force * P.dt;
    let speed = length(v);
    if speed > P.r_max {
        v *= P.r_max / speed;
    }
    var p = pi + v * P.dt;
    if p.x < 0.0 { p.x += P.world.x; } else if p.x >= P.world.x { p.x -= P.world.x; }
    if p.y < 0.0 { p.y += P.world.y; } else if p.y >= P.world.y { p.y -= P.world.y; }
    pos_out[i] = p;
    vel_out[i] = v;
}

@compute @workgroup_size(256)
fn step(@builtin(global_invocation_id) gid: vec3u) {
    let i = gid.x;
    if i >= P.count {
        return;
    }
    let pi = pos_in[i];
    let bi = bucket(hue[i]);
    var acc = vec2f(0.0);
    for (var j = 0u; j < P.count; j++) {
        acc += pair_force(i, pi, bi, j);
    }
    integrate(i, pi, acc);
}

@compute @workgroup_size(256)
fn step_grid(@builtin(global_invocation_id) gid: vec3u) {
    let i = gid.x;
    if i >= P.count {
        return;
    }
    let pi = pos_in[i];
    let bi = bucket(hue[i]);
    var acc = vec2f(0.0);

    let cx = clamp(i32(pi.x * P.inv_cell), 0, P.cols - 1);
    let cy = clamp(i32(pi.y * P.inv_cell), 0, P.rows - 1);
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            // Vecinas con envoltura toroidal (como el rem_euclid de la CPU).
            var nx = (cx + dx) % P.cols;
            if nx < 0 { nx += P.cols; }
            var ny = (cy + dy) % P.rows;
            if ny < 0 { ny += P.rows; }
            let c = u32(ny * P.cols + nx);
            let e = starts[c + 1u];
            for (var k = starts[c]; k < e; k++) {
                acc += pair_force(i, pi, bi, items[k]);
            }
        }
    }
    integrate(i, pi, acc);
}
