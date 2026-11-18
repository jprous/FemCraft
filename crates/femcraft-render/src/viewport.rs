//! GPU renderer drawn inside the egui viewport (grid, axes, members, surfaces, result contours).
//!
//! egui's own render pass has no depth buffer, so the 3D scene is rendered to an offscreen MSAA
//! texture with a depth attachment, then composited back into egui's pass. The depth buffer lets
//! the opaque result contour occlude itself correctly from any angle; the translucent model fills
//! and the lines keep their previous painter-order behaviour (drawn without depth writes).

use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui;
use eframe::egui_wgpu::{self, wgpu};
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

/// Anti-aliasing samples. Must match `NativeOptions::multisampling`.
pub const MSAA_SAMPLES: u32 = 4;
/// Depth buffer format of the offscreen pass.
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// Offscreen render scale over the viewport (supersampling factor), downsampled at blit time.
const SUPERSAMPLE: u32 = 1;
/// Width of drawn lines in physical pixels.
const LINE_WIDTH_PX: f32 = 1.0;

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LineVertex {
    pub pos: [f32; 3],
    pub color: [f32; 4],
    /// Line width multiplier (lines only; 1.0 = the standard width).
    pub width: f32,
}

impl LineVertex {
    pub fn new(pos: Vec3, color: [f32; 4]) -> Self {
        Self {
            pos: pos.to_array(),
            color,
            width: 1.0,
        }
    }

    pub fn with_width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }
}

const VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 3] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4, 2 => Float32];
/// Line segments as instances: both end vertices of a segment (two consecutive `LineVertex`).
const LINE_ATTRIBUTES: [wgpu::VertexAttribute; 6] = wgpu::vertex_attr_array![
    0 => Float32x3, 1 => Float32x4, 2 => Float32, 3 => Float32x3, 4 => Float32x4, 5 => Float32
];
const LINE_STRIDE: u64 = 2 * std::mem::size_of::<LineVertex>() as u64;

/// GPU objects kept alive by egui between frames: shared pipelines plus one set of buffers per
/// model view, so several views can be drawn in the same frame.
struct LineRenderer {
    /// Lines (grid, axes, model edges); drawn on top, no depth interaction.
    line_pipeline: wgpu::RenderPipeline,
    /// Translucent model surface fills; depth-tested but no depth write.
    tri_pipeline: wgpu::RenderPipeline,
    /// Opaque result contour; depth write + test, so it occludes itself correctly.
    result_pipeline: wgpu::RenderPipeline,
    /// Composites the offscreen texture into egui's pass.
    blit_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    blit_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// egui's target colour format, used for the offscreen targets.
    format: wgpu::TextureFormat,
    views: HashMap<u64, ViewBuffers>,
}

/// Buffers and offscreen targets of one model view.
struct ViewBuffers {
    uniform_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    vertex_buffer: Option<wgpu::Buffer>,
    vertex_count: u32,
    tri_buffer: Option<wgpu::Buffer>,
    tri_count: u32,
    result_buffer: Option<wgpu::Buffer>,
    result_count: u32,
    grid_buffer: Option<wgpu::Buffer>,
    grid_count: u32,
    geometry_version: Option<u64>,
    /// Offscreen target size in physical pixels.
    size: [u32; 2],
    /// Supersampled colour target (sampled for the blit) and its depth target.
    msaa_view: Option<wgpu::TextureView>,
    color_view: Option<wgpu::TextureView>,
    depth_view: Option<wgpu::TextureView>,
    blit_bind_group: Option<wgpu::BindGroup>,
}

impl ViewBuffers {
    fn new(device: &wgpu::Device, layout: &wgpu::BindGroupLayout) -> Self {
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("femcraft view_proj"),
            size: std::mem::size_of::<[f32; 20]>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("femcraft lines bind group"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });
        Self {
            uniform_buffer,
            bind_group,
            vertex_buffer: None,
            vertex_count: 0,
            tri_buffer: None,
            tri_count: 0,
            result_buffer: None,
            result_count: 0,
            grid_buffer: None,
            grid_count: 0,
            geometry_version: None,
            size: [0, 0],
            msaa_view: None,
            color_view: None,
            depth_view: None,
            blit_bind_group: None,
        }
    }

    /// (Re)creates the offscreen MSAA colour + depth targets and the resolved colour the blit
    /// samples, sized to `size` (physical pixels).
    fn ensure_targets(
        &mut self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        blit_layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        size: [u32; 2],
    ) {
        if self.color_view.is_some() && self.size == size {
            return;
        }
        self.size = size;
        // Supersample: render larger than the viewport, then downsample with a linear filter at
        // blit time. Together with MSAA this smooths the many internal band-boundary edges of a
        // result contour. Clamped so a 4K viewport stays within the max texture size.
        let extent = wgpu::Extent3d {
            width: (size[0] * SUPERSAMPLE).clamp(1, 8192),
            height: (size[1] * SUPERSAMPLE).clamp(1, 8192),
            depth_or_array_layers: 1,
        };
        let make = |label, fmt, samples, usage| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: extent,
                    mip_level_count: 1,
                    sample_count: samples,
                    dimension: wgpu::TextureDimension::D2,
                    format: fmt,
                    usage,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        let msaa = make(
            "femcraft offscreen msaa colour",
            format,
            MSAA_SAMPLES,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        let color = make(
            "femcraft offscreen colour",
            format,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let depth = make(
            "femcraft offscreen depth",
            DEPTH_FORMAT,
            MSAA_SAMPLES,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        self.blit_bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("femcraft blit bind group"),
            layout: blit_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&color),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        }));
        self.msaa_view = Some(msaa);
        self.color_view = Some(color);
        self.depth_view = Some(depth);
    }
}

/// Frees the GPU buffers of a view that is gone (for example a closed model).
pub fn forget_view(render_state: &egui_wgpu::RenderState, view: u64) {
    if let Some(r) = render_state
        .renderer
        .write()
        .callback_resources
        .get_mut::<LineRenderer>()
    {
        r.views.remove(&view);
    }
}

/// Create the pipelines and hand them to egui. Call once at startup.
pub fn install(render_state: &egui_wgpu::RenderState) {
    let device = &render_state.device;
    let format = render_state.target_format;

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("femcraft lines shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("shaders/lines.wgsl").into()),
    });
    let blit_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("femcraft blit shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("shaders/blit.wgsl").into()),
    });

    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("femcraft lines bind group layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });

    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("femcraft lines layout"),
        bind_group_layouts: &[Some(&bind_group_layout)],
        immediate_size: 0,
    });

    // The offscreen pass has a depth attachment, so every scene pipeline declares a depth state.
    let depth_state = |write: bool, compare: wgpu::CompareFunction| {
        Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(write),
            depth_compare: Some(compare),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        })
    };

    let make_scene =
        |label: &str, topology: wgpu::PrimitiveTopology, depth: Option<wgpu::DepthStencilState>| {
            // `LineList` means line segments, drawn as screen-space quads (one instance each).
            let is_line = topology == wgpu::PrimitiveTopology::LineList;
            let topology = wgpu::PrimitiveTopology::TriangleList;
            let buffer = if is_line {
                wgpu::VertexBufferLayout {
                    array_stride: LINE_STRIDE,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &LINE_ATTRIBUTES,
                }
            } else {
                wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<LineVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &VERTEX_ATTRIBUTES,
                }
            };
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(if is_line { "vs_line" } else { "vs_main" }),
                    compilation_options: Default::default(),
                    buffers: &[Some(buffer)],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: depth,
                // Offscreen targets are MSAA and resolved into a single-sample texture for the blit.
                multisample: wgpu::MultisampleState {
                    count: MSAA_SAMPLES,
                    ..Default::default()
                },
                multiview_mask: None,
                cache: None,
            })
        };

    // Lines and translucent fills do not write depth (painter order, drawn on top of the contour);
    // the opaque result contour writes and tests depth so it occludes itself from any angle.
    let line_pipeline = make_scene(
        "femcraft lines pipeline",
        wgpu::PrimitiveTopology::LineList,
        depth_state(false, wgpu::CompareFunction::Always),
    );
    let tri_pipeline = make_scene(
        "femcraft triangles pipeline",
        wgpu::PrimitiveTopology::TriangleList,
        depth_state(false, wgpu::CompareFunction::Always),
    );
    let result_pipeline = make_scene(
        "femcraft result pipeline",
        wgpu::PrimitiveTopology::TriangleList,
        depth_state(true, wgpu::CompareFunction::Less),
    );

    // Blit: samples the resolved offscreen colour into egui's (MSAA) pass; no depth.
    let blit_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("femcraft blit bind group layout"),
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
        ],
    });
    let blit_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("femcraft blit layout"),
        bind_group_layouts: &[Some(&blit_layout)],
        immediate_size: 0,
    });
    let blit_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("femcraft blit pipeline"),
        layout: Some(&blit_pipeline_layout),
        vertex: wgpu::VertexState {
            module: &blit_shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &blit_shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                // The offscreen texture holds premultiplied colour (blended over transparent black).
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState {
            count: MSAA_SAMPLES,
            ..Default::default()
        },
        multiview_mask: None,
        cache: None,
    });

    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("femcraft blit sampler"),
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        ..Default::default()
    });

    render_state
        .renderer
        .write()
        .callback_resources
        .insert(LineRenderer {
            line_pipeline,
            tri_pipeline,
            result_pipeline,
            blit_pipeline,
            bind_group_layout,
            blit_layout,
            sampler,
            format,
            views: HashMap::new(),
        });
}

/// One frame's draw request for a model view.
pub struct LinesCallback {
    /// Identifies the view (each open model has its own GPU buffers).
    pub view: u64,
    pub view_proj: Mat4,
    /// Line list (pairs of vertices).
    pub vertices: Arc<Vec<LineVertex>>,
    /// Translucent model surface fills (triangle list).
    pub triangles: Arc<Vec<LineVertex>>,
    /// Opaque result contour (triangle list), depth-tested.
    pub result_triangles: Arc<Vec<LineVertex>>,
    /// Grid lines, drawn first so opaque results hide them.
    pub grid_lines: Arc<Vec<LineVertex>>,
    /// Bumped whenever any geometry changes, so GPU buffers are only rebuilt then.
    pub version: u64,
    /// Viewport size in physical pixels, for the offscreen targets.
    pub pixels: [u32; 2],
}

impl egui_wgpu::CallbackTrait for LinesCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen_descriptor: &egui_wgpu::ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(renderer) = resources.get_mut::<LineRenderer>() else {
            return Vec::new();
        };
        let layout = &renderer.bind_group_layout;
        let r = renderer
            .views
            .entry(self.view)
            .or_insert_with(|| ViewBuffers::new(device, layout));

        queue.write_buffer(
            &r.uniform_buffer,
            0,
            bytemuck::cast_slice(&self.view_proj.to_cols_array()),
        );
        queue.write_buffer(
            &r.uniform_buffer,
            64,
            bytemuck::cast_slice(&[
                (self.pixels[0] * SUPERSAMPLE).max(1) as f32,
                (self.pixels[1] * SUPERSAMPLE).max(1) as f32,
                LINE_WIDTH_PX * SUPERSAMPLE as f32,
                0.0,
            ]),
        );

        if r.geometry_version != Some(self.version) {
            let upload = |label: &str, v: &[LineVertex]| {
                (!v.is_empty()).then(|| {
                    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some(label),
                        contents: bytemuck::cast_slice(v),
                        usage: wgpu::BufferUsages::VERTEX,
                    })
                })
            };
            r.vertex_buffer = upload("femcraft lines vertices", &self.vertices);
            r.vertex_count = self.vertices.len() as u32;
            r.tri_buffer = upload("femcraft triangle vertices", &self.triangles);
            r.tri_count = self.triangles.len() as u32;
            r.result_buffer = upload("femcraft result vertices", &self.result_triangles);
            r.result_count = self.result_triangles.len() as u32;
            r.grid_buffer = upload("femcraft grid vertices", &self.grid_lines);
            r.grid_count = self.grid_lines.len() as u32;
            r.geometry_version = Some(self.version);
        }

        r.ensure_targets(
            device,
            renderer.format,
            &renderer.blit_layout,
            &renderer.sampler,
            self.pixels,
        );

        // Render the scene into the supersampled offscreen colour + depth.
        let (Some(msaa), Some(color), Some(depth)) =
            (&r.msaa_view, &r.color_view, &r.depth_view) else {
            return Vec::new();
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("femcraft offscreen pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: msaa,
                depth_slice: None,
                resolve_target: Some(color),
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Discard,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_bind_group(0, &r.bind_group, &[]);
        // Translucent fills first (no depth write), then the depth-tested opaque contour, then
        // lines on top.
        if let Some(gb) = &r.grid_buffer {
            pass.set_pipeline(&renderer.line_pipeline);
            pass.set_vertex_buffer(0, gb.slice(..));
            pass.draw(0..6, 0..r.grid_count / 2);
        }
        if let Some(tb) = &r.tri_buffer {
            pass.set_pipeline(&renderer.tri_pipeline);
            pass.set_vertex_buffer(0, tb.slice(..));
            pass.draw(0..r.tri_count, 0..1);
        }
        if let Some(rb) = &r.result_buffer {
            pass.set_pipeline(&renderer.result_pipeline);
            pass.set_vertex_buffer(0, rb.slice(..));
            pass.draw(0..r.result_count, 0..1);
        }
        if let Some(vb) = &r.vertex_buffer {
            pass.set_pipeline(&renderer.line_pipeline);
            pass.set_vertex_buffer(0, vb.slice(..));
            pass.draw(0..6, 0..r.vertex_count / 2);
        }
        drop(pass);

        Vec::new()
    }

    fn paint(
        &self,
        info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(renderer) = resources.get::<LineRenderer>() else {
            return;
        };
        let Some(r) = renderer.views.get(&self.view) else {
            return;
        };
        let Some(blit) = &r.blit_bind_group else {
            return;
        };
        // Composite the offscreen texture into this view's rectangle within egui's pass.
        let vp = info.viewport_in_pixels();
        let (x, y, w, h) = (
            vp.left_px as f32,
            vp.top_px as f32,
            vp.width_px as f32,
            vp.height_px as f32,
        );
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        render_pass.set_viewport(x, y, w, h, 0.0, 1.0);
        render_pass.set_pipeline(&renderer.blit_pipeline);
        render_pass.set_bind_group(0, blit, &[]);
        render_pass.draw(0..3, 0..1);
    }
}
