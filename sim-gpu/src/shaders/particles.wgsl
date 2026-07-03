// Render de partículas: un quad por instancia leído directo del buffer de
// posiciones de la simulación (cero copias CPU↔GPU), con caída radial de brillo
// en el fragmento (estilo "glow") y mezcla ADITIVA (los solapes suman → neón).

struct Camera {
    // mundo → NDC: ndc = pos * scale + offset (la Y del mundo crece hacia
    // abajo, como en la CPU; el flip vive en `scale.y`).
    scale: vec2f,
    offset: vec2f,
    // Semitamaño del quad en unidades de mundo.
    point_size: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
};

@group(0) @binding(0) var<uniform> cam: Camera;
@group(0) @binding(1) var<storage, read> pos: array<vec2f>;
@group(0) @binding(2) var<storage, read> hue: array<f32>;

struct VsOut {
    @builtin(position) clip: vec4f,
    @location(0) uv: vec2f,
    @location(1) color: vec3f,
};

// Rueda de color continua (= `color_for_hue` de la CPU).
fn color_for_hue(h: f32) -> vec3f {
    let h6 = fract(h) * 6.0;
    let i = i32(floor(h6));
    let f = h6 - floor(h6);
    switch i {
        case 0: { return vec3f(1.0, f, 0.0); }
        case 1: { return vec3f(1.0 - f, 1.0, 0.0); }
        case 2: { return vec3f(0.0, 1.0, f); }
        case 3: { return vec3f(0.0, 1.0 - f, 1.0); }
        case 4: { return vec3f(f, 0.0, 1.0); }
        default: { return vec3f(1.0, 0.0, 1.0 - f); }
    }
}

@vertex
fn vs(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VsOut {
    // Esquina del quad en triangle-strip: (-1,-1) (1,-1) (-1,1) (1,1).
    let corner = vec2f(
        f32(vi & 1u) * 2.0 - 1.0,
        f32(vi >> 1u) * 2.0 - 1.0,
    );
    let world = pos[ii] + corner * cam.point_size;
    var out: VsOut;
    out.clip = vec4f(world * cam.scale + cam.offset, 0.0, 1.0);
    out.uv = corner;
    out.color = color_for_hue(hue[ii]);
    return out;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4f {
    // Glow: brilla en el centro y cae suave (misma curva que la textura CPU).
    let d = min(length(in.uv), 1.0);
    let a = pow(1.0 - d, 2.2);
    return vec4f(in.color * a, a);
}
