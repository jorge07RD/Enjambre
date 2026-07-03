// Construcción del grid espacial en GPU: counting sort en formato CSR, el
// mismo esquema que `sim/src/grid.rs` en la CPU (counts → prefijo acumulado →
// scatter con cursor), con atómicos para el conteo y la colocación.
//
// Tres pasadas por frame: `count` (histograma), `prefix` (scan serial: las
// celdas son pocas, (mundo/r_max)², así que un solo hilo es despreciable
// frente a las fuerzas; paralelizar con un scan jerárquico si creciera) y
// `scatter` (colocar los índices, cursor atómico por celda).

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
    matrix: array<vec4f, 9>,
};

@group(0) @binding(0) var<uniform> P: Params;
@group(0) @binding(1) var<storage, read> pos: array<vec2f>;
@group(0) @binding(2) var<storage, read_write> counts: array<atomic<u32>>;
@group(0) @binding(3) var<storage, read_write> starts: array<u32>;
@group(0) @binding(4) var<storage, read_write> cursor: array<atomic<u32>>;
@group(0) @binding(5) var<storage, read_write> items: array<u32>;

fn cell_of(p: vec2f) -> u32 {
    let cx = clamp(i32(p.x * P.inv_cell), 0, P.cols - 1);
    let cy = clamp(i32(p.y * P.inv_cell), 0, P.rows - 1);
    return u32(cy * P.cols + cx);
}

@compute @workgroup_size(256)
fn count(@builtin(global_invocation_id) gid: vec3u) {
    let i = gid.x;
    if i >= P.count {
        return;
    }
    atomicAdd(&counts[cell_of(pos[i])], 1u);
}

@compute @workgroup_size(1)
fn prefix() {
    let ncells = u32(P.cols * P.rows);
    var acc = 0u;
    for (var c = 0u; c < ncells; c++) {
        starts[c] = acc;
        atomicStore(&cursor[c], acc);
        acc += atomicLoad(&counts[c]);
    }
    // El total en la última posición: `starts[ncells]` debe valer N (esto es
    // lo que valida el readback de arranque).
    starts[ncells] = acc;
}

@compute @workgroup_size(256)
fn scatter(@builtin(global_invocation_id) gid: vec3u) {
    let i = gid.x;
    if i >= P.count {
        return;
    }
    let dst = atomicAdd(&cursor[cell_of(pos[i])], 1u);
    items[dst] = i;
}
