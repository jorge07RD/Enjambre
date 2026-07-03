// Post-proceso: desvanecido de las estelas (quad negro semitransparente sobre
// la textura HDR persistente, = el rectángulo de fade de la CPU) y volcado
// (blit) de esa textura a la superficie de la ventana.
//
// `fs_fade` reutiliza el mismo uniform RenderParams del render de partículas
// (solo lee `trail_fade`); `fs_blit` usa su propio layout (textura + sampler).

struct RenderParams {
    scale: vec2f,
    offset: vec2f,
    point_size: f32,
    style: u32,
    brightness: f32,
    orient: f32,
    bloom_intensity: f32,
    bloom_radius: f32,
    trail_fade: f32,
    _pad: f32,
};

@group(0) @binding(0) var<uniform> R: RenderParams;
@group(0) @binding(1) var scene_tex: texture_2d<f32>;
@group(0) @binding(2) var scene_smp: sampler;

struct FsIn {
    @builtin(position) clip: vec4f,
    @location(0) uv: vec2f,
};

// Triángulo que cubre toda la pantalla (3 vértices, sin buffers).
@vertex
fn vs_fullscreen(@builtin(vertex_index) vi: u32) -> FsIn {
    let xy = vec2f(f32((vi << 1u) & 2u), f32(vi & 2u));
    var out: FsIn;
    out.clip = vec4f(xy * 2.0 - 1.0, 0.0, 1.0);
    // La textura se muestrea con el origen arriba (Y invertida frente a NDC).
    out.uv = vec2f(xy.x, 1.0 - xy.y);
    return out;
}

@fragment
fn fs_fade(in: FsIn) -> @location(0) vec4f {
    return vec4f(0.0, 0.0, 0.0, R.trail_fade);
}

@fragment
fn fs_blit(in: FsIn) -> @location(0) vec4f {
    return vec4f(textureSample(scene_tex, scene_smp, in.uv).rgb, 1.0);
}
