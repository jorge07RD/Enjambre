// Render de partículas con paridad visual con `sim/src/render.rs`: estilos
// Brillo/Sólido/Sólido+halo (perfiles de alfa portados de `make_texture`),
// flechas orientadas por la velocidad (mezcla continua disco↔flecha por
// `orient`) y bloom (halo aditivo grande por partícula, como `draw_bloom`).
// Todo se pinta sobre la textura HDR fuera de pantalla (ver gpu_sim.rs); el
// volcado a la superficie y el desvanecido de estelas viven en post.wgsl.

struct RenderParams {
    // mundo → NDC: ndc = pos * scale + offset (la Y del mundo crece hacia
    // abajo, como en la CPU; el flip vive en `scale.y`).
    scale: vec2f,
    offset: vec2f,
    point_size: f32,
    // 0 = Brillo (glow), 1 = Sólido, 2 = Sólido+halo.
    style: u32,
    brightness: f32,
    orient: f32,
    bloom_intensity: f32,
    bloom_radius: f32,
    trail_fade: f32,
    _pad: f32,
    // Mosaico (fase A): las primeras `mosaic_n` partículas (las reclutadas)
    // funden su color hacia el de la foto muestreada en su posición.
    // `mosaic_reveal` es la mezcla 0..1; center/extent = caja de la foto en
    // mundo; `mosaic_on` = modo foto activo. El resto de partículas no se tocan.
    mosaic_on: u32,
    mosaic_reveal: f32,
    mosaic_n: u32,
    _pad2: u32,
    photo_center: vec2f,
    photo_extent: vec2f,
    // Efecto "bandas → colores": niveles por banda (x=graves, y=medios,
    // z=agudos) y w=ganancia (0 = efecto apagado). Ver `band_boost`.
    bands: vec4f,
};

@group(0) @binding(0) var<uniform> R: RenderParams;
@group(0) @binding(1) var<storage, read> pos: array<vec2f>;
@group(0) @binding(2) var<storage, read> hue: array<f32>;
@group(0) @binding(3) var<storage, read> vel: array<vec2f>;
// Textura de la foto + sampler para el color del mosaico (se muestrean en el
// vértice, de ahí `textureSampleLevel`).
@group(0) @binding(4) var photo_tex: texture_2d<f32>;
@group(0) @binding(5) var photo_samp: sampler;

// Matiz [0,1) a RGB vivo (= `color_for_hue` de la CPU: HSV con s=v=1).
fn color_for_hue(h: f32) -> vec3f {
    let h6 = fract(h) * 6.0;
    let i = i32(floor(h6)) % 6;
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

// Color de la partícula `ii`: su matiz normal, fundido hacia el color de la
// foto en su posición (fase A). Solo las reclutadas (`ii < mosaic_n`); el
// resto conserva su color. La `u`/`v` coinciden con las de la superposición
// (mundo→NDC→uv), así el mosaico queda alineado con la imagen.
fn particle_color(ii: u32) -> vec3f {
    let base = color_for_hue(hue[ii]);
    if R.mosaic_on == 1u && ii < R.mosaic_n {
        let p = pos[ii];
        let u = (p.x - R.photo_center.x) / R.photo_extent.x + 0.5;
        let v = (p.y - R.photo_center.y) / R.photo_extent.y + 0.5;
        if u >= 0.0 && u <= 1.0 && v >= 0.0 && v <= 1.0 {
            let photo = textureSampleLevel(photo_tex, photo_samp, vec2f(u, v), 0.0).rgb;
            return mix(base, photo, R.mosaic_reveal);
        }
    }
    return base;
}

// Factor de brillo del efecto "bandas → colores": el cubo de matiz elige su
// banda (0-1 graves, 2-3 medios, 4-5 agudos); banda callada = tenue, banda
// fuerte = refuerzo. `bands.w` es la ganancia (audio_intensity); 0 = apagado.
fn band_boost(ii: u32) -> f32 {
    if R.bands.w <= 0.0 {
        return 1.0;
    }
    let band = min(u32(fract(hue[ii]) * 6.0), 5u) / 2u;
    var lvl = R.bands.x;
    if band == 1u {
        lvl = R.bands.y;
    } else if band == 2u {
        lvl = R.bands.z;
    }
    return clamp(0.15 + lvl * (0.85 + R.bands.w * 0.5), 0.0, 2.0);
}

struct VsOut {
    @builtin(position) clip: vec4f,
    // Coordenada -1..1 dentro del quad (distancia radial en el fragmento).
    @location(0) uv: vec2f,
    @location(1) color: vec3f,
    @location(2) alpha: f32,
};

// Quad instanciado en triangle-strip: esquinas (-1,-1),(1,-1),(-1,1),(1,1).
fn quad_vertex(vi: u32, center: vec2f, s: f32, color: vec3f, alpha: f32) -> VsOut {
    let corner = vec2f(f32(vi & 1u), f32(vi >> 1u)) * 2.0 - 1.0;
    var out: VsOut;
    out.clip = vec4f((center + corner * s) * R.scale + R.offset, 0.0, 1.0);
    out.uv = corner;
    out.color = color;
    out.alpha = alpha;
    return out;
}

// --- Discos (estilo del punto), mezcla alfa normal ---

@vertex
fn vs_disc(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VsOut {
    // El quad se extiende más allá del radio nominal según el estilo (mismos
    // factores que las texturas de la CPU).
    var mult = 1.6; // Brillo
    if R.style == 1u { mult = 1.0; } else if R.style == 2u { mult = 1.8; }
    let a = min(R.brightness * (1.0 - R.orient) * band_boost(ii), 1.0);
    return quad_vertex(vi, pos[ii], R.point_size * mult, particle_color(ii), a);
}

@fragment
fn fs_disc(in: VsOut) -> @location(0) vec4f {
    let d = min(length(in.uv), 1.0);
    var prof = 0.0;
    if R.style == 1u {
        // Disco lleno con borde antialias en el último 8%.
        prof = clamp((0.96 - d) / 0.08, 0.0, 1.0);
    } else if R.style == 2u {
        // Núcleo opaco hasta el 45% del radio + halo suave alrededor.
        if d < 0.45 {
            prof = 1.0;
        } else {
            prof = pow(clamp(1.0 - (d - 0.45) / 0.55, 0.0, 1.0), 1.8);
        }
    } else {
        // Caída suave: brilla más en el centro (glow).
        prof = pow(1.0 - d, 2.2);
    }
    return vec4f(in.color, prof * in.alpha);
}

// --- Bloom: halo grande ADITIVO por debajo de las partículas ---

@vertex
fn vs_bloom(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VsOut {
    // La intensidad va en el alfa (la mezcla aditiva pondera por alfa),
    // atenuada ×0.5 para que se acumule con gracia (= `draw_bloom`).
    let ga = clamp(R.brightness * R.bloom_intensity * 0.5 * band_boost(ii), 0.0, 1.0);
    let s = R.point_size * max(R.bloom_radius, 0.1);
    return quad_vertex(vi, pos[ii], s, particle_color(ii), ga);
}

@fragment
fn fs_bloom(in: VsOut) -> @location(0) vec4f {
    let d = min(length(in.uv), 1.0);
    return vec4f(in.color, pow(1.0 - d, 2.2) * in.alpha);
}

// --- Flechas: triángulo orientado por la velocidad ---

@vertex
fn vs_arrow(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VsOut {
    let v = vel[ii];
    let l = length(v);
    var dir = vec2f(1.0, 0.0);
    if l > 1e-5 {
        dir = v / l;
    }
    let perp = vec2f(-dir.y, dir.x);
    let s = R.point_size * 1.3;
    let p = pos[ii];
    var w: vec2f;
    if vi == 0u {
        w = p + dir * (s * 1.8); // punta
    } else if vi == 1u {
        w = p - dir * (s * 0.9) + perp * s;
    } else {
        w = p - dir * (s * 0.9) - perp * s;
    }
    var out: VsOut;
    out.clip = vec4f(w * R.scale + R.offset, 0.0, 1.0);
    out.uv = vec2f(0.0);
    out.color = particle_color(ii);
    out.alpha = min(R.brightness * R.orient * band_boost(ii), 1.0);
    return out;
}

@fragment
fn fs_arrow(in: VsOut) -> @location(0) vec4f {
    return vec4f(in.color, in.alpha);
}
