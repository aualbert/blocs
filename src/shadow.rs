//! Shadows cast by the relief, for one sun direction: one sweep over the grid, away from
//! the sun, each point taking the shadow left by the line before it.

use glam::Vec3;

use crate::relief::CELL;

/// Points of the grid kept for the shadows, along each axis: every 10 m is plenty.
pub const STEP: usize = 2;
/// Width of a shadow's fading edge, in steps.
const SOFT: f32 = 3.0;

/// Heights (degrees) and number of directions of the suns whose shadows make `sky`. Each
/// height stands for an equal share of the light the sky sheds on level ground: most of it
/// comes from high up (cosine law), so the heights are 90° − asin(√((k + ½) / 3)).
const SKY_HEIGHTS: [f32; 3] = [65.9, 45.0, 24.1];
const SKY_DIRECTIONS: usize = 8;

/// Share of the sky each point sees, every STEP points, 255 under an open sky: the relief's
/// shadows under suns all around, averaged. Valleys and gullies see less of it. Slow (tens
/// of ms), once per block: off the UI thread.
pub fn sky(heights: &[f32], nx: usize, ny: usize) -> Vec<u8> {
    let suns = SKY_HEIGHTS.iter().flat_map(|h| {
        let (sh, ch) = h.to_radians().sin_cos();
        (0..SKY_DIRECTIONS).map(move |k| {
            let a = (k as f32 + 0.5) * std::f32::consts::TAU / SKY_DIRECTIONS as f32;
            Vec3::new(a.cos() * ch, sh, a.sin() * ch)
        })
    });
    // One thread per sun.
    let masks: Vec<Vec<u8>> = std::thread::scope(|s| {
        let jobs: Vec<_> = suns
            .map(|sun| s.spawn(move || mask(heights, nx, ny, sun)))
            .collect();
        jobs.into_iter().map(|j| j.join().unwrap()).collect()
    });
    let n = masks.len() as u32;
    (0..masks[0].len())
        .map(|k| (masks.iter().map(|m| m[k] as u32).sum::<u32>() / n) as u8)
        .collect()
}

/// Size of the shadow mask for a grid of `nx` × `ny` points.
pub fn size(nx: usize, ny: usize) -> (usize, usize) {
    size_at(nx, ny, STEP)
}

fn size_at(nx: usize, ny: usize, step: usize) -> (usize, usize) {
    ((nx - 1) / step + 1, (ny - 1) / step + 1)
}

/// 255 where the sun reaches the point, 0 where the relief hides it, every STEP points.
/// In between on a shadow's edge.
/// `heights` are rows south → north of `nx` points; `sun` points towards the sun
/// (x east, y up, z south).
pub fn mask(heights: &[f32], nx: usize, ny: usize, sun: Vec3) -> Vec<u8> {
    cast(heights, nx, ny, sun, STEP)
}

/// The same, cast every 2 × STEP points then spread over the mask's: four times faster but
/// softer, for while the sun is dragged.
pub fn rough(heights: &[f32], nx: usize, ny: usize, sun: Vec3) -> Vec<u8> {
    let coarse = cast(heights, nx, ny, sun, 2 * STEP);
    let (cx, cy) = size_at(nx, ny, 2 * STEP);
    let (mx, my) = size(nx, ny);
    // Mask points fall on coarse ones, or halfway between two: their average.
    let mut out = Vec::with_capacity(mx * my);
    for j in 0..my {
        let row = |r: usize| &coarse[r.min(cy - 1) * cx..][..cx];
        let (a, b) = (row(j / 2), row(j / 2 + j % 2));
        out.extend((0..mx).map(|i| {
            let (c0, c1) = (i / 2, (i / 2 + i % 2).min(cx - 1));
            ((a[c0] as u16 + a[c1] as u16 + b[c0] as u16 + b[c1] as u16) / 4) as u8
        }));
    }
    out
}

/// `mask` every `step` points.
fn cast(heights: &[f32], nx: usize, ny: usize, sun: Vec3, step: usize) -> Vec<u8> {
    let (mx, my) = size_at(nx, ny, step);
    let flat = sun.x.hypot(sun.z);
    if sun.y <= 0.0 {
        return vec![0; mx * my];
    }
    let mut lit = vec![255; mx * my];
    if flat < 1e-6 {
        return lit;
    }
    // Towards the sun on the grid (i east, j north). Lines run across the main axis `a`;
    // from one line to the next, the sun is one step along `a` and `slope` steps along `b`.
    let (di, dj) = (sun.x, -sun.z);
    let along_i = di.abs() >= dj.abs();
    let (na, nb, da, db) = if along_i {
        (mx, my, di, dj)
    } else {
        (my, mx, dj, di)
    };
    let index = |a: usize, b: usize| if along_i { b * mx + a } else { a * mx + b };
    let height = |a: usize, b: usize| {
        let (i, j) = if along_i { (a, b) } else { (b, a) };
        heights[j * step * nx + i * step]
    };
    let slope = db / da.abs();
    // How much lower the shadow line is one step further from the sun, in true metres.
    let drop = (step as f64 * CELL) as f32 * (1.0 + slope * slope).sqrt() * sun.y / flat;

    // Height of the shadow line over each point of the previous line; none before the first.
    let mut prev = vec![f32::NEG_INFINITY; nb];
    let mut cur = vec![0.0; nb];
    for step in 0..na {
        let a = if da > 0.0 { na - 1 - step } else { step };
        for b in 0..nb {
            let x = b as f32 + slope;
            let shade = if x < 0.0 || x > (nb - 1) as f32 {
                f32::NEG_INFINITY // the sun comes from outside the block
            } else {
                let k = x as usize;
                let f = x - k as f32;
                let next = prev[(k + 1).min(nb - 1)];
                // Interpolate only between finite heights; infinities stay as they are.
                if f == 0.0 {
                    prev[k]
                } else {
                    prev[k] + (next - prev[k]) * f
                }
            } - drop;
            let h = height(a, b);
            // Soft edges: full shadow only SOFT steps behind the shadow line's end.
            let depth = (shade - h) / (SOFT * drop);
            if depth > 0.0 {
                lit[index(a, b)] = ((1.0 - depth).max(0.0) * 255.0) as u8;
            }
            cur[b] = h.max(shade);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    lit
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trench_sees_less_sky_than_open_ground() {
        // Flat ground, with a 100 m deep, 20 m wide trench running north-south in the middle.
        let (nx, ny) = (41, 41);
        let mut h = vec![0.0; nx * ny];
        for j in 0..ny {
            for i in 18..23 {
                h[j * nx + i] = -100.0;
            }
        }
        let sky = sky(&h, nx, ny);
        let mx = size(nx, ny).0;
        let at = |i: usize| sky[10 * mx + i / STEP];
        assert_eq!(at(0), 255);
        assert!(at(20) < 128, "{}", at(20));
    }

    #[test]
    fn rough_shadows_fall_where_sharp_ones_do() {
        // A 100 m high, 40 m thick wall at the west edge: rough masks skip thinner features.
        let (nx, ny) = (161, 9);
        let mut h = vec![0.0; nx * ny];
        for j in 0..ny {
            h[j * nx..j * nx + 9].fill(100.0);
        }
        let sun = Vec3::new(-1.0, 1.0, 0.0).normalize();
        let (sharp, rough) = (mask(&h, nx, ny, sun), rough(&h, nx, ny, sun));
        assert_eq!(sharp.len(), rough.len());
        let mx = size(nx, ny).0;
        // 40 m into the shadow (it reaches 140 m), and well past its end.
        assert_eq!((sharp[mx + 8], rough[mx + 8]), (0, 0));
        assert_eq!((sharp[mx + 30], rough[mx + 30]), (255, 255));
    }

    #[test]
    fn wall_shades_the_east_under_a_western_sun() {
        // A 100 m wall at 10 m on flat ground, sun from the west at 45°.
        let (nx, ny) = (81, 5);
        let mut h = vec![0.0; nx * ny];
        for j in 0..ny {
            h[j * nx + 2] = 100.0;
        }
        let lit = mask(&h, nx, ny, Vec3::new(-1.0, 1.0, 0.0).normalize());
        // The shadow reaches 100 m east of the wall, no further; mask points are 10 m apart.
        let mx = size(nx, ny).0;
        assert_eq!(lit[mx], 255);
        assert_eq!(lit[mx + 5], 0);
        assert_eq!(lit[mx + 15], 255);
    }
}
