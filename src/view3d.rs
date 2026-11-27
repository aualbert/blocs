//! 3D view: draws a `Heightmap` as a closed block inside egui's render pass.
//!
//! Parallel (axonometric) projection only, as in hand-drawn block diagrams: no vanishing
//! point, one scale across the whole block.

use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_2, PI};
use std::sync::Arc;

use eframe::{egui, egui_wgpu, wgpu};
use glam::camera::rh::{proj::directx::orthographic, view::look_at_mat4};
use glam::{Mat4, Quat, Vec3};
use image::RgbaImage;
use wgpu::util::DeviceExt;

use crate::buildings;
use crate::relief::{CELL, Heightmap};
use crate::shadow;
use crate::sun::Sun;

pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24Plus;
/// The block is drawn into its own image, which egui then shows: egui expects colours
/// as displayed (not sRGB-aware), and its own pass has no depth buffer.
const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// Layer images stacked on the surface, mirrored in terrain.wgsl.
pub const SLOTS: usize = 9;
/// Depths offered for the base, in metres.
pub const BASE_DEPTHS: std::ops::RangeInclusive<f32> = 10.0..=1000.0;
/// MSAA samples per pixel: smooths the block's outline and wall edges at little cost.
const SAMPLES: u32 = 4;

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
    /// Opacity of each layer slot, bottom first; 0 for empty slots.
    opacity: [[f32; 4]; 3],
}

/// GPU objects for the current block, kept in egui's callback resources.
struct Gpu {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    bind_group: wgpu::BindGroup,
    uniforms: wgpu::Buffer,
    heights: wgpu::TextureView,
    /// Where the sun reaches the surface, see shadow.rs.
    shadows: wgpu::Texture,
    sampler: wgpu::Sampler,
    /// Transparent 1 × 1 image for empty slots.
    blank: wgpu::TextureView,
    /// Draped layer images by layer id.
    layers: HashMap<String, wgpu::TextureView>,
    /// Layer ids in the slots, bottom first.
    slots: Vec<String>,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
    /// Buildings pipeline, and their vertices, indices and index count once known.
    building_pipeline: wgpu::RenderPipeline,
    buildings: Option<(wgpu::Buffer, wgpu::Buffer, u32)>,
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
    /// Opacity of each layer slot, bottom first.
    opacity: [f32; SLOTS],
    sun: Sun,
    /// The block's turn on the map, anticlockwise: the sun and the axes stay on the compass.
    angle: f32,
    /// The block's heights, and the sun its shadows were cast for: on the true relief,
    /// so that stretching it leaves them alone.
    heights: Vec<f32>,
    shadows_for: Option<Vec3>,
    /// The on-screen image and its egui id, remade when the view changes size.
    screen: Option<(Arc<Target>, egui::TextureId)>,
}

/// An image the block is drawn into, with its multisampled colour and depth buffers.
struct Target {
    texture: wgpu::Texture,
    color: wgpu::TextureView,
    msaa: wgpu::TextureView,
    depth: wgpu::TextureView,
}

impl Target {
    fn new(device: &wgpu::Device, [width, height]: [u32; 2], usage: wgpu::TextureUsages) -> Self {
        let texture = |format, usage, sample_count| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("bloc"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let attachment = wgpu::TextureUsages::RENDER_ATTACHMENT;
        let color = texture(COLOR_FORMAT, attachment | usage, 1);
        let msaa = texture(COLOR_FORMAT, attachment, SAMPLES);
        let depth = texture(DEPTH_FORMAT, attachment, SAMPLES);
        Self {
            color: color.create_view(&Default::default()),
            msaa: msaa.create_view(&Default::default()),
            depth: depth.create_view(&Default::default()),
            texture: color,
        }
    }

    fn size(&self) -> [u32; 2] {
        [self.texture.width(), self.texture.height()]
    }
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
    /// Uploads the block to the GPU, replacing the previous one; `latitude` in degrees,
    /// `angle` the frame's.
    pub fn new(
        render_state: &egui_wgpu::RenderState,
        map: &Heightmap,
        latitude: f64,
        angle: f64,
    ) -> Self {
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
            // 6 % of the block's width, within the depths offered.
            base_depth: (extent.0.max(extent.1) * 0.06)
                .clamp(*BASE_DEPTHS.start(), *BASE_DEPTHS.end()),
            extent,
            points: (map.nx as u32, map.ny as u32),
            range: map.range(),
            opacity: [0.0; SLOTS],
            // Afternoon sun from the south-west.
            sun: Sun::new(
                (latitude as f32).to_radians(),
                Vec3::new(-0.5, 0.6, 0.6).normalize(),
            ),
            angle: angle as f32,
            heights: map.heights.clone(),
            shadows_for: None,
            screen: None,
        }
    }

    /// Casts the relief's shadows again if the sun moved.
    fn update_shadows(&mut self, rs: &egui_wgpu::RenderState) {
        let sun = self.to_block(self.sun.dir());
        if self.shadows_for == Some(sun) {
            return;
        }
        self.shadows_for = Some(sun);
        let (nx, ny) = (self.points.0 as usize, self.points.1 as usize);
        let mask = shadow::mask(&self.heights, nx, ny, sun);
        let renderer = rs.renderer.read();
        if let Some(gpu) = renderer.callback_resources.get::<Gpu>() {
            let mx = shadow::size(nx, ny).0;
            rs.queue.write_texture(
                gpu.shadows.as_image_copy(),
                &mask,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(mx as u32),
                    rows_per_image: None,
                },
                gpu.shadows.size(),
            );
        }
    }

    /// Hands a layer's draped image, with its `mipmaps`, to the GPU.
    pub fn set_layer(&self, rs: &egui_wgpu::RenderState, id: &str, levels: &[RgbaImage]) {
        let view = image_view(rs, levels);
        let mut renderer = rs.renderer.write();
        if let Some(gpu) = renderer.callback_resources.get_mut::<Gpu>() {
            gpu.layers.insert(id.to_owned(), view);
            gpu.slots.clear(); // rebound by the next `set_stack`
        }
    }

    /// Hands the block's buildings to the GPU.
    pub fn set_buildings(&self, rs: &egui_wgpu::RenderState, mesh: &buildings::Mesh) {
        let mut renderer = rs.renderer.write();
        if let Some(gpu) = renderer.callback_resources.get_mut::<Gpu>()
            && !mesh.indices.is_empty()
        {
            let buffer = |contents: &[u8], usage| {
                rs.device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("bâtiments"),
                        contents,
                        usage,
                    })
            };
            gpu.buildings = Some((
                buffer(
                    bytemuck::cast_slice(&mesh.vertices),
                    wgpu::BufferUsages::VERTEX,
                ),
                buffer(
                    bytemuck::cast_slice(&mesh.indices),
                    wgpu::BufferUsages::INDEX,
                ),
                mesh.indices.len() as u32,
            ));
        }
    }

    /// The visible layers, bottom first, with their opacity. Layers whose image is not
    /// ready yet are skipped; a new order rebinds the images, opacity is just a uniform.
    pub fn set_stack(&mut self, rs: &egui_wgpu::RenderState, stack: &[(&str, f32)]) {
        let mut renderer = rs.renderer.write();
        let Some(gpu) = renderer.callback_resources.get_mut::<Gpu>() else {
            return;
        };
        let shown: Vec<_> = stack
            .iter()
            .filter(|(id, _)| gpu.layers.contains_key(*id))
            .take(SLOTS)
            .collect();
        let ids: Vec<String> = shown.iter().map(|(id, _)| id.to_string()).collect();
        if ids != gpu.slots {
            gpu.bind(&rs.device, ids);
        }
        self.opacity = [0.0; SLOTS];
        for (slot, (_, opacity)) in shown.iter().enumerate() {
            self.opacity[slot] = *opacity;
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, rs: &egui_wgpu::RenderState) {
        let (rect, response) =
            ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
        let aspect = rect.aspect_ratio();
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

        // Draw the block into its image before egui's pass, then show that image.
        self.update_shadows(rs);
        let size = (rect.size() * ui.pixels_per_point()).round();
        let size = [size.x.max(1.0) as u32, size.y.max(1.0) as u32];
        let target = self.screen_target(rs, size);
        let callback = Draw {
            uniforms: self.uniforms(aspect),
            target,
        };
        ui.painter()
            .add(egui_wgpu::Callback::new_paint_callback(rect, callback));
        if let Some((_, id)) = &self.screen {
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            ui.painter().image(*id, rect, uv, egui::Color32::WHITE);
        }
        self.gizmo(ui, rect);
    }

    /// The on-screen image at `size` pixels, made (and handed to egui) on a size change.
    fn screen_target(&mut self, rs: &egui_wgpu::RenderState, size: [u32; 2]) -> Arc<Target> {
        if let Some((target, _)) = &self.screen
            && target.size() == size
        {
            return target.clone();
        }
        let target = Arc::new(Target::new(
            &rs.device,
            size,
            wgpu::TextureUsages::TEXTURE_BINDING,
        ));
        let mut renderer = rs.renderer.write();
        let linear = wgpu::FilterMode::Linear;
        let id = match &self.screen {
            Some((_, id)) => {
                renderer.update_egui_texture_from_wgpu_texture(
                    &rs.device,
                    &target.color,
                    linear,
                    *id,
                );
                *id
            }
            None => renderer.register_native_texture(&rs.device, &target.color, linear),
        };
        self.screen = Some((target.clone(), id));
        target
    }

    /// Blender-style axis widget in the top-right corner: clicking an axis aligns the view,
    /// dragging the widget turns it.
    /// The sun sits among the axes at its true direction, over its day's course; dragging it
    /// moves it where the real sun goes.
    fn gizmo(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        let (dir, right, up) = self.basis();
        let radius = 40.0;
        let center = rect.right_top() + egui::vec2(-radius - 16.0, radius + 16.0);
        let area = egui::Rect::from_center_size(center, egui::Vec2::splat(2.0 * radius + 24.0));
        let response = ui.interact(
            area,
            ui.scope_id().with("gizmo"),
            egui::Sense::click_and_drag(),
        );
        let painter = ui.painter_at(area);
        painter.circle_filled(center, radius + 12.0, egui::Color32::from_black_alpha(40));

        let to_widget = |d: Vec3| center + radius * egui::vec2(d.dot(right), -d.dot(up));
        let path = self.sun.path().into_iter().map(to_widget).collect();
        // Opaque: half transparent, the path looked thinner than the axes.
        let gold = egui::Color32::from_rgb(250, 200, 40);
        painter.line(path, egui::Stroke::new(2.0, gold));
        let sun = self.sun.dir();
        let sun_pos = to_widget(sun);
        let draw_sun = || {
            let rim = egui::Stroke::new(1.5, egui::Color32::from_rgb(200, 120, 20));
            painter.circle(sun_pos, 7.0, egui::Color32::from_rgb(250, 200, 40), rim);
        };
        if sun.dot(dir) < 0.0 {
            draw_sun(); // behind the axes
        }

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
        // The axes follow the pointer: one widget radius of drag is one radian.
        if response.dragged() {
            let d = response.drag_delta() / radius;
            self.yaw -= d.x;
            self.pitch = (self.pitch + d.y).clamp(-FRAC_PI_2, FRAC_PI_2);
        }

        if sun.dot(dir) >= 0.0 {
            draw_sun();
        }
        let response = ui
            .interact(
                egui::Rect::from_center_size(sun_pos, egui::Vec2::splat(18.0)),
                ui.scope_id().with("soleil"),
                egui::Sense::drag(),
            )
            .on_hover_text(format!("Soleil à {} (heure solaire)", self.sun.describe()));
        if response.dragged()
            && let Some(p) = response.interact_pointer_pos()
        {
            // The pointer on the widget's sphere, on the sun's side of it (front or back).
            let v = (p - center) / radius;
            let (x, y) = (v.x, -v.y);
            let z = (1.0 - x * x - y * y).max(0.0).sqrt().copysign(sun.dot(dir));
            let d = (x * right + y * up + z * dir).normalize();
            self.sun.snap(d);
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

    /// The on-screen window: `zoom` high, around the point looked at.
    fn uniforms(&self, aspect: f32) -> Uniforms {
        let (h, w) = (self.zoom, self.zoom * aspect);
        self.uniforms_in([-w, w, -h, h])
    }

    /// Point looked at (the middle of the relief) and the block's diagonal.
    /// A compass direction in the block's own axes.
    fn to_block(&self, d: Vec3) -> Vec3 {
        Quat::from_rotation_y(-self.angle) * d
    }

    fn framing(&self) -> (Vec3, f32) {
        let relief = (self.range.1 - self.range.0) * self.exaggeration;
        let target = self.target + Vec3::Y * relief * 0.3;
        let radius = Vec3::new(self.extent.0, relief + self.base_depth, self.extent.1).length();
        (target, radius)
    }

    /// The window `[left, right, bottom, top]` (m) that holds the whole block, with a margin.
    fn fit_window(&self) -> [f32; 4] {
        let (_, right, up) = self.basis();
        let (target, _) = self.framing();
        let (hx, hz) = (self.extent.0 / 2.0, self.extent.1 / 2.0);
        let top = (self.range.1 - self.range.0) * self.exaggeration;
        let mut win = [f32::MAX, f32::MIN, f32::MAX, f32::MIN];
        // The 8 corners of the block's box, seen along the view.
        for c in 0..8 {
            let p = Quat::from_rotation_y(self.angle)
                * Vec3::new(
                    if c & 1 == 0 { -hx } else { hx },
                    if c & 2 == 0 { -self.base_depth } else { top },
                    if c & 4 == 0 { -hz } else { hz },
                )
                - target;
            let (x, y) = (p.dot(right), p.dot(up));
            win = [win[0].min(x), win[1].max(x), win[2].min(y), win[3].max(y)];
        }
        // 5% around, also room for buildings above the highest ground.
        let m = 0.05 * (win[1] - win[0]).max(win[3] - win[2]);
        [win[0] - m, win[1] + m, win[2] - m, win[3] + m]
    }

    fn uniforms_in(&self, [l, r, b, t]: [f32; 4]) -> Uniforms {
        let (dir, _, up) = self.basis();
        // The camera stands outside the whole block.
        let (target, radius) = self.framing();
        // The block turned as on the map, the camera on the compass.
        let view = look_at_mat4(target + dir * radius * 2.0, target, up)
            * Mat4::from_rotation_y(self.angle);
        let proj = orthographic(l, r, b, t, 0.0, radius * 4.0);
        let sun = self.to_block(self.sun.dir());
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
            // SLOTS opacities, padded to whole vec4s.
            opacity: {
                let mut o = [0.0; 12];
                o[..SLOTS].copy_from_slice(&self.opacity);
                bytemuck::cast(o)
            },
        }
    }

    /// Renders the whole block, seen as on screen, off-screen on a transparent background,
    /// its longer side `size` px (within the GPU's limit). Returns `(width, height, rgba)`.
    pub fn render_image(&self, rs: &egui_wgpu::RenderState, size: u32) -> (u32, u32, Vec<u8>) {
        let (device, queue) = (&rs.device, &rs.queue);
        let size = size.min(device.limits().max_texture_dimension_2d) as f32;
        let window = self.fit_window();
        let aspect = (window[1] - window[0]) / (window[3] - window[2]);
        let (width, height) = if aspect >= 1.0 {
            (size, size / aspect)
        } else {
            (size * aspect, size)
        };
        let (width, height) = (
            width.round().max(1.0) as u32,
            height.round().max(1.0) as u32,
        );
        let target = Target::new(device, [width, height], wgpu::TextureUsages::COPY_SRC);
        let renderer = rs.renderer.read();
        let gpu: &Gpu = renderer.callback_resources.get().expect("bloc chargé");
        queue.write_buffer(
            &gpu.uniforms,
            0,
            bytemuck::bytes_of(&self.uniforms_in(window)),
        );
        let mut encoder = device.create_command_encoder(&Default::default());
        gpu.render(&mut encoder, &target);
        queue.submit([encoder.finish()]);
        let mut rgba = read_texture(device, queue, &target.texture);
        // Antialiased edges come out premultiplied; PNG wants straight alpha.
        for px in rgba
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .filter(|px| px[3] > 0)
        {
            for c in 0..3 {
                px[c] = (px[c] as u32 * 255 / px[3] as u32) as u8;
            }
        }
        (width, height, rgba)
    }

    /// Gives the on-screen image back to egui, before this view is dropped.
    pub fn release(&mut self, rs: &egui_wgpu::RenderState) {
        if let Some((_, id)) = self.screen.take() {
            rs.renderer.write().free_texture(&id);
        }
    }
}

/// Draws the block into its on-screen image, before egui's pass shows it.
struct Draw {
    uniforms: Uniforms,
    target: Arc<Target>,
}

impl egui_wgpu::CallbackTrait for Draw {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(gpu) = resources.get::<Gpu>() {
            queue.write_buffer(&gpu.uniforms, 0, bytemuck::bytes_of(&self.uniforms));
            gpu.render(encoder, &self.target);
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        _pass: &mut wgpu::RenderPass<'static>,
        _resources: &egui_wgpu::CallbackResources,
    ) {
        // Already drawn in `prepare`; egui shows the image.
    }
}

impl Gpu {
    /// Binds the layers `ids` to the slots, bottom first; missing ones stay blank.
    fn bind(&mut self, device: &wgpu::Device, ids: Vec<String>) {
        let mut entries = vec![
            wgpu::BindGroupEntry {
                binding: 0,
                resource: self.uniforms.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&self.heights),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(&self.sampler),
            },
        ];
        for slot in 0..SLOTS {
            let view = ids.get(slot).and_then(|id| self.layers.get(id));
            entries.push(wgpu::BindGroupEntry {
                binding: 3 + slot as u32,
                resource: wgpu::BindingResource::TextureView(view.unwrap_or(&self.blank)),
            });
        }
        let shadows = self.shadows.create_view(&Default::default());
        entries.push(wgpu::BindGroupEntry {
            binding: 3 + SLOTS as u32,
            resource: wgpu::BindingResource::TextureView(&shadows),
        });
        self.bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layout,
            entries: &entries,
        });
        self.slots = ids;
    }

    /// Draws the block into `target`, on a transparent background, resolving the samples
    /// into a premultiplied image.
    fn render(&self, encoder: &mut wgpu::CommandEncoder, target: &Target) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("bloc"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target.msaa,
                depth_slice: None,
                resolve_target: Some(&target.color),
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Discard,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &target.depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..self.index_count, 0, 0..1);
        if let Some((vertices, indices, count)) = &self.buildings {
            pass.set_pipeline(&self.building_pipeline);
            pass.set_vertex_buffer(0, vertices.slice(..));
            pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..*count, 0, 0..1);
        }
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
    let (mx, my) = shadow::size(map.nx, map.ny);
    let shadows = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("ombres"),
        size: wgpu::Extent3d {
            width: mx as u32,
            height: my as u32,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("uniforms"),
        size: size_of::<Uniforms>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    // Uniforms, heights, a sampler, then one image per layer slot.
    let mut entries = vec![
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
        wgpu::BindGroupLayoutEntry {
            binding: 2,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        },
    ];
    // The layers, then the shadows.
    for slot in 0..=SLOTS {
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: 3 + slot as u32,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        });
    }
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &entries,
    });
    // Trilinear, and anisotropic so that layers stay sharp on slopes seen edge-on.
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        anisotropy_clamp: 8,
        ..Default::default()
    });
    let blank = image_view(rs, &[RgbaImage::new(1, 1)]);

    let shader = device.create_shader_module(wgpu::include_wgsl!("terrain.wgsl"));
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    // Terrain and buildings differ only by their vertices.
    let pipeline = |entry, buffer: wgpu::VertexBufferLayout| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(entry),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                buffers: &[Some(buffer)],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(COLOR_FORMAT.into())],
            }),
            primitive: Default::default(), // no culling: walls are seen from both sides when orbiting
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: SAMPLES,
                ..Default::default()
            },
            multiview_mask: None,
            cache: None,
        })
    };
    let building_pipeline = pipeline(
        "vs_building",
        wgpu::VertexBufferLayout {
            array_stride: size_of::<buildings::Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32, 2 => Float32, 3 => Float32x2],
        },
    );
    let pipeline = pipeline(
        "vs_main",
        wgpu::VertexBufferLayout {
            array_stride: size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Uint16x2, 1 => Uint8x2],
        },
    );

    let (vertices, indices) = mesh(map.nx, map.ny);
    let buffer = |label, contents: &[u8], usage| {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents,
            usage,
        })
    };
    // The bind group is made by `bind`; a placeholder until then.
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[],
        }),
        entries: &[],
    });
    let mut gpu = Gpu {
        pipeline,
        layout,
        bind_group,
        uniforms,
        heights: texture.create_view(&Default::default()),
        shadows,
        sampler,
        blank,
        layers: HashMap::new(),
        slots: Vec::new(),
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
        building_pipeline,
        buildings: None,
    };
    gpu.bind(device, Vec::new());
    gpu
}

/// The pixels of a 4-byte texture, rows top to bottom, once the GPU work queued so far is done.
pub fn read_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
) -> Vec<u8> {
    let (width, height) = (texture.width(), texture.height());
    let row = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("lecture"),
        size: (row * height) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: None,
            },
        },
        texture.size(),
    );
    queue.submit([encoder.finish()]);

    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("rendu terminé");
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for line in slice
        .get_mapped_range()
        .expect("tampon lu")
        .chunks(row as usize)
    {
        rgba.extend_from_slice(&line[..(width * 4) as usize]);
    }
    rgba
}

/// The image and its successive halves down to 1 × 1 (mipmaps), so that thin lines and
/// labels do not flicker on a block seen from afar. Slow on big images: off the UI thread.
pub fn mipmaps(image: RgbaImage) -> Vec<RgbaImage> {
    let mut levels = vec![image];
    while let Some(last) = levels.last().filter(|l| l.width().max(l.height()) > 1) {
        let (w, h) = ((last.width() / 2).max(1), (last.height() / 2).max(1));
        // Premultiplied pixels, so plain averaging is right.
        let half = image::imageops::resize(last, w, h, image::imageops::FilterType::Triangle);
        levels.push(half);
    }
    levels
}

/// An sRGB-coded, premultiplied image and its mipmaps as a texture, sampled as is like the
/// shader's colours.
fn image_view(rs: &egui_wgpu::RenderState, levels: &[RgbaImage]) -> wgpu::TextureView {
    let data: Vec<u8> = levels.iter().flat_map(|l| l.as_raw()).copied().collect();
    rs.device
        .create_texture_with_data(
            &rs.queue,
            &wgpu::TextureDescriptor {
                label: Some("calque"),
                size: wgpu::Extent3d {
                    width: levels[0].width(),
                    height: levels[0].height(),
                    depth_or_array_layers: 1,
                },
                mip_level_count: levels.len() as u32,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &data,
        )
        .create_view(&Default::default())
}

#[cfg(test)]
mod tests {
    #[test]
    fn mipmaps_halve_down_to_one_pixel() {
        let sizes: Vec<_> = super::mipmaps(image::RgbaImage::new(5, 3))
            .iter()
            .map(|l| l.dimensions())
            .collect();
        assert_eq!(sizes, [(5, 3), (2, 1), (1, 1)]);
    }
}
