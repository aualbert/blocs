//! 3D view: draws a `Heightmap` as a closed block inside egui's render pass.

use eframe::{egui, egui_wgpu, wgpu};
use glam::Vec3;
use glam::camera::rh::{proj::directx::perspective, view::look_at_mat4};
use wgpu::util::DeviceExt;

use crate::relief::{CELL, Heightmap};

pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24Plus;

// Vertex kinds and wall sides, mirrored in terrain.wgsl.
const SURFACE: u8 = 0;
const WALL_TOP: u8 = 1;
const WALL_BOTTOM: u8 = 2;
const FLOOR: u8 = 3;

/// A grid point plus what part of the block it belongs to.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    grid: [u16; 2],
    kind_side: [u8; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    view_proj: [[f32; 4]; 4],
    params: [f32; 4],
    sun: [f32; 4],
    range: [f32; 4],
}

/// GPU objects for the current block, kept in egui's callback resources.
struct Gpu {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniforms: wgpu::Buffer,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
}

/// Orbit camera and display settings.
pub struct View3d {
    yaw: f32,
    pitch: f32,
    distance: f32,
    pub exaggeration: f32,
    pub base_depth: f32,
    size: f32,
    range: (f32, f32),
}

impl View3d {
    /// Uploads the block to the GPU, replacing the previous one.
    pub fn new(render_state: &egui_wgpu::RenderState, map: &Heightmap) -> Self {
        let gpu = upload(render_state, map);
        render_state.renderer.write().callback_resources.insert(gpu);
        let size = (map.size - 1) as f32 * CELL as f32;
        Self {
            yaw: -30f32.to_radians(),
            pitch: 30f32.to_radians(),
            distance: size * 2.6,
            exaggeration: 1.5,
            base_depth: size * 0.06,
            size,
            range: map.range(),
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), egui::Sense::drag());

        // Drag to orbit, scroll to zoom.
        let drag = response.drag_delta();
        self.yaw -= drag.x * 0.008;
        self.pitch = (self.pitch + drag.y * 0.008).clamp(-1.5, 1.5);
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            self.distance =
                (self.distance * (-scroll * 0.002).exp()).clamp(self.size * 0.3, self.size * 10.0);
        }

        let callback = Draw {
            uniforms: self.uniforms(rect.aspect_ratio()),
        };
        ui.painter()
            .add(egui_wgpu::Callback::new_paint_callback(rect, callback));
    }

    fn uniforms(&self, aspect: f32) -> Uniforms {
        let target = Vec3::new(
            0.0,
            (self.range.1 - self.range.0) * self.exaggeration * 0.3,
            0.0,
        );
        let eye = target
            + self.distance
                * Vec3::new(
                    self.yaw.sin() * self.pitch.cos(),
                    self.pitch.sin(),
                    self.yaw.cos() * self.pitch.cos(),
                );
        let view = look_at_mat4(eye, target, Vec3::Y);
        let proj = perspective(
            30f32.to_radians(),
            aspect,
            self.distance * 0.01,
            self.distance * 10.0,
        );
        // Fixed afternoon sun from the south-west for now.
        let sun = Vec3::new(-0.5, 0.6, 0.6).normalize();
        Uniforms {
            view_proj: (proj * view).to_cols_array_2d(),
            params: [
                self.exaggeration,
                self.base_depth,
                CELL as f32,
                (self.size / CELL as f32) + 1.0,
            ],
            sun: sun.extend(0.0).to_array(),
            range: [self.range.0, self.range.1, 0.0, 0.0],
        }
    }
}

struct Draw {
    uniforms: Uniforms,
}

impl egui_wgpu::CallbackTrait for Draw {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(gpu) = resources.get::<Gpu>() {
            queue.write_buffer(&gpu.uniforms, 0, bytemuck::bytes_of(&self.uniforms));
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(gpu) = resources.get::<Gpu>() else {
            return;
        };
        pass.set_pipeline(&gpu.pipeline);
        pass.set_bind_group(0, &gpu.bind_group, &[]);
        pass.set_vertex_buffer(0, gpu.vertices.slice(..));
        pass.set_index_buffer(gpu.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..gpu.index_count, 0, 0..1);
    }
}

/// Surface grid, then two vertices per edge point for the walls, then the 4 floor corners.
fn mesh(n: usize) -> (Vec<Vertex>, Vec<u32>) {
    let v = |i: usize, j: usize, kind: u8, side: u8| Vertex {
        grid: [i as u16, j as u16],
        kind_side: [kind, side, 0, 0],
    };
    let mut vertices = Vec::with_capacity(n * n + 8 * n + 4);
    let mut indices = Vec::with_capacity(6 * (n - 1) * (n - 1) + 24 * n + 6);
    let mut quad = |a: usize, b: usize, c: usize, d: usize| {
        indices.extend([a, b, c, b, d, c].map(|k| k as u32));
    };

    for j in 0..n {
        for i in 0..n {
            vertices.push(v(i, j, SURFACE, 0));
        }
    }
    for j in 0..n - 1 {
        for i in 0..n - 1 {
            let k = j * n + i;
            quad(k, k + 1, k + n, k + n + 1);
        }
    }

    // Walls: west, east, south, north.
    for side in 0..4u8 {
        let start = vertices.len();
        for t in 0..n {
            let (i, j) = match side {
                0 => (0, t),
                1 => (n - 1, t),
                2 => (t, 0),
                _ => (t, n - 1),
            };
            vertices.extend([v(i, j, WALL_TOP, side), v(i, j, WALL_BOTTOM, side)]);
        }
        for t in 0..n - 1 {
            let k = start + 2 * t;
            quad(k, k + 2, k + 1, k + 3);
        }
    }

    let start = vertices.len();
    vertices.extend([
        v(0, 0, FLOOR, 0),
        v(n - 1, 0, FLOOR, 0),
        v(0, n - 1, FLOOR, 0),
        v(n - 1, n - 1, FLOOR, 0),
    ]);
    quad(start, start + 1, start + 2, start + 3);
    (vertices, indices)
}

fn upload(rs: &egui_wgpu::RenderState, map: &Heightmap) -> Gpu {
    let device = &rs.device;
    let n = map.size as u32;

    let texture = device.create_texture_with_data(
        &rs.queue,
        &wgpu::TextureDescriptor {
            label: Some("heights"),
            size: wgpu::Extent3d {
                width: n,
                height: n,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        bytemuck::cast_slice(&map.heights),
    );
    let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("uniforms"),
        size: size_of::<Uniforms>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
        ],
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(
                    &texture.create_view(&Default::default()),
                ),
            },
        ],
    });

    let shader = device.create_shader_module(wgpu::include_wgsl!("terrain.wgsl"));
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("terrain"),
        layout: Some(
            &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None,
                bind_group_layouts: &[Some(&layout)],
                immediate_size: 0,
            }),
        ),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: size_of::<Vertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Uint16x2, 1 => Uint8x2],
            })],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(rs.target_format.into())],
        }),
        primitive: Default::default(), // no culling: walls are seen from both sides when orbiting
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    });

    let (vertices, indices) = mesh(map.size);
    let buffer = |label, contents: &[u8], usage| {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents,
            usage,
        })
    };
    Gpu {
        pipeline,
        bind_group,
        uniforms,
        vertices: buffer(
            "vertices",
            bytemuck::cast_slice(&vertices),
            wgpu::BufferUsages::VERTEX,
        ),
        indices: buffer(
            "indices",
            bytemuck::cast_slice(&indices),
            wgpu::BufferUsages::INDEX,
        ),
        index_count: indices.len() as u32,
    }
}
