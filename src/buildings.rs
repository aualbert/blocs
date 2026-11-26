//! Buildings raised on the block: each Plan IGN building outline becomes a prism with a flat
//! roof at its true height (never exaggerated), in plain light grey.

use std::collections::HashMap;
use std::iter::once;

use earcut::Earcut;
use mvt_reader::{Reader, feature::Value};
use walkers::geo_types::{Coord, Geometry};

use crate::drape::from_mercator;
use crate::lambert93;
use crate::map::Frame;
use crate::relief::CELL;

/// A building corner, mirrored in terrain.wgsl.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    /// Position in block grid units: east, north.
    grid: [f32; 2],
    /// Ground altitude of the building (m).
    ground: f32,
    /// Roof height above the ground (m); 0 at a wall's foot, which reaches down to the terrain.
    roof: f32,
    /// Outward wall normal, east and north; zero on roofs.
    normal: [f32; 2],
}

#[derive(Default)]
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

/// Buildings of the Plan IGN `tiles` (MVT by `(x, y)` at `level`) lying wholly in `frame`.
pub fn mesh(level: u8, tiles: &HashMap<(u32, u32), Vec<u8>>, frame: &Frame) -> Mesh {
    let mut mesh = Mesh::default();
    let (nx, ny) = frame.points();
    let (nx, ny) = ((nx - 1) as f32, (ny - 1) as f32);
    let n = (1u32 << level) as f64;
    for (&(tx, ty), bytes) in tiles {
        let Some((extent, features)) = layer(bytes, "bati_surf") else {
            continue;
        };
        for feature in features {
            let props = feature.properties.unwrap_or_default();
            let number = |key| match props.get(key)? {
                Value::Int(v) | Value::SInt(v) => Some(*v as f32),
                Value::UInt(v) => Some(*v as f32),
                Value::Float(v) => Some(*v),
                Value::Double(v) => Some(*v as f32),
                _ => None,
            };
            let (Some(roof), Some(ground)) = (number("hauteur"), number("alti_sol")) else {
                continue;
            };
            if roof <= 0.0 {
                continue;
            }
            let polygons = match feature.geometry {
                Geometry::Polygon(p) => vec![p],
                Geometry::MultiPolygon(m) => m.0,
                _ => continue,
            };
            // Tile pixels → Lambert-93 → block grid units.
            let to_grid = |c: &Coord<f32>| {
                let (mx, my) = (
                    (tx as f64 + c.x as f64 / extent) / n,
                    (ty as f64 + c.y as f64 / extent) / n,
                );
                let (lon, lat) = from_mercator(mx, my);
                let (x, y) = lambert93::from_wgs84(lon, lat);
                [
                    ((x - frame.x0) / CELL) as f32,
                    ((y - frame.y0) / CELL) as f32,
                ]
            };
            for polygon in &polygons {
                // A piece wholly in the tile's margin is drawn by the neighbouring tile.
                let inside = |v: f32| (0.0..extent as f32).contains(&v);
                if !polygon
                    .exterior()
                    .coords()
                    .any(|c| inside(c.x) && inside(c.y))
                {
                    continue;
                }
                // Rings without their closing point.
                let rings: Vec<Vec<[f32; 2]>> = once(polygon.exterior())
                    .chain(polygon.interiors())
                    .map(|r| {
                        r.0[..r.0.len().saturating_sub(1)]
                            .iter()
                            .map(to_grid)
                            .collect()
                    })
                    .collect();
                // Buildings crossing the block's edge would overhang its walls.
                let on_block =
                    |p: &[f32; 2]| (0.0..=nx).contains(&p[0]) && (0.0..=ny).contains(&p[1]);
                if rings[0].len() >= 3 && rings.iter().flatten().all(on_block) {
                    mesh.add(&rings, ground, roof);
                }
            }
        }
    }
    mesh
}

impl Mesh {
    /// A prism: the roof over the rings (outline, then holes), and a wall along each edge.
    fn add(&mut self, rings: &[Vec<[f32; 2]>], ground: f32, roof: f32) {
        let vertex = |grid, roof, normal| Vertex {
            grid,
            ground,
            roof,
            normal,
        };
        let start = self.vertices.len() as u32;
        let points: Vec<[f32; 2]> = rings.concat();
        let mut holes = Vec::new();
        let mut len = 0;
        for ring in rings {
            holes.push(len);
            len += ring.len() as u32;
        }
        let mut triangles: Vec<u32> = Vec::new();
        Earcut::new().earcut(points.iter().copied(), &holes[1..], &mut triangles);
        self.vertices
            .extend(points.iter().map(|&p| vertex(p, roof, [0.0; 2])));
        self.indices.extend(triangles.iter().map(|t| start + t));

        for (k, ring) in rings.iter().enumerate() {
            // Right of each edge is outside for a counter-clockwise outline or a clockwise hole.
            let area: f32 = edges(ring).map(|(a, b)| a[0] * b[1] - b[0] * a[1]).sum();
            let out = if (area > 0.0) == (k == 0) { 1.0 } else { -1.0 };
            for (&a, &b) in edges(ring) {
                let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                let len = dx.hypot(dy);
                if len == 0.0 {
                    continue;
                }
                let normal = [out * dy / len, -out * dx / len];
                let s = self.vertices.len() as u32;
                self.vertices.extend([
                    vertex(a, roof, normal),
                    vertex(b, roof, normal),
                    vertex(a, 0.0, normal),
                    vertex(b, 0.0, normal),
                ]);
                self.indices.extend([s, s + 1, s + 2, s + 1, s + 3, s + 2]);
            }
        }
    }
}

/// Each point of a closed ring with the next one.
fn edges(ring: &[[f32; 2]]) -> impl Iterator<Item = (&[f32; 2], &[f32; 2])> {
    ring.iter().zip(ring.iter().cycle().skip(1))
}

/// A layer's extent and features in an MVT tile.
fn layer(bytes: &[u8], name: &str) -> Option<(f64, Vec<mvt_reader::feature::Feature>)> {
    let reader = Reader::new(bytes.to_vec()).ok()?;
    let layer = reader
        .get_layer_metadata()
        .ok()?
        .into_iter()
        .find(|l| l.name == name)?;
    Some((
        layer.extent as f64,
        reader.get_features(layer.layer_index).ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_square_makes_a_roof_and_four_outward_walls() {
        let mut mesh = Mesh::default();
        let square = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        mesh.add(&[square], 100.0, 6.0);
        assert_eq!(mesh.vertices.len(), 4 + 4 * 4);
        assert_eq!(mesh.indices.len(), 6 + 4 * 6);
        // The south wall (first edge, counter-clockwise outline) faces south.
        assert_eq!(mesh.vertices[4].normal, [0.0, -1.0]);
    }
}
