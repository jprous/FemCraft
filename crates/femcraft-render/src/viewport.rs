//! GPU line renderer drawn inside the egui viewport (grid, axes, later members).

use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui;
use eframe::egui_wgpu::{self, wgpu};
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

/// Anti-aliasing samples. Must match `NativeOptions::multisampling`.
pub const MSAA_SAMPLES: u32 = 4;

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LineVertex {
    pub pos: [f32; 3],
    pub color: [f32; 4],
}

impl LineVertex {
    pub fn new(pos: Vec3, color: [f32; 4]) -> Self {
        Self {
            pos: pos.to_array(),
            color,
        }
    }
}

const VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 2] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4];

/// GPU objects kept alive by egui between frames: shared pipelines plus one set of buffers per
/// model view, so several views can be drawn in the same frame.
struct LineRenderer {
    pipeline: wgpu::RenderPipeline,
    tri_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    views: HashMap<u64, ViewBuffers>,
}

/// Buffers of one model view.
struct ViewBuffers {
    uniform_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    vertex_buffer: Option<wgpu::Buffer>,
    vertex_count: u32,
    tri_buffer: Option<wgpu::Buffer>,
    tri_count: u32,
    geometry_version: Option<u64>,
}

impl ViewBuffers {
    fn new(device: &wgpu::Device, layout: &wgpu::BindGroupLayout) -> Self {
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("femcraft view_proj"),
            size: std::mem::size_of::<[f32; 16]>() as u64,
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
            geometry_version: None,
        }
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

/// Create the line pipeline and hand it to egui. Call once at startup.
pub fn install(render_state: &egui_wgpu::RenderState) {
    let device = &render_state.device;

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("femcraft lines shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("shaders/lines.wgsl").into()),
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

    let make_pipeline = |label: &str, topology: wgpu::PrimitiveTopology| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<LineVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &VERTEX_ATTRIBUTES,
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: render_state.target_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState {
                count: MSAA_SAMPLES,
                ..Default::default()
            },
            multiview_mask: None,
            cache: None,
        })
    };
    let pipeline = make_pipeline("femcraft lines pipeline", wgpu::PrimitiveTopology::LineList);
    // Translucent fills (surfaces); no depth buffer yet, so drawn before the lines.
    let tri_pipeline = make_pipeline(
        "femcraft triangles pipeline",
        wgpu::PrimitiveTopology::TriangleList,
    );

    render_state
        .renderer
        .write()
        .callback_resources
        .insert(LineRenderer {
            pipeline,
            tri_pipeline,
            bind_group_layout,
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
    /// Triangle list (triples of vertices), drawn first.
    pub triangles: Arc<Vec<LineVertex>>,
    /// Bumped whenever `vertices` or `triangles` change, so GPU buffers are only rebuilt then.
    pub version: u64,
}

impl egui_wgpu::CallbackTrait for LinesCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen_descriptor: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
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
            r.geometry_version = Some(self.version);
        }

        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(renderer) = resources.get::<LineRenderer>() else {
            return;
        };
        let Some(r) = renderer.views.get(&self.view) else {
            return;
        };
        render_pass.set_bind_group(0, &r.bind_group, &[]);
        if let Some(tb) = &r.tri_buffer {
            render_pass.set_pipeline(&renderer.tri_pipeline);
            render_pass.set_vertex_buffer(0, tb.slice(..));
            render_pass.draw(0..r.tri_count, 0..1);
        }
        if let Some(vb) = &r.vertex_buffer {
            render_pass.set_pipeline(&renderer.pipeline);
            render_pass.set_vertex_buffer(0, vb.slice(..));
            render_pass.draw(0..r.vertex_count, 0..1);
        }
    }
}
