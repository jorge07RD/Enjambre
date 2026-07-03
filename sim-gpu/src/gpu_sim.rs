//! Simulación y render de partículas en GPU (wgpu).
//!
//! Hito 1: fuerzas naive O(n²) en compute (modo Matriz, contorno toroidal) y
//! render instanciado aditivo leyendo el buffer de posiciones directamente.
//! Hito 2: grid espacial CSR construido en GPU (counting sort, ver grid.wgsl)
//! y kernel de fuerzas por vecinos `step_grid`; `use_grid` alterna entre ambos
//! caminos (G) para comparar comportamiento y rendimiento.
//!
//! Posición/velocidad van en doble buffer (ping-pong); las partículas nunca
//! vuelven a la CPU después del sembrado.

use rand::Rng;
use shared::{hue_for_index, SimParams, NUM_COLORS};
use wgpu::util::DeviceExt;

/// Espejo plano de los parámetros que usan los kernels (ver sim.wgsl). La
/// matriz 6×6 viaja empaquetada en 9 vec4 por la alineación de uniform.
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
    _pad: u32,
    matrix: [[f32; 4]; 9],
}

impl GpuParams {
    fn from(params: &SimParams, world: [f32; 2], count: u32, grid: &GridDims) -> Self {
        let mut matrix = [[0.0f32; 4]; 9];
        for i in 0..NUM_COLORS {
            for j in 0..NUM_COLORS {
                let idx = i * NUM_COLORS + j;
                matrix[idx / 4][idx % 4] = params.matrix[i][j];
            }
        }
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
            _pad: 0,
            matrix,
        }
    }
}

/// Dimensiones del grid (celda = r_max, como en la CPU). Fijas al arrancar:
/// si r_max cambiara en caliente habría que redimensionar los buffers.
struct GridDims {
    cols: i32,
    rows: i32,
    inv_cell: f32,
}

impl GridDims {
    fn new(world: [f32; 2], r_max: f32) -> Self {
        let cell = r_max.max(1.0);
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

/// Cámara del render: mapea el mundo completo al viewport (ver particles.wgsl).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CamUniform {
    scale: [f32; 2],
    offset: [f32; 2],
    point_size: f32,
    _pad: [f32; 3],
}

pub struct GpuSim {
    pub count: u32,
    pub world: [f32; 2],
    /// Fuerzas por el grid (true) o naive O(n²) (false). Mismo comportamiento
    /// estadístico; el grid escala a cientos de miles de partículas.
    pub use_grid: bool,
    grid: GridDims,
    params_buf: wgpu::Buffer,
    cam_buf: wgpu::Buffer,
    counts_buf: wgpu::Buffer,
    starts_buf: wgpu::Buffer,
    pipeline_naive: wgpu::ComputePipeline,
    pipeline_grid: wgpu::ComputePipeline,
    pipeline_count: wgpu::ComputePipeline,
    pipeline_prefix: wgpu::ComputePipeline,
    pipeline_scatter: wgpu::ComputePipeline,
    render_pipeline: wgpu::RenderPipeline,
    /// Bind groups del compute para cada sentido del ping-pong (A→B, B→A).
    compute_bind: [wgpu::BindGroup; 2],
    /// Grid CSR de solo lectura para las fuerzas (group 1 de sim.wgsl).
    grid_read_bind: wgpu::BindGroup,
    /// Bind groups de la construcción del grid, uno por buffer de entrada.
    grid_build_bind: [wgpu::BindGroup; 2],
    /// Bind groups del render para leer el buffer recién escrito (B, A).
    render_bind: [wgpu::BindGroup; 2],
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
        rng: &mut impl Rng,
    ) -> Self {
        let grid = GridDims::new(world, params.r_max);

        // --- Sembrado en CPU (posición aleatoria, color por cubos) ---
        let mut pos = Vec::with_capacity(count as usize);
        let mut hue = Vec::with_capacity(count as usize);
        for _ in 0..count {
            pos.push([
                rng.gen_range(0.0..world[0]),
                rng.gen_range(0.0..world[1]),
            ]);
            hue.push(hue_for_index(rng.gen_range(0..NUM_COLORS)));
        }

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

        // --- Buffers del grid CSR ---
        let ncells = grid.ncells();
        let counts_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("grid counts"),
            size: ncells * 4,
            usage: storage,
            mapped_at_creation: false,
        });
        let starts_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("grid starts"),
            size: (ncells + 1) * 4,
            // COPY_SRC: el readback de validación lee el total (starts[ncells]).
            usage: storage | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let cursor_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("grid cursor"),
            size: ncells * 4,
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
            contents: bytemuck::bytes_of(&GpuParams::from(params, world, count, &grid)),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let cam = CamUniform {
            // Mundo completo a NDC; la Y del mundo crece hacia abajo.
            scale: [2.0 / world[0], -2.0 / world[1]],
            offset: [-1.0, 1.0],
            point_size: params.point_size * 1.6,
            _pad: [0.0; 3],
        };
        let cam_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("camera"),
            contents: bytemuck::bytes_of(&cam),
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

        // --- Kernels de física (sim.wgsl): naive y por grid ---
        let sim_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sim.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/sim.wgsl").into()),
        });
        let compute_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("compute layout"),
            entries: &[uniform_entry(0), ro(1), ro(2), rw(3), rw(4), ro(5)],
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
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/grid.wgsl").into()),
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

        // --- Render pipeline (aditivo, sin depth) ---
        let particles_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("particles.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/particles.wgsl").into()),
        });
        let vs_ro = |binding| wgpu::BindGroupLayoutEntry {
            visibility: wgpu::ShaderStages::VERTEX,
            ..ro(binding)
        };
        let render_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("render layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    visibility: wgpu::ShaderStages::VERTEX,
                    ..uniform_entry(0)
                },
                vs_ro(1),
                vs_ro(2),
            ],
        });
        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("particles"),
            layout: Some(&device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None,
                bind_group_layouts: &[&render_layout],
                push_constant_ranges: &[],
            })),
            vertex: wgpu::VertexState {
                module: &particles_shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &particles_shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    // Aditivo puro: los solapes suman brillo (neón).
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent::OVER,
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        let mk_render_bind = |pos: &wgpu::Buffer| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("render bind"),
                layout: &render_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: cam_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: pos.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: hue_buf.as_entire_binding() },
                ],
            })
        };
        // Tras el paso A→B se pinta B; tras B→A se pinta A.
        let render_bind = [mk_render_bind(&pos_b), mk_render_bind(&pos_a)];

        Self {
            count,
            world,
            use_grid: true,
            grid,
            params_buf,
            cam_buf,
            counts_buf,
            starts_buf,
            pipeline_naive,
            pipeline_grid,
            pipeline_count,
            pipeline_prefix,
            pipeline_scatter,
            render_pipeline,
            compute_bind,
            grid_read_bind,
            grid_build_bind,
            render_bind,
            flip: 0,
            // Sin ningún paso dado, los datos iniciales viven en A (= bind 1).
            last: 1,
        }
    }

    /// Re-sube los parámetros físicos (p. ej. tras aleatorizar la matriz).
    /// Nota: el tamaño de celda del grid quedó fijado al `r_max` inicial.
    pub fn upload_params(&self, queue: &wgpu::Queue, params: &SimParams) {
        let gp = GpuParams::from(params, self.world, self.count, &self.grid);
        queue.write_buffer(&self.params_buf, 0, bytemuck::bytes_of(&gp));
        let cam = CamUniform {
            scale: [2.0 / self.world[0], -2.0 / self.world[1]],
            offset: [-1.0, 1.0],
            point_size: params.point_size * 1.6,
            _pad: [0.0; 3],
        };
        queue.write_buffer(&self.cam_buf, 0, bytemuck::bytes_of(&cam));
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

    /// Un paso de física + el render del resultado sobre `view`.
    pub fn frame(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        paused: bool,
    ) {
        if !paused {
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

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("particles"),
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
            pass.set_pipeline(&self.render_pipeline);
            pass.set_bind_group(0, &self.render_bind[self.last], &[]);
            pass.draw(0..4, 0..self.count);
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
