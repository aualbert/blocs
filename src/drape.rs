//! Layers draped on the block: each layer is drawn once into an image aligned on the block's
//! Lambert-93 frame, from the same tiles as the map. The GPU then stacks these images.
//!
//! Image layers are resampled from their tiles. The Plan IGN is drawn by walkers, as on the
//! map, into an off-screen Web Mercator image, which is then resampled the same way.

use std::collections::HashMap;
use std::f64::consts::PI;
use std::sync::Arc;

use eframe::egui::{self, mutex::RwLock, pos2, vec2};
use eframe::{egui_wgpu, wgpu};
use image::RgbaImage;
use walkers::{Map, MapMemory, Style, Tile, TileId, TilePiece, Tiles, lon_lat};

use crate::lambert93;
use crate::map::Frame;
use crate::relief::Result;
use crate::tiles::Store;

/// Ground size of a texel (m): a 10 km block gets a 3 334 px image. The Plan IGN of
/// smaller blocks gets finer ones, so that its labels stay sharp.
const TEXEL: f64 = 3.0;
/// Tile level read for image layers: about 3 m per pixel in France.
const ZOOM: u8 = 15;
/// Texels between exact reprojections; positions in between are interpolated.
const STEP: u32 = 16;
const THREADS: usize = 6;
/// The Plan IGN is styled as on a map showing the frame this many points wide: details,
/// line widths and labels as the user saw them when drawing it.
const SCREEN: f64 = 1500.0;
/// Plan IGN tiles are 512 px, one level above walkers' 256 px map zoom.
const PLAN_TILE: u32 = 512;
/// Equator length (m).
const EARTH: f64 = 40_075_016.7;

/// An image layer over `frame`, rows north to south, premultiplied alpha.
pub fn drape(store: &Store, frame: &Frame) -> Result<RgbaImage> {
    let zoom = ZOOM.min(store.source.max_zoom());
    let tiles = fetch(store, &crate::ign::client()?, frame, zoom);
    let decoded: HashMap<_, _> = tiles
        .iter()
        .filter_map(|(&k, b)| Some((k, image::load_from_memory(b).ok()?.to_rgba8())))
        .collect();
    // One tile pixel, transparent where the tile is missing.
    let pixel = |px: i64, py: i64| -> [f32; 4] {
        let key = ((px.div_euclid(256)) as u32, (py.div_euclid(256)) as u32);
        decoded.get(&key).map_or([0.0; 4], |t| {
            t.get_pixel(px.rem_euclid(256) as u32, py.rem_euclid(256) as u32)
                .0
                .map(f32::from)
        })
    };
    Ok(reproject(
        frame,
        TEXEL,
        256.0 * (1u32 << zoom) as f64,
        pixel,
    ))
}

/// The Plan IGN tiles over a frame, fetched once for all its categories.
pub struct PlanIgn {
    frame: Frame,
    /// Map zoom (256 px grid) the frame is drawn at.
    zoom: u8,
    tiles: HashMap<(u32, u32), Vec<u8>>,
}

impl PlanIgn {
    pub fn fetch(store: &Store, frame: &Frame) -> Result<Self> {
        let (w, h) = frame.size();
        let lat = centre(frame).1.to_radians();
        let zoom = (EARTH * lat.cos() * SCREEN / (256.0 * w.max(h)))
            .log2()
            .round();
        let zoom = zoom.clamp(1.0, store.source.max_zoom() as f64 + 1.0) as u8;
        let tiles = fetch(store, &crate::ign::client()?, frame, zoom - 1);
        if tiles.is_empty() {
            return Err("tuiles du Plan IGN indisponibles".into());
        }
        Ok(Self {
            frame: *frame,
            zoom,
            tiles,
        })
    }

    /// The 3D buildings of the frame, from the same tiles as the draped Plan IGN.
    pub fn buildings(&self) -> crate::buildings::Mesh {
        crate::buildings::mesh(self.zoom - 1, &self.tiles, &self.frame)
    }

    /// The Plan IGN drawn with `style` over the frame, rows north to south, premultiplied.
    pub fn draw(&self, style: &Style, rs: &egui_wgpu::RenderState) -> Result<RgbaImage> {
        // Styled as walkers styles them for the map.
        let level = self.zoom - 1;
        let styled = self
            .tiles
            .iter()
            .filter_map(|(&(x, y), bytes)| {
                let tile = Tile::from_mvt(bytes, style, level, PLAN_TILE).ok()?;
                Some((TileId { x, y, zoom: level }, tile))
            })
            .collect();

        // Mercator image covering the frame: `scale` pixels per world unit, `origin` its
        // top-left pixel, and `ppp` pixels per map point, so that a pixel is about a texel.
        let world = 256.0 * (1u32 << self.zoom) as f64; // points per world unit
        let lat = centre(&self.frame).1.to_radians();
        let point = EARTH * lat.cos() / world; // ground size of a map point (m)
        let texel = TEXEL.min(point);
        let ppp = point / texel;
        let scale = world * ppp;
        let corners = self.frame.corners().map(|(x, y)| {
            let (lon, lat) = lambert93::to_wgs84(x, y);
            to_mercator(lon, lat)
        });
        let bound = |c: fn(&(f64, f64)) -> f64, f: fn(f64, f64) -> f64, init| {
            corners.iter().map(c).fold(init, f) * scale
        };
        let origin = (
            bound(|c| c.0, f64::min, f64::MAX).floor(),
            bound(|c| c.1, f64::min, f64::MAX).floor(),
        );
        let w = (bound(|c| c.0, f64::max, f64::MIN).ceil() - origin.0) as u32;
        let h = (bound(|c| c.1, f64::max, f64::MIN).ceil() - origin.1) as u32;
        let (lon, lat) = from_mercator(
            (origin.0 + w as f64 / 2.0) / scale,
            (origin.1 + h as f64 / 2.0) / scale,
        );

        let image = render_map(Styled(styled), lon, lat, self.zoom, [w, h], ppp as f32, rs)?;
        let pixel = |px: i64, py: i64| -> [f32; 4] {
            let (x, y) = (px - origin.0 as i64, py - origin.1 as i64);
            image
                .get_pixel_checked(x as u32, y as u32)
                .filter(|_| x >= 0 && y >= 0)
                .map_or([0.0; 4], |p| p.0.map(f32::from))
        };
        Ok(reproject(&self.frame, texel, scale, pixel))
    }
}

/// Styled tiles, handed to walkers' map as they are.
struct Styled(HashMap<TileId, Tile>);

impl Tiles for Styled {
    fn at(&mut self, id: TileId) -> Option<TilePiece> {
        let whole = egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        Some(TilePiece::new(self.0.get(&id)?.clone(), whole))
    }

    fn attribution(&self) -> walkers::sources::Attribution {
        crate::tiles::IGN
    }

    fn tile_size(&self) -> u32 {
        PLAN_TILE
    }
}

/// Draws walkers' map of `tiles`, centred on `lon, lat`, into a `size` pixel image: one egui
/// pass without a window, rendered off-screen with walkers' own renderer.
fn render_map(
    mut tiles: Styled,
    lon: f64,
    lat: f64,
    zoom: u8,
    size: [u32; 2],
    ppp: f32,
    rs: &egui_wgpu::RenderState,
) -> Result<RgbaImage> {
    let ctx = egui::Context::default();
    let points = vec2(size[0] as f32, size[1] as f32) / ppp;
    let mut input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(pos2(0.0, 0.0), points)),
        ..Default::default()
    };
    input
        .viewports
        .entry(egui::ViewportId::ROOT)
        .or_default()
        .native_pixels_per_point = Some(ppp);

    // Fonts only exist once a pass has run; its font atlas is uploaded with the second's.
    let mut first = ctx.run_ui(input.clone(), |_| {});
    let mut memory = MapMemory::default();
    memory.set_zoom(zoom as f64)?;
    memory.center_at(lon_lat(lon, lat));
    let mut output = ctx.run_ui(input, |ui| {
        ui.add(Map::new(Some(&mut tiles), &mut memory, lon_lat(lon, lat)));
    });
    let primitives = ctx.tessellate(output.shapes, ppp);

    // A renderer of our own, walkers' installed in it as in the app's.
    let options = egui_wgpu::RendererOptions {
        dithering: false,
        ..Default::default()
    };
    // walkers builds its pipeline for `target_format`: the image's, not the window's.
    let state = egui_wgpu::RenderState {
        renderer: Arc::new(RwLock::new(egui_wgpu::Renderer::new(
            &rs.device, FORMAT, options,
        ))),
        target_format: FORMAT,
        ..rs.clone()
    };
    walkers::install_renderer(Some(&state));
    let mut renderer = state.renderer.write();
    let (device, queue) = (&rs.device, &rs.queue);
    let deltas = first
        .textures_delta
        .set
        .iter()
        .chain(&output.textures_delta.set);
    for (id, delta) in deltas.flat_map(|(id, d)| d.iter().map(move |d| (*id, d))) {
        renderer.update_texture(device, queue, id, delta);
    }
    first.textures_delta.clear();
    output.textures_delta.clear();

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("plan ign"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: size,
        pixels_per_point: ppp,
    };
    let mut encoder = device.create_command_encoder(&Default::default());
    let mut commands = renderer.update_buffers(device, queue, &mut encoder, &primitives, &screen);
    let view = target.create_view(&Default::default());
    let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("plan ign"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })],
        ..Default::default()
    });
    renderer.render(&mut pass.forget_lifetime(), &primitives, &screen);
    commands.push(encoder.finish());
    queue.submit(commands);

    let rgba = crate::view3d::read_texture(device, queue, &target);
    Ok(RgbaImage::from_raw(size[0], size[1], rgba).expect("taille de l'image"))
}

/// Colours as displayed, like the block's other images.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Tiles of `zoom` covering the frame, fetched in parallel; missing ones are skipped.
fn fetch(
    store: &Store,
    http: &reqwest::blocking::Client,
    frame: &Frame,
    zoom: u8,
) -> HashMap<(u32, u32), Vec<u8>> {
    let n = (1u32 << zoom) as f64;
    let corners = frame.corners().map(|(x, y)| {
        let (lon, lat) = lambert93::to_wgs84(x, y);
        to_mercator(lon, lat)
    });
    let range = |c: fn(&(f64, f64)) -> f64| {
        let v = corners.iter().map(c);
        let lo = v.clone().fold(f64::MAX, f64::min);
        let hi = v.fold(f64::MIN, f64::max);
        (lo * n) as u32..=(hi * n) as u32
    };
    let ids: Vec<(u32, u32)> = range(|c| c.0)
        .flat_map(|x| range(|c| c.1).map(move |y| (x, y)))
        .collect();

    std::thread::scope(|s| {
        let chunks = ids.chunks(ids.len().div_ceil(THREADS).max(1));
        let jobs: Vec<_> = chunks
            .map(|chunk| {
                s.spawn(move || {
                    chunk
                        .iter()
                        .filter_map(|&(x, y)| {
                            let bytes = store.bytes(http, TileId { x, y, zoom }).ok()?;
                            Some(((x, y), bytes))
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        jobs.into_iter().flat_map(|j| j.join().unwrap()).collect()
    })
}

/// Resamples a Web Mercator image onto the frame's Lambert-93 grid of `texel` m (bilinear).
/// `scale`: image pixels per world unit; `pixel`: one image pixel, by world pixel position.
fn reproject(
    frame: &Frame,
    texel: f64,
    scale: f64,
    pixel: impl Fn(i64, i64) -> [f32; 4] + Sync,
) -> RgbaImage {
    let (w, h) = frame.size();
    let (w, h) = ((w / texel).ceil() as u32, (h / texel).ceil() as u32);
    // Exact Mercator positions every STEP texels; the projection is smooth in between.
    let (gw, gh) = (w / STEP + 2, h / STEP + 2);
    let grid: Vec<(f64, f64)> = (0..gh)
        .flat_map(|j| (0..gw).map(move |i| (i, j)))
        .map(|(i, j)| {
            let x = frame.x0 + (i * STEP) as f64 * texel;
            let y = frame.y1 - (j * STEP) as f64 * texel;
            let (lon, lat) = lambert93::to_wgs84(x, y);
            let (mx, my) = to_mercator(lon, lat);
            (mx * scale, my * scale)
        })
        .collect();
    let at = |u: f64, v: f64| {
        let (i, j) = ((u as u32).min(gw - 2), (v as u32).min(gh - 2));
        let (fu, fv) = (u - i as f64, v - j as f64);
        let g = |di: u32, dj: u32| grid[((j + dj) * gw + i + di) as usize];
        let lerp =
            |a: (f64, f64), b: (f64, f64), t: f64| (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
        lerp(lerp(g(0, 0), g(1, 0), fu), lerp(g(0, 1), g(1, 1), fu), fv)
    };

    let mut out = RgbaImage::new(w, h);
    let row_bytes = (w * 4) as usize;
    let band_rows = (h as usize).div_ceil(THREADS).max(1);
    std::thread::scope(|s| {
        // Each thread fills its own band of rows.
        for (b, band) in out.chunks_mut(band_rows * row_bytes).enumerate() {
            let (at, pixel) = (&at, &pixel);
            s.spawn(move || {
                for (r, row) in band.chunks_mut(row_bytes).enumerate() {
                    let j = b * band_rows + r;
                    for i in 0..w as usize {
                        let (x, y) = at(
                            (i as f64 + 0.5) / STEP as f64,
                            (j as f64 + 0.5) / STEP as f64,
                        );
                        let (x, y) = (x - 0.5, y - 0.5); // pixel centres
                        let (x0, y0) = (x.floor() as i64, y.floor() as i64);
                        let (fx, fy) = ((x - x0 as f64) as f32, (y - y0 as f64) as f32);
                        let (a, b, c, d) = (
                            pixel(x0, y0),
                            pixel(x0 + 1, y0),
                            pixel(x0, y0 + 1),
                            pixel(x0 + 1, y0 + 1),
                        );
                        for k in 0..4 {
                            let top = a[k] + (b[k] - a[k]) * fx;
                            let bottom = c[k] + (d[k] - c[k]) * fx;
                            row[i * 4 + k] = (top + (bottom - top) * fy).round() as u8;
                        }
                    }
                }
            });
        }
    });
    out
}

/// Frame centre in degrees.
fn centre(frame: &Frame) -> (f64, f64) {
    lambert93::to_wgs84((frame.x0 + frame.x1) / 2.0, (frame.y0 + frame.y1) / 2.0)
}

/// Degrees → Web Mercator world units (0..1, y down).
fn to_mercator(lon: f64, lat: f64) -> (f64, f64) {
    (
        (lon + 180.0) / 360.0,
        (1.0 - lat.to_radians().tan().asinh() / PI) / 2.0,
    )
}

/// Web Mercator world units → degrees.
pub fn from_mercator(x: f64, y: f64) -> (f64, f64) {
    (
        x * 360.0 - 180.0,
        (PI * (1.0 - 2.0 * y)).sinh().atan().to_degrees(),
    )
}

/// Live check: `BLOCS_DRAPE_DIR=<dir> cargo test drapes_clecy -- --ignored`, then look at the image.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore]
    fn drapes_clecy() {
        let out = std::path::PathBuf::from(std::env::var("BLOCS_DRAPE_DIR").unwrap());
        let frame = Frame::from_corners((442_850.0, 6_871_550.0), (445_850.0, 6_874_550.0));
        let image = drape(&Store::new("photo", crate::layers::PHOTO), &frame).unwrap();
        assert_eq!(image.dimensions(), (1000, 1000));
        image.save(out.join("photo.png")).unwrap();

        // The Plan IGN, whole and by category, on a GPU without a window; the window format
        // differs from the images', as in the app.
        const WINDOW: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        let (device, queue) =
            pollster::block_on(adapter.request_device(&Default::default())).unwrap();
        let renderer = egui_wgpu::Renderer::new(&device, WINDOW, Default::default());
        let rs = egui_wgpu::RenderState {
            available_adapters: vec![adapter.clone()],
            adapter,
            instance,
            device,
            queue,
            target_format: WINDOW,
            renderer: Arc::new(RwLock::new(renderer)),
        };
        let plan = PlanIgn::fetch(
            &Store::new("planign", crate::layers::Source::PlanIgn),
            &frame,
        )
        .unwrap();
        let categories = crate::style::Category::ALL.map(Some);
        for category in categories.into_iter().chain([None]) {
            let image = plan.draw(&crate::style::plan_ign(category), &rs).unwrap();
            let name = category.map_or("fond", |c| c.id());
            image.save(out.join(format!("{name}.png"))).unwrap();
        }
    }

    #[test]
    fn mercator_round_trips() {
        let (x, y) = to_mercator(-0.49, 48.92);
        let (lon, lat) = from_mercator(x, y);
        assert!((lon + 0.49).abs() < 1e-9 && (lat - 48.92).abs() < 1e-9);
    }
}
