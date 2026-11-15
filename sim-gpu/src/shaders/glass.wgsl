// Cristal esmerilado detrás del panel: la escena HDR se reduce a 1/4, se
// desenfoca con un gauss separable (2 iteraciones H+V) y se compone en la
// superficie solo dentro del rect de la lámina (scissor). Después egui pinta
// encima el tinte translúcido de la lámina y los controles.

struct G {
    texel: vec2f,   // 1 / tamaño de la textura de destino (1/4 de ventana)
    dir: vec2f,     // (1,0) horizontal · (0,1) vertical · (0,0) sin uso
    spread: f32,
    _p0: f32,
    _p1: f32,
    _p2: f32,
};

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var smp: sampler;
@group(0) @binding(2) var<uniform> g: G;

struct FsIn {
    @builtin(position) clip: vec4f,
    @location(0) uv: vec2f,
};

// Triángulo que cubre toda la pantalla (igual que post.wgsl).
@vertex
fn vs_fullscreen(@builtin(vertex_index) vi: u32) -> FsIn {
    let xy = vec2f(f32((vi << 1u) & 2u), f32(vi & 2u));
    var out: FsIn;
    out.clip = vec4f(xy * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2f(xy.x, 1.0 - xy.y);
    return out;
}

// Reducción: 4 lecturas bilineales (= caja 4×4 de la fuente).
@fragment
fn fs_down(in: FsIn) -> @location(0) vec4f {
    let o = g.texel * 0.5;
    let c = textureSample(src, smp, in.uv + vec2f(-o.x, -o.y))
          + textureSample(src, smp, in.uv + vec2f( o.x, -o.y))
          + textureSample(src, smp, in.uv + vec2f(-o.x,  o.y))
          + textureSample(src, smp, in.uv + vec2f( o.x,  o.y));
    return vec4f(c.rgb * 0.25, 1.0);
}

// Gauss de 9 taps con 5 lecturas (aprovechando el filtrado lineal).
@fragment
fn fs_blur(in: FsIn) -> @location(0) vec4f {
    let d = g.dir * g.texel * g.spread;
    var c = textureSample(src, smp, in.uv).rgb * 0.2270270270;
    c += (textureSample(src, smp, in.uv + d * 1.3846153846).rgb
        + textureSample(src, smp, in.uv - d * 1.3846153846).rgb) * 0.3162162162;
    c += (textureSample(src, smp, in.uv + d * 3.2307692308).rgb
        + textureSample(src, smp, in.uv - d * 3.2307692308).rgb) * 0.0702702703;
    return vec4f(c, 1.0);
}

// Composición: un punto más de saturación y un techo de luminancia para que
// el texto del panel se lea aunque detrás haya un cúmulo brillante.
@fragment
fn fs_composite(in: FsIn) -> @location(0) vec4f {
    var c = textureSample(src, smp, in.uv).rgb;
    let l = dot(c, vec3f(0.2126, 0.7152, 0.0722));
    c = mix(vec3f(l), c, 1.25);
    c = clamp(c, vec3f(0.0), vec3f(0.80));
    return vec4f(c, 1.0);
}
