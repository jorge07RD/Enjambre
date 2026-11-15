//! Blur real detrás de la lámina de cristal del panel.
//!
//! Cada frame: escena HDR → A (1/4 de ventana) → gauss H → B → gauss V → A
//! (dos iteraciones) y composición de A en la superficie, recortada con
//! scissor al rect de la lámina. Es barato (texturas a 1/16 de los píxeles) y
//! no toca la grabación: se encola después de copiar la escena al Recorder.

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

const FMT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Reducción de la textura de blur respecto a la ventana.
const DOWN: u32 = 4;
/// Separación de los taps del gauss (en texels de la textura reducida).
const SPREAD: f32 = 1.5;
/// Iteraciones H+V.
const ITER: usize = 2;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniform {
    texel: [f32; 2],
    dir: [f32; 2],
    spread: f32,
    _pad: [f32; 3],
}

pub struct Glass {
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    down: wgpu::RenderPipeline,
    blur: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
    u_down: wgpu::Buffer,
    u_h: wgpu::Buffer,
    u_v: wgpu::Buffer,
    a: wgpu::TextureView,
    b: wgpu::TextureView,
    bind_h: wgpu::BindGroup,
    bind_v: wgpu::BindGroup,
    bind_comp: wgpu::BindGroup,
    size: (u32, u32),
}

fn make_target(device: &wgpu::Device, w: u32, h: u32, label: &str) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FMT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}

impl Glass {
    /// `surface_format`: formato de la superficie de la ventana; `w`/`h`: su tamaño.
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat, w: u32, h: u32) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("glass"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/glass.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("glass layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("glass pipeline layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let mk = |label: &str, fs: &str, format: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_fullscreen"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview: None,
                cache: None,
            })
        };
        let down = mk("glass down", "fs_down", FMT);
        let blur = mk("glass blur", "fs_blur", FMT);
        let composite = mk("glass composite", "fs_composite", surface_format);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("glass sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let mk_u = |label: &str| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytemuck::bytes_of(&Uniform::zeroed()),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            })
        };
        let (u_down, u_h, u_v) = (mk_u("glass u down"), mk_u("glass u h"), mk_u("glass u v"));
        let (bw, bh) = Self::dims(w, h);
        let a = make_target(device, bw, bh, "glass A");
        let b = make_target(device, bw, bh, "glass B");
        let bind = |view: &wgpu::TextureView, u: &wgpu::Buffer| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("glass bind"),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: u.as_entire_binding(),
                    },
                ],
            })
        };
        let bind_h = bind(&a, &u_h);
        let bind_v = bind(&b, &u_v);
        let bind_comp = bind(&a, &u_down);
        Self {
            layout,
            sampler,
            down,
            blur,
            composite,
            u_down,
            u_h,
            u_v,
            a,
            b,
            bind_h,
            bind_v,
            bind_comp,
            size: (0, 0),
        }
    }

    fn dims(w: u32, h: u32) -> (u32, u32) {
        ((w / DOWN).max(1), (h / DOWN).max(1))
    }

    fn bind(
        &self,
        device: &wgpu::Device,
        view: &wgpu::TextureView,
        u: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("glass bind"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: u.as_entire_binding(),
                },
            ],
        })
    }

    /// Ajusta las texturas al tamaño de la ventana (si cambió).
    fn ensure_size(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, w: u32, h: u32) {
        if self.size == (w, h) {
            return;
        }
        self.size = (w, h);
        let (bw, bh) = Self::dims(w, h);
        self.a = make_target(device, bw, bh, "glass A");
        self.b = make_target(device, bw, bh, "glass B");
        self.bind_h = self.bind(device, &self.a, &self.u_h);
        self.bind_v = self.bind(device, &self.b, &self.u_v);
        self.bind_comp = self.bind(device, &self.a, &self.u_down);
        let texel = [1.0 / bw as f32, 1.0 / bh as f32];
        let u = |dir: [f32; 2]| Uniform {
            texel,
            dir,
            spread: SPREAD,
            _pad: [0.0; 3],
        };
        queue.write_buffer(&self.u_down, 0, bytemuck::bytes_of(&u([0.0, 0.0])));
        queue.write_buffer(&self.u_h, 0, bytemuck::bytes_of(&u([1.0, 0.0])));
        queue.write_buffer(&self.u_v, 0, bytemuck::bytes_of(&u([0.0, 1.0])));
    }

    fn pass(
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        pipeline: &wgpu::RenderPipeline,
        bind: &wgpu::BindGroup,
        scissor: Option<[u32; 4]>,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("glass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: if scissor.is_some() {
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
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, bind, &[]);
        if let Some([x, y, w, h]) = scissor {
            pass.set_scissor_rect(x, y, w, h);
        }
        pass.draw(0..3, 0..1);
    }

    /// Encola el desenfoque de `scene` y su composición en `target` (tamaño
    /// `size` en píxeles) dentro de `rect_px` = [x, y, w, h].
    #[allow(clippy::too_many_arguments)]
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        scene: &wgpu::TextureView,
        target: &wgpu::TextureView,
        size: (u32, u32),
        rect_px: [f32; 4],
    ) {
        let (sw, sh) = size;
        if sw == 0 || sh == 0 {
            return;
        }
        // Recorte al tamaño de la superficie.
        let x0 = rect_px[0].floor().clamp(0.0, sw as f32) as u32;
        let y0 = rect_px[1].floor().clamp(0.0, sh as f32) as u32;
        let x1 = (rect_px[0] + rect_px[2]).ceil().clamp(0.0, sw as f32) as u32;
        let y1 = (rect_px[1] + rect_px[3]).ceil().clamp(0.0, sh as f32) as u32;
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        self.ensure_size(device, queue, sw, sh);
        // La escena puede recrearse (resize, grabación a otra resolución):
        // su bind group se construye en cada frame (es barato).
        let bind_down = self.bind(device, scene, &self.u_down);
        Self::pass(encoder, &self.a, &self.down, &bind_down, None);
        for _ in 0..ITER {
            Self::pass(encoder, &self.b, &self.blur, &self.bind_h, None);
            Self::pass(encoder, &self.a, &self.blur, &self.bind_v, None);
        }
        Self::pass(
            encoder,
            target,
            &self.composite,
            &self.bind_comp,
            Some([x0, y0, x1 - x0, y1 - y0]),
        );
    }
}
