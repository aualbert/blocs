//! Layers draped on the block: each layer is drawn once into an image aligned on the block's
//! Lambert-93 frame, from the same tiles as the map. The GPU then stacks these images.

use std::collections::HashMap;
use std::f64::consts::PI;

use image::RgbaImage;
use walkers::TileId;

use crate::lambert93;
use crate::layers::Source;
use crate::map::Frame;
use crate::relief::Result;
use crate::tiles::Store;

/// Ground size of a texel (m): a 10 km block gets a 3 334 px image.
pub const TEXEL: f64 = 3.0;
/// Tile level read for the block: about 3 m per pixel in France.
const ZOOM: u8 = 15;
/// Texels between exact reprojections; positions in between are interpolated.
const STEP: u32 = 16;
const THREADS: usize = 6;

/// Image size for a frame, in texels.
pub fn size(frame: &Frame) -> (u32, u32) {
    let (w, h) = frame.size();
    (
        (w / TEXEL).ceil().max(1.0) as u32,
        (h / TEXEL).ceil().max(1.0) as u32,
    )
}

/// The layer's image over `frame`, rows north to south, straight alpha.
pub fn drape(store: &Store, frame: &Frame) -> Result<RgbaImage> {
    if let Source::PlanIgn = *store.source {
        return Err("le Plan IGN vectoriel n'est pas encore drapé sur le bloc".into());
    }
    let http = crate::ign::client()?;
    let zoom = ZOOM.min(store.source.max_zoom());
    let (w, h) = size(frame);
    let tiles = fetch(store, &http, frame, zoom);
    Ok(reproject(&tiles, frame, zoom, w, h))
}

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

/// Resamples Web Mercator image tiles onto the Lambert-93 texel grid (bilinear).
fn reproject(
    tiles: &HashMap<(u32, u32), Vec<u8>>,
    frame: &Frame,
    zoom: u8,
    w: u32,
    h: u32,
) -> RgbaImage {
    let decoded: HashMap<_, _> = tiles
        .iter()
        .filter_map(|(&k, b)| Some((k, image::load_from_memory(b).ok()?.to_rgba8())))
        .collect();
    let world = 256.0 * (1u32 << zoom) as f64; // world size in tile pixels

    // Exact Mercator positions every STEP texels; the projection is smooth in between.
    let (gw, gh) = (w / STEP + 2, h / STEP + 2);
    let grid: Vec<(f64, f64)> = (0..gh)
        .flat_map(|j| (0..gw).map(move |i| (i, j)))
        .map(|(i, j)| {
            let x = frame.x0 + (i * STEP) as f64 * TEXEL;
            let y = frame.y1 - (j * STEP) as f64 * TEXEL;
            let (lon, lat) = lambert93::to_wgs84(x, y);
            let (mx, my) = to_mercator(lon, lat);
            (mx * world, my * world)
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
    // One tile pixel, transparent where the tile is missing.
    let pixel = |px: i64, py: i64| -> [f32; 4] {
        let key = ((px.div_euclid(256)) as u32, (py.div_euclid(256)) as u32);
        decoded.get(&key).map_or([0.0; 4], |t| {
            t.get_pixel(px.rem_euclid(256) as u32, py.rem_euclid(256) as u32)
                .0
                .map(f32::from)
        })
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

/// Degrees → Web Mercator world units (0..1, y down).
fn to_mercator(lon: f64, lat: f64) -> (f64, f64) {
    (
        (lon + 180.0) / 360.0,
        (1.0 - lat.to_radians().tan().asinh() / PI) / 2.0,
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
    }
}
