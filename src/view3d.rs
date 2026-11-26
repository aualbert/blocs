//! 3D view: draws a `Heightmap` as a closed block inside egui's render pass.
//!
//! Parallel (axonometric) projection only, as in hand-drawn block diagrams: no vanishing
//! point, one scale across the whole block.

use std::f32::consts::{FRAC_PI_2, PI};

use eframe::{egui, egui_wgpu, wgpu};
use glam::Vec3;
use glam::camera::rh::{proj::directx::orthographic, view::look_at_mat4};
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
    grid: [f32; 4],
    sun: [f32; 4],
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

/// Axonometric camera orbiting the block, and display settings.
pub struct View3d {
    /// Azimuth of the viewer: 0 looks from the south, π/2 from the east.
    yaw: f32,
    /// Elevation of the viewer: π/2 looks straight down.
    pitch: f32,
    /// Half the visible height (m).
    zoom: f32,
    /// Point looked at, relative to the block centre (m).
    target: Vec3,
    pub exaggeration: f32,
    pub base_depth: f32,
    /// Block width (east) and depth (north) in metres, and the grid points per side.
    extent: (f32, f32),
    points: (u32, u32),
    range: (f32, f32),
    /// Last on-screen aspect ratio, reused by the export.
    aspect: f32,
}

/// Gizmo axes: label, world direction (x east, y up, z south), and the view it selects.
const AXES: [(&str, [f32; 3], f32, f32); 6] = [
    ("E", [1.0, 0.0, 0.0], FRAC_PI_2, 0.0),
    ("O", [-1.0, 0.0, 0.0], -FRAC_PI_2, 0.0),
    ("N", [0.0, 0.0, -1.0], PI, 0.0),
    ("S", [0.0, 0.0, 1.0], 0.0, 0.0),
    ("H", [0.0, 1.0, 0.0], 0.0, FRAC_PI_2),
    ("B", [0.0, -1.0, 0.0], 0.0, -FRAC_PI_2),
];

impl View3d {
    /// Uploads the block to the GPU, replacing the previous one.
    pub fn new(render_state: &egui_wgpu::RenderState, map: &Heightmap) -> Self {
        let gpu = upload(render_state, map);
        render_state.renderer.write().callback_resources.insert(gpu);
        let cell = CELL as f32;
        let extent = ((map.nx - 1) as f32 * cell, (map.ny - 1) as f32 * cell);
        Self {
            yaw: -30f32.to_radians(),
            pitch: 30f32.to_radians(),
            zoom: extent.0.max(extent.1) * 0.55,
            target: Vec3::ZERO,
            exaggeration: 1.5,
            base_depth: extent.0.max(extent.1) * 0.06,
            extent,
            points: (map.nx as u32, map.ny as u32),
            range: map.range(),
            aspect: 1.5,
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let (rect, response) =
            ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
        self.aspect = rect.aspect_ratio();
        let (_, right, up) = self.basis();

        // Left drag orbits, right or middle drag pans. The wheel zooms;
        // with Shift it pans sideways, with Ctrl up and down.
        let m_per_px = 2.0 * self.zoom / rect.height();
        let drag = response.drag_delta();
        if response.dragged_by(egui::PointerButton::Primary) {
            self.yaw -= drag.x * 0.008;
            self.pitch = (self.pitch + drag.y * 0.008).clamp(-FRAC_PI_2, FRAC_PI_2);
        } else if response.dragged() {
            self.target += (-drag.x * right + drag.y * up) * m_per_px;
        }
        if response.hovered() {
            let (scroll, modifiers) = ui.input(|i| (i.smooth_scroll_delta, i.modifiers));
            if modifiers.shift || modifiers.ctrl {
                self.target += (-scroll.x * right + scroll.y * up) * m_per_px;
            } else {
                let size = self.extent.0.max(self.extent.1);
                self.zoom = (self.zoom * (-scroll.y * 0.002).exp()).clamp(size * 0.05, size * 3.0);
            }
        }

        let callback = Draw {
            uniforms: self.uniforms(self.aspect),
        };
        ui.painter()
            .add(egui_wgpu::Callback::new_paint_callback(rect, callback));
        self.gizmo(ui, rect);
    }

    /// Blender-style axis widget in the top-right corner; clicking an axis aligns the view.
    fn gizmo(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        let (dir, right, up) = self.basis();
        let radius = 40.0;
        let center = rect.right_top() + egui::vec2(-radius - 16.0, radius + 16.0);
        let area = egui::Rect::from_center_size(center, egui::Vec2::splat(2.0 * radius + 24.0));
        let response = ui.interact(area, ui.scope_id().with("gizmo"), egui::Sense::click());
        let painter = ui.painter_at(area);
        painter.circle_filled(center, radius + 12.0, egui::Color32::from_black_alpha(40));

        // Draw back to front so the nearest axis is on top and gets the click.
        let mut axes: Vec<_> = AXES
            .iter()
            .map(|axis| {
                let a = Vec3::from(axis.1);
                let pos = center + radius * egui::vec2(a.dot(right), -a.dot(up));
                (a.dot(dir), pos, axis)
            })
            .collect();
        axes.sort_by(|a, b| a.0.total_cmp(&b.0));

        let mut clicked = None;
        for &(_, pos, axis) in &axes {
            let color = match axis.0 {
                "E" | "O" => egui::Color32::from_rgb(220, 80, 80),
                "N" | "S" => egui::Color32::from_rgb(90, 170, 90),
                _ => egui::Color32::from_rgb(80, 130, 230),
            };
            painter.line_segment([center, pos], egui::Stroke::new(2.0, color));
            painter.circle_filled(pos, 9.0, color);
            painter.text(
                pos,
                egui::Align2::CENTER_CENTER,
                axis.0,
                egui::FontId::proportional(11.0),
                egui::Color32::WHITE,
            );
            if response.clicked()
                && let Some(p) = response.interact_pointer_pos()
                && p.distance(pos) < 10.0
            {
                clicked = Some(axis);
            }
        }
        if let Some(&(_, _, yaw, pitch)) = clicked {
            (self.yaw, self.pitch) = (yaw, pitch);
        }
    }

    /// Unit vectors towards the viewer, to the right and up on screen.
    fn basis(&self) -> (Vec3, Vec3, Vec3) {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        let dir = Vec3::new(sy * cp, sp, cy * cp);
        // Derivative of `dir` along the pitch: stays valid when looking straight down.
        let up = Vec3::new(-sy * sp, cp, -cy * sp);
        (dir, up.cross(dir), up)
    }

    fn uniforms(&self, aspect: f32) -> Uniforms {
        let (dir, _, up) = self.basis();
        // Look at the middle of the relief; the camera stands outside the whole block.
        let relief = (self.range.1 - self.range.0) * self.exaggeration;
        let target = self.target + Vec3::Y * relief * 0.3;
        let radius = Vec3::new(self.extent.0, relief + self.base_depth, self.extent.1).length();
        let view = look_at_mat4(target + dir * radius * 2.0, target, up);
        let (h, w) = (self.zoom, self.zoom * aspect);
        let proj = orthographic(-w, w, -h, h, 0.0, radius * 4.0);
        // Fixed afternoon sun from the south-west for now.
        let sun = Vec3::new(-0.5, 0.6, 0.6).normalize();
        Uniforms {
            view_proj: (proj * view).to_cols_array_2d(),
            params: [self.exaggeration, self.base_depth, CELL as f32, 0.0],
            grid: [
                self.points.0 as f32,
                self.points.1 as f32,
                self.range.0,
                self.range.1,
            ],
            sun: sun.extend(0.0).to_array(),
        }
    }

    /// Renders the current view off-screen at `width` px, on a transparent background.
    /// Returns `(width, height, rgba)`.
    pub fn render_image(&self, rs: &egui_wgpu::RenderState, width: u32) -> (u32, u32, Vec<u8>) {
        let height = (width as f32 / self.aspect).round() as u32;
        let (device, queue) = (&rs.device, &rs.queue);
        let extent = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let texture = |format, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("export"),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        // Same formats as the screen, so the pipeline is reused and colours match.
        let color = texture(
            rs.target_format,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let depth = texture(DEPTH_FORMAT, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let row = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("export"),
            size: (row * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let renderer = rs.renderer.read();
        let gpu: &Gpu = renderer.callback_resources.get().expect("bloc chargé");
        queue.write_buffer(
            &gpu.uniforms,
            0,
            bytemuck::bytes_of(&self.uniforms(self.aspect)),
        );
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let color_view = color.create_view(&Default::default());
            let depth_view = depth.create_view(&Default::default());
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("export"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            gpu.draw(&mut pass);
        }
        encoder.copy_texture_to_buffer(
            color.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: None,
                },
            },
            extent,
        );
        queue.submit([encoder.finish()]);

        let slice = readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("rendu terminé");
        let bgra = matches!(
            rs.target_format,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
        );
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for line in slice
            .get_mapped_range()
            .expect("tampon lu")
            .chunks(row as usize)
        {
            rgba.extend_from_slice(&line[..(width * 4) as usize]);
        }
        if bgra {
            rgba.chunks_mut(4).for_each(|px| px.swap(0, 2));
        }
        (width, height, rgba)
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
        if let Some(gpu) = resources.get::<Gpu>() {
            gpu.draw(pass);
        }
    }
}

impl Gpu {
    fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..self.index_count, 0, 0..1);
    }
}

/// Surface grid, then two vertices per edge point for the walls, then the 4 floor corners.
fn mesh(nx: usize, ny: usize) -> (Vec<Vertex>, Vec<u32>) {
    let v = |i: usize, j: usize, kind: u8, side: u8| Vertex {
        grid: [i as u16, j as u16],
        kind_side: [kind, side, 0, 0],
    };
    let mut vertices = Vec::with_capacity(nx * ny + 4 * (nx + ny) + 4);
    let mut indices = Vec::with_capacity(6 * (nx - 1) * (ny - 1) + 12 * (nx + ny) + 6);
    let mut quad = |a: usize, b: usize, c: usize, d: usize| {
        indices.extend([a, b, c, b, d, c].map(|k| k as u32));
    };

    for j in 0..ny {
        for i in 0..nx {
            vertices.push(v(i, j, SURFACE, 0));
        }
    }
    for j in 0..ny - 1 {
        for i in 0..nx - 1 {
            let k = j * nx + i;
            quad(k, k + 1, k + nx, k + nx + 1);
        }
    }

    // Walls: west, east, south, north.
    for side in 0..4u8 {
        let start = vertices.len();
        let n = if side < 2 { ny } else { nx };
        for t in 0..n {
            let (i, j) = match side {
                0 => (0, t),
                1 => (nx - 1, t),
                2 => (t, 0),
                _ => (t, ny - 1),
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
        v(nx - 1, 0, FLOOR, 0),
        v(0, ny - 1, FLOOR, 0),
        v(nx - 1, ny - 1, FLOOR, 0),
    ]);
    quad(start, start + 1, start + 2, start + 3);
    (vertices, indices)
}

fn upload(rs: &egui_wgpu::RenderState, map: &Heightmap) -> Gpu {
    let device = &rs.device;

    let texture = device.create_texture_with_data(
        &rs.queue,
        &wgpu::TextureDescriptor {
            label: Some("heights"),
            size: wgpu::Extent3d {
                width: map.nx as u32,
                height: map.ny as u32,
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

    let (vertices, indices) = mesh(map.nx, map.ny);
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
