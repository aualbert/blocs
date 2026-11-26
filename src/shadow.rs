//! Shadows cast by the relief, for one sun direction: one sweep over the grid, away from
//! the sun, each point taking the shadow left by the line before it.

use glam::Vec3;

use crate::relief::CELL;

/// Points of the grid kept for the shadows, along each axis: every 10 m is plenty.
pub const STEP: usize = 2;
/// Width of a shadow's fading edge, in steps.
const SOFT: f32 = 3.0;

/// Size of the shadow mask for a grid of `nx` × `ny` points.
pub fn size(nx: usize, ny: usize) -> (usize, usize) {
    ((nx - 1) / STEP + 1, (ny - 1) / STEP + 1)
}

/// 255 where the sun reaches the point, 0 where the relief hides it, every STEP points.
/// In between on a shadow's edge.
/// `heights` are rows south → north of `nx` points; `sun` points towards the sun
/// (x east, y up, z south).
pub fn mask(heights: &[f32], nx: usize, ny: usize, sun: Vec3) -> Vec<u8> {
    let (mx, my) = size(nx, ny);
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
        heights[j * STEP * nx + i * STEP]
    };
    let slope = db / da.abs();
    // How much lower the shadow line is one step further from the sun, in true metres.
    let drop = (STEP as f64 * CELL) as f32 * (1.0 + slope * slope).sqrt() * sun.y / flat;

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
