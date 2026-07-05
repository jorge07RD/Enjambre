// Física "particle life" en compute: port de `sim/src/simulation.rs` con
// paridad de TODOS los modos de interacción (`coef_raw` + `interaction`), la
// bandada (Boids), ambos contornos (toroidal y rebote) y las transiciones
// (cruce origen→objetivo por `blend_t` y `boids_mix`, ambos calculados en la
// CPU). Dos entradas: `step` (naive O(n²), referencia) y `step_grid` (vecinos
// por el grid CSR de grid.wgsl, O(n·k)); deben producir comportamiento
// estadísticamente idéntico.
//
// La struct `Params` llega antepuesta desde params.wgsl.

@group(0) @binding(0) var<uniform> P: Params;
@group(0) @binding(1) var<storage, read> pos_in: array<vec2f>;
@group(0) @binding(2) var<storage, read> vel_in: array<vec2f>;
@group(0) @binding(3) var<storage, read_write> pos_out: array<vec2f>;
@group(0) @binding(4) var<storage, read_write> vel_out: array<vec2f>;
@group(0) @binding(5) var<storage, read> hue: array<f32>;
// Puntos meta de la forma (texto/imagen); solo los primeros `n_shape` valen.
@group(0) @binding(6) var<storage, read> shape_tgt: array<vec2f>;
// Grid CSR (lo construye grid.wgsl): solo lo usa `step_grid`.
@group(1) @binding(0) var<storage, read> starts: array<u32>;
@group(1) @binding(1) var<storage, read> items: array<u32>;

// Cubo de color del matiz continuo (= `hue_bucket` de la CPU, NUM_COLORS = 6).
fn bucket(h: f32) -> u32 {
    return min(u32(fract(h) * 6.0), 5u);
}

// Distancia circular entre matices, en [0, 0.5] (= `hue_distance`).
fn hue_dist(a: f32, b: f32) -> f32 {
    let d = fract(a - b);
    return min(d, 1.0 - d);
}

// Coeficiente de interacción de un modo (= `coef_raw` de la CPU). La matriz
// se elige con `use_from` porque un uniform no se puede indexar por miembro
// de forma dinámica.
fn coef_raw(
    mode: u32,
    use_from: bool,
    sim_range: f32,
    same_repel: u32,
    same_strength: f32,
    ha: f32,
    hb: f32,
) -> f32 {
    let a = bucket(ha);
    let b = bucket(hb);
    switch mode {
        // Mismo color: se atrae; los distintos se ignoran o repelen.
        case 0u: {
            if a == b { return 1.0; }
            if same_repel == 1u { return -same_strength; }
            return 0.0;
        }
        // Matriz 6×6 (fila = recibe, columna = ejerce).
        case 1u: {
            let idx = a * 6u + b;
            if use_from { return P.from_matrix[idx / 4u][idx % 4u]; }
            return P.matrix[idx / 4u][idx % 4u];
        }
        // Similitud: más parecido el matiz, más atracción.
        case 2u: {
            let dh = hue_dist(ha, hb);
            return clamp(1.0 - dh / max(sim_range, 1e-3), -1.0, 1.0);
        }
        // Cíclico: persigue al siguiente color de la rueda, huye del anterior.
        case 3u: {
            if a == b { return 0.6; }
            if b == (a + 1u) % 6u { return 1.0; }
            if b == (a + 5u) % 6u { return -1.0; }
            return 0.0;
        }
        // Opuestos: los complementarios se atraen, los parecidos se repelen.
        case 4u: {
            let dh = hue_dist(ha, hb);
            return clamp(dh * 4.0 - 1.0, -1.0, 1.0);
        }
        // Depredador–presa: los pares cazan a los impares (que huyen).
        case 5u: {
            if a % 2u == b % 2u { return 0.5; }
            if a % 2u == 0u { return 1.0; }
            return -1.0;
        }
        // Repulsión propia: el mismo color se repele, los distintos se atraen.
        case 6u: {
            if a == b { return -1.0; }
            return 0.5;
        }
        // Boids: sin coeficiente escalar (su física es vectorial, más abajo).
        default: { return 0.0; }
    }
}

// Coeficiente efectivo (= `SimParams::interaction()`): cruza el congelado y
// el objetivo por el progreso ya suavizado `blend_t`.
fn interaction(ha: f32, hb: f32) -> f32 {
    let objetivo = coef_raw(P.mode, false, P.sim_range, P.same_repel, P.same_strength, ha, hb);
    if P.blend_t >= 1.0 {
        return objetivo;
    }
    let origen = coef_raw(
        P.from_mode, true, P.from_sim_range, P.from_same_repel, P.from_same_strength, ha, hb,
    );
    return mix(origen, objetivo, P.blend_t);
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

// Imagen mínima del toro (la distancia más corta); solo con contorno wrap.
fn min_image(d0: vec2f) -> vec2f {
    var d = d0;
    let half = P.world * 0.5;
    if d.x > half.x { d.x -= P.world.x; } else if d.x < -half.x { d.x += P.world.x; }
    if d.y > half.y { d.y -= P.world.y; } else if d.y < -half.y { d.y += P.world.y; }
    return d;
}

// Acumuladores por partícula: fuerza radial + reglas de la bandada.
struct Acc {
    acc: vec2f,
    sep: vec2f,
    ali: vec2f,
    coh: vec2f,
    grp: vec2f,
    // Suma de vectores a los vecinos: apunta al centro de masa local (para
    // la dispersión anti-aglomeración).
    crowd: vec2f,
    flock_n: u32,
    neighbors: u32,
}

// Aportación del vecino `j` (= cuerpo del bucle de vecinos de la CPU).
// Durante una transición pueden correr ambos modelos a la vez (se combinan
// luego con `boids_mix`).
fn accumulate(i: u32, pi: vec2f, hi: f32, bi: u32, j: u32, A: ptr<function, Acc>) {
    if j == i {
        return;
    }
    var d = pos_in[j] - pi;
    if P.boundary == 0u {
        d = min_image(d);
    }
    let d2 = dot(d, d);
    if d2 > P.r_max * P.r_max || d2 < 1e-8 {
        return;
    }
    (*A).neighbors += 1u;
    (*A).crowd += d;
    let dist = sqrt(d2);
    // Interacción texto ↔ fondo (= la CPU): se ignoran mutuamente y el fondo
    // esquiva al texto para no invadir las letras.
    if P.n_shape > 0u {
        let j_text = j < P.n_shape;
        if i < P.n_shape {
            // El texto solo se relaciona con el propio texto.
            if !j_text {
                return;
            }
        } else if j_text {
            // El fondo repele al texto y no lo toma como vecino normal.
            let push = 1.0 - dist / P.r_max;
            (*A).acc -= d * (push * P.shape_avoid / dist);
            return;
        }
    }
    if P.boids_mix > 0.0 {
        let same = bi == bucket(hue[j]);
        // Separación: frente a todas salvo en ámbito "por color" (1).
        if (P.boids_scope != 1u || same) && d2 < P.sep_r * P.sep_r {
            (*A).sep -= d * ((P.sep_r - dist) / (P.sep_r * dist));
        }
        // Alineación + cohesión con la bandada ("todas" (0) o mismo color).
        if P.boids_scope == 0u || same {
            (*A).ali += vel_in[j];
            (*A).coh += d;
            (*A).flock_n += 1u;
        }
        // Evasión de otros grupos (solo híbrido/por color), caída lineal.
        if P.boids_scope != 0u && !same {
            (*A).grp -= d * ((1.0 - dist / P.r_max) / dist);
        }
    }
    if P.boids_mix < 1.0 {
        let cf = interaction(hi, hue[j]);
        (*A).acc += d * (force_fn(dist / P.r_max, cf, P.beta) / dist);
    }
}

// Integración (= CPU): fricción, límite de velocidad, crucero de la bandada y
// contorno (envoltura, deslizamiento boids o rebote amortiguado).
fn integrate(i: u32, pi: vec2f, acc: vec2f) {
    var v = vel_in[i] * P.friction + acc * P.force * P.dt;
    let speed = length(v);
    if speed > P.r_max {
        v *= P.r_max / speed;
    }
    // Crucero: rapidez mínima de la bandada (escalada por la mezcla). No se
    // aplica a las partículas de la forma (deben asentarse en el texto).
    let cruise = P.cruise * P.boids_mix;
    if cruise > 0.0 && i >= P.n_shape {
        let s = length(v);
        if s > 1e-4 {
            if s < cruise {
                v *= cruise / s;
            }
        } else {
            // En reposo: dirección pseudoaleatoria estable derivada de la
            // posición para que arranque el vuelo (mismo hash que la CPU).
            let a = pi.x * 12.9898 + pi.y * 78.233;
            v = vec2f(cos(a), sin(a)) * cruise;
        }
    }
    var p = pi + v * P.dt;
    if P.boundary == 0u {
        if p.x < 0.0 { p.x += P.world.x; } else if p.x >= P.world.x { p.x -= P.world.x; }
        if p.y < 0.0 { p.y += P.world.y; } else if p.y >= P.world.y { p.y -= P.world.y; }
    } else if P.boids_mix > 0.0 {
        // Bandada: deslizar por la pared (anular solo la componente saliente);
        // el giro de `wall_turn` ya la estaba curvando hacia el interior.
        if p.x < 0.0 { p.x = 0.0; v.x = max(v.x, 0.0); }
        else if p.x > P.world.x { p.x = P.world.x; v.x = min(v.x, 0.0); }
        if p.y < 0.0 { p.y = 0.0; v.y = max(v.y, 0.0); }
        else if p.y > P.world.y { p.y = P.world.y; v.y = min(v.y, 0.0); }
    } else {
        if p.x < 0.0 { p.x = 0.0; v.x = -v.x * 0.5; }
        else if p.x > P.world.x { p.x = P.world.x; v.x = -v.x * 0.5; }
        if p.y < 0.0 { p.y = 0.0; v.y = -v.y * 0.5; }
        else if p.y > P.world.y { p.y = P.world.y; v.y = -v.y * 0.5; }
    }
    pos_out[i] = p;
    vel_out[i] = v;
}

// Composición de la bandada + recentrado + integración (= final del bucle de
// fuerzas de la CPU).
fn finish(i: u32, pi: vec2f, A: Acc) {
    var acc = A.acc;
    // Anti-aglomeración (= la CPU): con muchos más vecinos que la media, se
    // calman las fuerzas de pareja y se empuja suavemente hacia fuera del
    // centro local para que la bola se disuelva desde el borde.
    if P.clump_thr > 0.0 && f32(A.neighbors) > P.clump_thr {
        let over = min((f32(A.neighbors) - P.clump_thr) / P.clump_thr, 1.0);
        acc *= 1.0 - 0.7 * over;
        let len = length(A.crowd);
        if len > 1e-4 {
            acc -= (A.crowd / len) * (over * P.clump_strength);
        }
    }
    if P.boids_mix > 0.0 {
        var b = A.sep * P.w_sep + A.grp * P.w_grp;
        if A.flock_n > 0u {
            let inv = 1.0 / f32(A.flock_n);
            // Alineación: dirigir la velocidad hacia la media local. Cohesión:
            // hacia el centro de masa (normalizado por r_max).
            b += (A.ali * inv - vel_in[i]) * P.w_ali;
            b += A.coh * (inv / P.r_max) * P.w_coh;
        }
        // Esquive de paredes (solo con rebote): giro hacia el interior que
        // crece al acercarse al borde.
        if P.boundary == 1u {
            let m = P.r_max;
            if pi.x < m { b.x += P.wall_turn * (1.0 - pi.x / m); }
            else if pi.x > P.world.x - m { b.x -= P.wall_turn * (1.0 - (P.world.x - pi.x) / m); }
            if pi.y < m { b.y += P.wall_turn * (1.0 - pi.y / m); }
            else if pi.y > P.world.y - m { b.y -= P.wall_turn * (1.0 - (P.world.y - pi.y) / m); }
        }
        acc += b * P.boids_mix;
    }
    // Atracción leve al centro para las zonas densas (recentrado).
    if P.attract == 1u {
        var toward = P.world * 0.5 - pi;
        if P.boundary == 0u {
            toward = min_image(toward);
        }
        let dd = length(toward);
        if dd > 1.0 {
            let activity = min(f32(A.neighbors) / 30.0, 1.0);
            acc += (toward / dd) * (P.attract_strength * activity);
        }
    }
    // Forma: solo las partículas asignadas van al texto (interacción residual
    // + resorte hacia su punto meta); el resto sigue con la animación.
    if i < P.n_shape {
        var pull = shape_tgt[i] - pi;
        if P.boundary == 0u {
            pull = min_image(pull);
        }
        acc = acc * P.shape_inter + pull * P.shape_k;
    }
    integrate(i, pi, acc);
}

@compute @workgroup_size(256)
fn step(@builtin(global_invocation_id) gid: vec3u) {
    let i = gid.x;
    if i >= P.count {
        return;
    }
    let pi = pos_in[i];
    let hi = hue[i];
    let bi = bucket(hi);
    var A: Acc; // cero-inicializado
    for (var j = 0u; j < P.count; j++) {
        accumulate(i, pi, hi, bi, j, &A);
    }
    finish(i, pi, A);
}

@compute @workgroup_size(256)
fn step_grid(@builtin(global_invocation_id) gid: vec3u) {
    let i = gid.x;
    if i >= P.count {
        return;
    }
    let pi = pos_in[i];
    let hi = hue[i];
    let bi = bucket(hi);
    var A: Acc;

    let cx = clamp(i32(pi.x * P.inv_cell), 0, P.cols - 1);
    let cy = clamp(i32(pi.y * P.inv_cell), 0, P.rows - 1);
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            var nx = cx + dx;
            var ny = cy + dy;
            if P.boundary == 0u {
                // Vecinas con envoltura toroidal (como el rem_euclid de la CPU).
                nx = nx % P.cols; if nx < 0 { nx += P.cols; }
                ny = ny % P.rows; if ny < 0 { ny += P.rows; }
            } else if nx < 0 || nx >= P.cols || ny < 0 || ny >= P.rows {
                // Con rebote no hay celdas al otro lado del borde.
                continue;
            }
            let c = u32(ny * P.cols + nx);
            let e = starts[c + 1u];
            for (var k = starts[c]; k < e; k++) {
                accumulate(i, pi, hi, bi, items[k], &A);
            }
        }
    }
    finish(i, pi, A);
}
