// Superposición de una imagen encima de la escena de partículas: dibuja un
// quad del tamaño de la foto (centrado, en la caja `half` en NDC) muestreando
// la textura, con opacidad `reveal` para el fundido partículas → foto. Con
// mezcla alfa normal sobre lo ya pintado, así las partículas quedan debajo.

struct PhotoOverlay {
    // Mitad de la caja de la foto en NDC (el quad va de -half a +half).
    half: vec2f,
    // Opacidad del fundido (0 = solo partículas, 1 = foto completa).
    reveal: f32,
    _pad: f32,
};

@group(0) @binding(0) var<uniform> P: PhotoOverlay;
@group(0) @binding(1) var tex: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

struct VsOut {
    @builtin(position) clip: vec4f,
    @location(0) uv: vec2f,
};

@vertex
fn vs_photo(@builtin(vertex_index) vi: u32) -> VsOut {
    // Triángulo-strip: esquinas (0,0),(1,0),(0,1),(1,1).
    let c = vec2f(f32(vi & 1u), f32(vi >> 1u));
    var out: VsOut;
    // De 0..1 a -half..+half en NDC.
    out.clip = vec4f((c * 2.0 - 1.0) * P.half, 0.0, 1.0);
    // La fila 0 de la imagen (arriba) debe quedar arriba (NDC y=+half): la v
    // se invierte respecto a la coordenada del quad.
    out.uv = vec2f(c.x, 1.0 - c.y);
    return out;
}

@fragment
fn fs_photo(in: VsOut) -> @location(0) vec4f {
    let col = textureSample(tex, samp, in.uv);
    return vec4f(col.rgb, col.a * P.reveal);
}
