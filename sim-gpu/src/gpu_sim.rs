//! Simulación y render de partículas en GPU (wgpu).
//!
//! Hitos 1-2: física en compute (naive O(n²) y grid CSR construido en GPU,
//! alternables con `use_grid`) con ping-pong de posición/velocidad; las
//! partículas nunca vuelven a la CPU después del sembrado.
//!
//! Hito 3 (paridad de modos): los kernels evalúan TODOS los `InteractionMode`
//! (port de `coef_raw`), la bandada Boids, ambos contornos y las transiciones
//! (cruce origen→objetivo + `boids_mix`); los parámetros suben cada frame, así
//! los blends que conduce la CPU (escenas, velocidad suave, deriva de la
//! matriz) se reflejan al instante. El grid se redimensiona en caliente al
//! cambiar `r_max` (buffers dimensionados al peor caso `MIN_CELL`).
//!
//! Hito 4 (paridad visual): el render pinta sobre una textura HDR
//! (rgba16float) persistente — estelas por desvanecido, bloom aditivo por
//! partícula, estilos Brillo/Sólido/Sólido+halo y flechas orientadas por la
//! velocidad (mezcla `orient`) — y un blit final la vuelca a la superficie.
//!
//! Hito 6: formas/texto — los puntos meta se rasterizan en la CPU (shape.rs)
//! y suben a un buffer; el kernel aplica el resorte y las reglas texto↔fondo
//! a las primeras `n_shape` partículas (factores en `ShapeDrive`). Grabación:
//! `blit_to` vuelca la escena (sin panel) a una textura rgba8 externa que el
//! Recorder (rec.rs) copia a staging y manda a ffmpeg.
//!
//! Sin paridad (documentado): la dinámica de color por partícula
//! (`random_color`/`gradual_color_speed`), que muta datos por partícula en la
//! CPU; la deriva de la MATRIZ (`gradual_matrix_speed`) sí funciona porque
//! vive en los parámetros.

use rand::Rng;
use shared::{
    hue_for_index, BoidsScope, Boundary, InteractionMode, RenderStyle, SimParams, NUM_COLORS,
};
use wgpu::util::DeviceExt;

/// Formato HDR del render fuera de pantalla: el bloom aditivo puede superar
/// 1.0 sin recortarse hasta el blit final.
const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// Celda mínima del grid: los buffers se dimensionan para este peor caso y
/// las dimensiones reales se recalculan de `r_max` en cada subida (el slider
/// de `r_max` no baja de 20, así el 3×3 de vecinas siempre cubre el radio).
const MIN_CELL: f32 = 20.0;

fn mode_code(m: InteractionMode) -> u32 {
    match m {
        InteractionMode::SameColorOnly => 0,
        InteractionMode::Matrix => 1,
        InteractionMode::Similarity => 2,
        InteractionMode::Cyclic => 3,
        InteractionMode::Opposite => 4,
        InteractionMode::PredatorPrey => 5,
        InteractionMode::SelfRepel => 6,
        InteractionMode::Boids => 7,
    }
}

fn scope_code(s: BoidsScope) -> u32 {
    match s {
        BoidsScope::All => 0,
        BoidsScope::SameColor => 1,
        BoidsScope::Hybrid => 2,
    }
}

fn style_code(s: RenderStyle) -> u32 {
    match s {
        RenderStyle::Glow => 0,
        RenderStyle::Solid => 1,
        RenderStyle::SolidHalo => 2,
    }
}

fn pack_matrix(m: &[[f32; NUM_COLORS]; NUM_COLORS]) -> [[f32; 4]; 9] {
    let mut out = [[0.0f32; 4]; 9];
    for i in 0..NUM_COLORS {
        for j in 0..NUM_COLORS {
            let idx = i * NUM_COLORS + j;
            out[idx / 4][idx % 4] = m[i][j];
        }
    }
    out
}

/// Progreso ya suavizado (ease-in-out) del cruce de interacción, 1 = sin
/// transición (el mismo cálculo que `SimParams::interaction()`).
fn eased_blend(p: &SimParams) -> f32 {
    if p.blend >= 1.0 || !p.smooth {
        1.0
    } else {
        let b = p.blend;
        b * b * (3.0 - 2.0 * b)
    }
}

/// Mezcla radial↔bandada con el mismo ease que el cruce (= el `boids_mix`
/// local de `Simulation::step` en la CPU).
fn boids_mix(p: &SimParams) -> f32 {
    let to = if p.mode == InteractionMode::Boids { 1.0 } else { 0.0 };
    if p.smooth && p.blend < 1.0 {
        let from = if p.from_state.mode == InteractionMode::Boids { 1.0 } else { 0.0 };
        let b = p.blend;
        let t = b * b * (3.0 - 2.0 * b);
        from + (to - from) * t
    } else {
        to
    }
}

/// Factores de la forma activa que consumen los kernels, ya escalados por la
/// mezcla de aparición (los calcula `ShapeState::drive` en main.rs a partir
/// de `shape_strength` y el blend, como en `Simulation::step` de la CPU).
pub struct ShapeDrive {
    /// Partículas asignadas a la forma (0 = sin forma).
    pub n: u32,
    /// Rigidez del resorte hacia el punto meta.
    pub k: f32,
    /// Interacción residual del texto (1 = plena, baja con la fijación).
    pub inter: f32,
    /// Repulsión del fondo hacia el texto.
    pub avoid: f32,
}

impl Default for ShapeDrive {
    fn default() -> Self {
        Self { n: 0, k: 0.0, inter: 1.0, avoid: 0.0 }
    }
}

/// Espejo plano de los parámetros que usan los kernels (ver params.wgsl). Las
/// matrices 6×6 viajan empaquetadas en 9 vec4 por la alineación de uniform.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GpuParams {
    world: [f32; 2],
    dt: f32,
    friction: f32,
    force: f32,
    r_max: f32,
    beta: f32,
    count: u32,
    cols: i32,
    rows: i32,
    inv_cell: f32,
    boundary: u32,
    mode: u32,
    sim_range: f32,
    same_repel: u32,
    same_strength: f32,
    from_mode: u32,
    from_sim_range: f32,
    from_same_repel: u32,
    from_same_strength: f32,
    blend_t: f32,
    boids_mix: f32,
    boids_scope: u32,
    w_sep: f32,
    w_ali: f32,
    w_coh: f32,
    w_grp: f32,
    sep_r: f32,
    cruise: f32,
    wall_turn: f32,
    attract: u32,
    attract_strength: f32,
    n_shape: u32,
    shape_k: f32,
    shape_inter: f32,
    shape_avoid: f32,
    matrix: [[f32; 4]; 9],
    from_matrix: [[f32; 4]; 9],
}

impl GpuParams {
    fn from(
        params: &SimParams,
        world: [f32; 2],
        count: u32,
        grid: &GridDims,
        shape: &ShapeDrive,
    ) -> Self {
        Self {
            world,
            dt: params.time_scale,
            friction: params.friction,
            force: params.force,
            r_max: params.r_max,
            beta: params.beta,
            count,
            cols: grid.cols,
            rows: grid.rows,
            inv_cell: grid.inv_cell,
            boundary: match params.boundary {
                Boundary::Wrap => 0,
                Boundary::Bounce => 1,
            },
            mode: mode_code(params.mode),
            sim_range: params.sim_range,
            same_repel: params.same_repel_others as u32,
            same_strength: params.same_repel_strength,
            from_mode: mode_code(params.from_state.mode),
            from_sim_range: params.from_state.sim_range,
            from_same_repel: params.from_state.same_repel_others as u32,
            from_same_strength: params.from_state.same_repel_strength,
            blend_t: eased_blend(params),
            boids_mix: boids_mix(params),
            boids_scope: scope_code(params.boids_scope),
            w_sep: params.boids_separation,
            w_ali: params.boids_alignment,
            w_coh: params.boids_cohesion,
            w_grp: params.boids_group_avoid,
            sep_r: (params.boids_sep_radius * params.r_max).max(1.0),
            cruise: params.boids_cruise,
            wall_turn: params.boids_cruise.max(1.0) * 1.5,
            attract: params.attract_active as u32,
            attract_strength: params.attract_active_strength,
            n_shape: shape.n.min(count),
            shape_k: shape.k,
            shape_inter: shape.inter,
            shape_avoid: shape.avoid,
            matrix: pack_matrix(&params.matrix),
            from_matrix: pack_matrix(&params.from_state.matrix),
        }
    }
}

/// Uniform del render (ver particles.wgsl / post.wgsl).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct RenderParams {
    scale: [f32; 2],
    offset: [f32; 2],
    point_size: f32,
    style: u32,
    brightness: f32,
    orient: f32,
    bloom_intensity: f32,
    bloom_radius: f32,
    trail_fade: f32,
    _pad: f32,
}

impl RenderParams {
    fn from(params: &SimParams, world: [f32; 2]) -> Self {
        Self {
            // Mundo completo a NDC; la Y del mundo crece hacia abajo.
            scale: [2.0 / world[0], -2.0 / world[1]],
            offset: [-1.0, 1.0],
            point_size: params.point_size,
            style: style_code(params.style),
            brightness: params.brightness.clamp(0.0, 1.0),
            orient: params.orient.clamp(0.0, 1.0),
            bloom_intensity: if params.bloom { params.bloom_intensity } else { 0.0 },
            bloom_radius: params.bloom_radius,
            trail_fade: params.trail_fade.clamp(0.0, 1.0),
            _pad: 0.0,
        }
    }
}

/// Dimensiones del grid (celda = r_max). Se recalculan al subir parámetros;
/// los buffers están dimensionados para el peor caso (`MIN_CELL`).
#[derive(Clone, Copy)]
struct GridDims {
    cols: i32,
    rows: i32,
    inv_cell: f32,
}

impl GridDims {
    fn new(world: [f32; 2], r_max: f32) -> Self {
        let cell = r_max.max(MIN_CELL);
        Self {
            cols: ((world[0] / cell).ceil() as i32).max(1),
            rows: ((world[1] / cell).ceil() as i32).max(1),
            inv_cell: 1.0 / cell,
        }
    }

    fn ncells(&self) -> u64 {
        (self.cols as u64) * (self.rows as u64)
    }
}

pub struct GpuSim {
    /// Partículas sembradas como máximo (tamaño de los buffers).
    pub capacity: u32,
    /// Partículas vivas (las que pisan los kernels y el render).
    pub count: u32,
    pub world: [f32; 2],
    /// Fuerzas por el grid (true) o naive O(n²) (false). Mismo comportamiento
    /// estadístico; el grid escala a cientos de miles de partículas.
    pub use_grid: bool,
    grid: GridDims,
    // Puertas del render, copiadas de los últimos parámetros subidos.
    trails: bool,
    bloom_on: bool,
    disc_alpha: f32,
    arrow_alpha: f32,
    /// Limpiar la textura persistente en el próximo frame (arranque/resize).
    clear_pending: bool,
    params_buf: wgpu::Buffer,
    render_buf: wgpu::Buffer,
    pos_bufs: [wgpu::Buffer; 2],
    vel_bufs: [wgpu::Buffer; 2],
    hue_buf: wgpu::Buffer,
    shape_buf: wgpu::Buffer,
    counts_buf: wgpu::Buffer,
    starts_buf: wgpu::Buffer,
    pipeline_naive: wgpu::ComputePipeline,
    pipeline_grid: wgpu::ComputePipeline,
    pipeline_count: wgpu::ComputePipeline,
    pipeline_prefix: wgpu::ComputePipeline,
    pipeline_scatter: wgpu::ComputePipeline,
    pipeline_disc: wgpu::RenderPipeline,
    pipeline_arrow: wgpu::RenderPipeline,
    pipeline_bloom: wgpu::RenderPipeline,
    pipeline_fade: wgpu::RenderPipeline,
    pipeline_blit: wgpu::RenderPipeline,
    pipeline_blit_rec: wgpu::RenderPipeline,
    /// Bind groups del compute para cada sentido del ping-pong (A→B, B→A).
    compute_bind: [wgpu::BindGroup; 2],
    /// Grid CSR de solo lectura para las fuerzas (group 1 de sim.wgsl).
    grid_read_bind: wgpu::BindGroup,
    /// Bind groups de la construcción del grid, uno por buffer de entrada.
    grid_build_bind: [wgpu::BindGroup; 2],
    /// Bind groups del render para leer el buffer recién escrito (B, A).
    render_bind: [wgpu::BindGroup; 2],
    blit_layout: wgpu::BindGroupLayout,
    blit_bind: wgpu::BindGroup,
    sampler: wgpu::Sampler,
    offscreen_view: wgpu::TextureView,
    /// Qué sentido toca este frame (0 = A→B, 1 = B→A).
    flip: usize,
    /// Último sentido computado: `render_bind[last]` lee los datos frescos
    /// (en pausa se sigue pintando este, no el del sentido pendiente).
    last: usize,
}

impl GpuSim {
    pub fn new(
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        params: &SimParams,
        world: [f32; 2],
        count: u32,
        size: (u32, u32),
        rng: &mut impl Rng,
    ) -> Self {
        let grid = GridDims::new(world, params.r_max);
        // Buffers del grid dimensionados al peor caso (r_max mínimo).
        let max_cells = GridDims::new(world, MIN_CELL).ncells();

        // --- Sembrado en CPU (posición aleatoria, color por cubos) ---
        let (pos, hue) = seed_data(world, count, rng);

        let storage = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST;
        let pos_a = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("pos_a"),
            contents: bytemuck::cast_slice(&pos),
            usage: storage,
        });
        let pos_b = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pos_b"),
            size: (count as u64) * 8,
            usage: storage,
            mapped_at_creation: false,
        });
        let zero_vel = vec![[0.0f32; 2]; count as usize];
        let vel_a = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vel_a"),
            contents: bytemuck::cast_slice(&zero_vel),
            usage: storage,
        });
        let vel_b = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vel_b"),
            size: (count as u64) * 8,
            usage: storage,
            mapped_at_creation: false,
        });
        let hue_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("hue"),
            contents: bytemuck::cast_slice(&hue),
            usage: storage,
        });
        // Puntos meta de la forma (capacidad completa, arranca vacío/a cero).
        let shape_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shape targets"),
            size: (count as u64) * 8,
            usage: storage,
            mapped_at_creation: false,
        });

        // --- Buffers del grid CSR ---
        let counts_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("grid counts"),
            size: max_cells * 4,
            usage: storage,
            mapped_at_creation: false,
        });
        let starts_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("grid starts"),
            size: (max_cells + 1) * 4,
            // COPY_SRC: el readback de validación lee el total (starts[ncells]).
            usage: storage | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let cursor_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("grid cursor"),
            size: max_cells * 4,
            usage: storage,
            mapped_at_creation: false,
        });
        let items_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("grid items"),
            size: (count as u64) * 4,
            usage: storage,
            mapped_at_creation: false,
        });

        let params_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("params"),
            contents: bytemuck::bytes_of(&GpuParams::from(
                params,
                world,
                count,
                &grid,
                &ShapeDrive::default(),
            )),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let render_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("render params"),
            contents: bytemuck::bytes_of(&RenderParams::from(params, world)),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        // --- Layouts auxiliares ---
        let uniform_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let ro = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let rw = |binding| wgpu::BindGroupLayoutEntry {
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: false },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            ..ro(binding)
        };

        // La struct Params compartida se antepone a los módulos de compute.
        let prelude = include_str!("shaders/params.wgsl");

        // --- Kernels de física (sim.wgsl): naive y por grid ---
        let sim_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sim.wgsl"),
            source: wgpu::ShaderSource::Wgsl(
                format!("{prelude}\n{}", include_str!("shaders/sim.wgsl")).into(),
            ),
        });
        let compute_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("compute layout"),
            entries: &[uniform_entry(0), ro(1), ro(2), rw(3), rw(4), ro(5), ro(6)],
        });
        let grid_read_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("grid read layout"),
            entries: &[ro(0), ro(1)],
        });
        let sim_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None,
                bind_group_layouts: &[&compute_layout, &grid_read_layout],
                push_constant_ranges: &[],
            });
        let mk_sim_pipeline = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&sim_pipeline_layout),
                module: &sim_shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let pipeline_naive = mk_sim_pipeline("step");
        let pipeline_grid = mk_sim_pipeline("step_grid");

        let mk_compute_bind = |src_pos: &wgpu::Buffer,
                               src_vel: &wgpu::Buffer,
                               dst_pos: &wgpu::Buffer,
                               dst_vel: &wgpu::Buffer| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("compute bind"),
                layout: &compute_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: params_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: src_pos.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: src_vel.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 3, resource: dst_pos.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 4, resource: dst_vel.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 5, resource: hue_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 6, resource: shape_buf.as_entire_binding() },
                ],
            })
        };
        let compute_bind = [
            mk_compute_bind(&pos_a, &vel_a, &pos_b, &vel_b),
            mk_compute_bind(&pos_b, &vel_b, &pos_a, &vel_a),
        ];
        let grid_read_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("grid read bind"),
            layout: &grid_read_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: starts_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: items_buf.as_entire_binding() },
            ],
        });

        // --- Construcción del grid (grid.wgsl: count / prefix / scatter) ---
        let grid_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("grid.wgsl"),
            source: wgpu::ShaderSource::Wgsl(
                format!("{prelude}\n{}", include_str!("shaders/grid.wgsl")).into(),
            ),
        });
        let grid_build_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("grid build layout"),
            entries: &[uniform_entry(0), ro(1), rw(2), rw(3), rw(4), rw(5)],
        });
        let grid_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None,
                bind_group_layouts: &[&grid_build_layout],
                push_constant_ranges: &[],
            });
        let mk_grid_pipeline = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&grid_pipeline_layout),
                module: &grid_shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let pipeline_count = mk_grid_pipeline("count");
        let pipeline_prefix = mk_grid_pipeline("prefix");
        let pipeline_scatter = mk_grid_pipeline("scatter");

        let mk_grid_build_bind = |pos: &wgpu::Buffer| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("grid build bind"),
                layout: &grid_build_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: params_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: pos.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: counts_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 3, resource: starts_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 4, resource: cursor_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 5, resource: items_buf.as_entire_binding() },
                ],
            })
        };
        // El grid se construye sobre la entrada del paso: A cuando flip=0, B
        // cuando flip=1 (misma indexación que `compute_bind`).
        let grid_build_bind = [mk_grid_build_bind(&pos_a), mk_grid_build_bind(&pos_b)];

        // --- Pipelines de render (particles.wgsl + post.wgsl) ---
        let particles_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("particles.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/particles.wgsl").into()),
        });
        let post_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("post.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/post.wgsl").into()),
        });

        let vs_ro = |binding| wgpu::BindGroupLayoutEntry {
            visibility: wgpu::ShaderStages::VERTEX,
            ..ro(binding)
        };
        let render_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("render layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    // El fragmento también lee el uniform (estilo, fade...).
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ..uniform_entry(0)
                },
                vs_ro(1),
                vs_ro(2),
                vs_ro(3),
            ],
        });
        let blit_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blit layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        // Mezcla alfa normal (discos, flechas, fade) y aditiva ponderada por
        // alfa (bloom, como el material aditivo de la CPU).
        let additive = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::OVER,
        };
        let render_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None,
                bind_group_layouts: &[&render_layout],
                push_constant_ranges: &[],
            });
        let blit_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None,
                bind_group_layouts: &[&blit_layout],
                push_constant_ranges: &[],
            });
        let mk_render_pipeline = |label: &str,
                                  module: &wgpu::ShaderModule,
                                  layout: &wgpu::PipelineLayout,
                                  vs: &str,
                                  fs: &str,
                                  blend: Option<wgpu::BlendState>,
                                  topology: wgpu::PrimitiveTopology,
                                  format: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: Default::default(),
                multiview: None,
                cache: None,
            })
        };
        use wgpu::PrimitiveTopology::{TriangleList, TriangleStrip};
        let pipeline_disc = mk_render_pipeline(
            "discos", &particles_shader, &render_pipeline_layout, "vs_disc", "fs_disc",
            Some(wgpu::BlendState::ALPHA_BLENDING), TriangleStrip, OFFSCREEN_FORMAT,
        );
        let pipeline_bloom = mk_render_pipeline(
            "bloom", &particles_shader, &render_pipeline_layout, "vs_bloom", "fs_bloom",
            Some(additive), TriangleStrip, OFFSCREEN_FORMAT,
        );
        let pipeline_arrow = mk_render_pipeline(
            "flechas", &particles_shader, &render_pipeline_layout, "vs_arrow", "fs_arrow",
            Some(wgpu::BlendState::ALPHA_BLENDING), TriangleList, OFFSCREEN_FORMAT,
        );
        let pipeline_fade = mk_render_pipeline(
            "fade estelas", &post_shader, &render_pipeline_layout, "vs_fullscreen", "fs_fade",
            Some(wgpu::BlendState::ALPHA_BLENDING), TriangleList, OFFSCREEN_FORMAT,
        );
        let pipeline_blit = mk_render_pipeline(
            "blit", &post_shader, &blit_pipeline_layout, "vs_fullscreen", "fs_blit",
            None, TriangleList, surface_format,
        );
        // Blit de grabación: la misma escena a la textura rgba8 del Recorder
        // (sin el panel egui, que se pinta después sobre la superficie).
        let pipeline_blit_rec = mk_render_pipeline(
            "blit grabación", &post_shader, &blit_pipeline_layout, "vs_fullscreen", "fs_blit",
            None, TriangleList, wgpu::TextureFormat::Rgba8Unorm,
        );

        let mk_render_bind = |pos: &wgpu::Buffer, vel: &wgpu::Buffer| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("render bind"),
                layout: &render_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: render_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: pos.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: hue_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 3, resource: vel.as_entire_binding() },
                ],
            })
        };
        // Tras el paso A→B se pinta B; tras B→A se pinta A.
        let render_bind = [mk_render_bind(&pos_b, &vel_b), mk_render_bind(&pos_a, &vel_a)];

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let offscreen_view = make_offscreen(device, size.0, size.1);
        let blit_bind = mk_blit_bind(device, &blit_layout, &offscreen_view, &sampler);

        Self {
            capacity: count,
            count,
            world,
            use_grid: true,
            grid,
            trails: params.trails,
            bloom_on: params.bloom && params.bloom_intensity > 0.001,
            disc_alpha: params.brightness * (1.0 - params.orient),
            arrow_alpha: params.brightness * params.orient,
            clear_pending: true,
            params_buf,
            render_buf,
            pos_bufs: [pos_a, pos_b],
            vel_bufs: [vel_a, vel_b],
            hue_buf,
            shape_buf,
            counts_buf,
            starts_buf,
            pipeline_naive,
            pipeline_grid,
            pipeline_count,
            pipeline_prefix,
            pipeline_scatter,
            pipeline_disc,
            pipeline_arrow,
            pipeline_bloom,
            pipeline_fade,
            pipeline_blit,
            pipeline_blit_rec,
            compute_bind,
            grid_read_bind,
            grid_build_bind,
            render_bind,
            blit_layout,
            blit_bind,
            sampler,
            offscreen_view,
            flip: 0,
            // Sin ningún paso dado, los datos iniciales viven en A (= bind 1).
            last: 1,
        }
    }

    /// Recrea la textura HDR fuera de pantalla al cambiar el tamaño de la
    /// ventana (las estelas acumuladas se pierden: se limpia en el próximo frame).
    pub fn resize(&mut self, device: &wgpu::Device, w: u32, h: u32) {
        self.offscreen_view = make_offscreen(device, w, h);
        self.blit_bind = mk_blit_bind(device, &self.blit_layout, &self.offscreen_view, &self.sampler);
        self.clear_pending = true;
    }

    /// Re-sube TODOS los parámetros (física + render). Se llama cada frame:
    /// así los blends que conduce la CPU (transiciones de escena/interacción/
    /// velocidad, deriva de la matriz) se reflejan al instante, y el grid se
    /// redimensiona en caliente si cambió `r_max`.
    pub fn upload_params(&mut self, queue: &wgpu::Queue, params: &SimParams, shape: &ShapeDrive) {
        self.grid = GridDims::new(self.world, params.r_max);
        let gp = GpuParams::from(params, self.world, self.count, &self.grid, shape);
        queue.write_buffer(&self.params_buf, 0, bytemuck::bytes_of(&gp));
        let rp = RenderParams::from(params, self.world);
        queue.write_buffer(&self.render_buf, 0, bytemuck::bytes_of(&rp));
        // Si las estelas se acaban de apagar, el Clear por frame ya limpia.
        self.trails = params.trails;
        self.bloom_on = params.bloom && params.bloom_intensity > 0.001;
        self.disc_alpha = rp.brightness * (1.0 - rp.orient);
        self.arrow_alpha = rp.brightness * rp.orient;
    }

    /// Vuelve a sembrar `n` partículas (≤ capacidad) con posición aleatoria y
    /// velocidad cero, escribiendo sobre el buffer de ENTRADA del próximo paso.
    /// Con `n = 0` vacía el lienzo.
    pub fn reseed(&mut self, queue: &wgpu::Queue, n: u32, rng: &mut impl Rng) {
        let n = n.min(self.capacity);
        if n > 0 {
            let (pos, hue) = seed_data(self.world, n, rng);
            let vel = vec![[0.0f32; 2]; n as usize];
            queue.write_buffer(&self.pos_bufs[self.flip], 0, bytemuck::cast_slice(&pos));
            queue.write_buffer(&self.vel_bufs[self.flip], 0, bytemuck::cast_slice(&vel));
            queue.write_buffer(&self.hue_buf, 0, bytemuck::cast_slice(&hue));
        }
        self.count = n;
        // Los datos frescos viven en el buffer fuente del sentido `flip`:
        // pintarlo también mientras estemos en pausa.
        self.last = self.flip ^ 1;
    }

    /// Sube los puntos meta de la forma (recortados a la capacidad) y devuelve
    /// cuántos quedaron activos.
    pub fn upload_shape_targets(&self, queue: &wgpu::Queue, pts: &[[f32; 2]]) -> u32 {
        let n = pts.len().min(self.capacity as usize);
        if n > 0 {
            queue.write_buffer(&self.shape_buf, 0, bytemuck::cast_slice(&pts[..n]));
        }
        n as u32
    }

    /// Tiñe del matiz indicado las primeras `n` partículas (las de la forma).
    pub fn tint(&self, queue: &wgpu::Queue, n: u32, hue: f32) {
        let n = n.min(self.capacity) as usize;
        if n > 0 {
            queue.write_buffer(&self.hue_buf, 0, bytemuck::cast_slice(&vec![hue; n]));
        }
    }

    /// Vuelca la escena HDR (sin el panel) a una textura externa, p. ej. la
    /// rgba8 del Recorder de vídeo.
    pub fn blit_to(&self, encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("blit grabación"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.pipeline_blit_rec);
        pass.set_bind_group(0, &self.blit_bind, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Encola las tres pasadas que construyen el grid CSR para la entrada del
    /// sentido `flip` (contador limpio + histograma + prefijo + colocación).
    fn build_grid(&self, encoder: &mut wgpu::CommandEncoder) {
        encoder.clear_buffer(&self.counts_buf, 0, None);
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("grid build"),
            timestamp_writes: None,
        });
        let groups = self.count.div_ceil(256);
        pass.set_bind_group(0, &self.grid_build_bind[self.flip], &[]);
        pass.set_pipeline(&self.pipeline_count);
        pass.dispatch_workgroups(groups, 1, 1);
        pass.set_pipeline(&self.pipeline_prefix);
        pass.dispatch_workgroups(1, 1, 1);
        pass.set_pipeline(&self.pipeline_scatter);
        pass.dispatch_workgroups(groups, 1, 1);
    }

    /// Un paso de física + el render del resultado sobre `surface_view`
    /// (escena HDR con estelas/bloom/estilos y blit final).
    pub fn frame(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        surface_view: &wgpu::TextureView,
        paused: bool,
    ) {
        if !paused && self.count > 0 {
            if self.use_grid {
                self.build_grid(encoder);
            }
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("step"),
                timestamp_writes: None,
            });
            pass.set_pipeline(if self.use_grid {
                &self.pipeline_grid
            } else {
                &self.pipeline_naive
            });
            pass.set_bind_group(0, &self.compute_bind[self.flip], &[]);
            pass.set_bind_group(1, &self.grid_read_bind, &[]);
            pass.dispatch_workgroups(self.count.div_ceil(256), 1, 1);
            self.last = self.flip;
            self.flip ^= 1;
        }

        // --- Escena sobre la textura HDR (persistente si hay estelas) ---
        {
            let keep = self.trails && !self.clear_pending;
            self.clear_pending = false;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("escena"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.offscreen_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: if keep {
                            wgpu::LoadOp::Load
                        } else {
                            wgpu::LoadOp::Clear(wgpu::Color::BLACK)
                        },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_bind_group(0, &self.render_bind[self.last], &[]);
            if keep {
                // Desvanecer el frame anterior (estela más larga = menos fade).
                pass.set_pipeline(&self.pipeline_fade);
                pass.draw(0..3, 0..1);
            }
            if self.count > 0 {
                // Bloom por debajo, luego discos y flechas (mezcla `orient`).
                if self.bloom_on {
                    pass.set_pipeline(&self.pipeline_bloom);
                    pass.draw(0..4, 0..self.count);
                }
                if self.disc_alpha > 0.003 {
                    pass.set_pipeline(&self.pipeline_disc);
                    pass.draw(0..4, 0..self.count);
                }
                if self.arrow_alpha > 0.003 {
                    pass.set_pipeline(&self.pipeline_arrow);
                    pass.draw(0..3, 0..self.count);
                }
            }
        }

        // --- Blit de la escena a la superficie ---
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("blit"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: surface_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline_blit);
            pass.set_bind_group(0, &self.blit_bind, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    /// Validación del prefix sum: construye el grid y lee de vuelta el total
    /// (`starts[ncells]`), que debe ser exactamente N. Se llama una vez al
    /// arrancar; un fallo aquí indicaría un scan roto (fuerzas corruptas).
    pub fn validate_grid(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<(), String> {
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("grid validate staging"),
            size: 4,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        self.build_grid(&mut encoder);
        encoder.copy_buffer_to_buffer(&self.starts_buf, self.grid.ncells() * 4, &staging, 0, 4);
        queue.submit([encoder.finish()]);

        let slice = staging.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        device.poll(wgpu::Maintain::Wait);
        rx.recv()
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        let total = u32::from_le_bytes(slice.get_mapped_range()[..4].try_into().unwrap());
        staging.unmap();
        if total == self.count {
            Ok(())
        } else {
            Err(format!(
                "prefix sum roto: total {total} != {} partículas",
                self.count
            ))
        }
    }
}

/// Posiciones aleatorias + matiz por cubos para `n` partículas.
fn seed_data(world: [f32; 2], n: u32, rng: &mut impl Rng) -> (Vec<[f32; 2]>, Vec<f32>) {
    let mut pos = Vec::with_capacity(n as usize);
    let mut hue = Vec::with_capacity(n as usize);
    for _ in 0..n {
        pos.push([rng.gen_range(0.0..world[0]), rng.gen_range(0.0..world[1])]);
        hue.push(hue_for_index(rng.gen_range(0..NUM_COLORS)));
    }
    (pos, hue)
}

fn make_offscreen(device: &wgpu::Device, w: u32, h: u32) -> wgpu::TextureView {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("escena HDR"),
        size: wgpu::Extent3d {
            width: w.max(1),
            height: h.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: OFFSCREEN_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    tex.create_view(&wgpu::TextureViewDescriptor::default())
}

fn mk_blit_bind(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("blit bind"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(view) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(sampler) },
        ],
    })
}
