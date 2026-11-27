//! Map view: the block frame, drawn and edited with the mouse.

use std::f64::consts::PI;

use eframe::egui::{self, Color32, CursorIcon, Pos2, Stroke};
use walkers::{MapMemory, Plugin, Position, Projector, lon_lat};

use crate::lambert93;
use crate::relief::CELL;

/// Longest frame side (m).
pub const MAX_SIDE: f64 = 10_000.0;
/// Shortest frame side (m): anything smaller is taken for a click.
const MIN_SIDE: f64 = 100.0;

/// Block frame in Lambert-93 metres: a rectangle, sides multiple of 5 m, turned by
/// `angle` about its centre.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Frame {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
    /// Anticlockwise, in radians; 0 keeps it on the 5 m grid.
    pub angle: f64,
}

impl Frame {
    /// From a fixed corner towards `to`, each side capped at `MAX_SIDE`.
    pub fn from_corners(a: (f64, f64), to: (f64, f64)) -> Self {
        Self::spanning(a, to, 0.0)
    }

    /// From a fixed corner `a` towards `to`, its sides turned by `angle`.
    pub fn spanning(a: (f64, f64), to: (f64, f64), angle: f64) -> Self {
        let (ax, ay) = (snap(a.0), snap(a.1));
        let (s, c) = angle.sin_cos();
        let (dx, dy) = (to.0 - ax, to.1 - ay);
        let side = |d: f64| snap(d).clamp(-MAX_SIDE, MAX_SIDE);
        let (du, dv) = (side(dx * c + dy * s), side(dy * c - dx * s));
        let (cx, cy) = (ax + (du * c - dv * s) / 2.0, ay + (du * s + dv * c) / 2.0);
        let (hw, hh) = (du.abs() / 2.0, dv.abs() / 2.0);
        Self {
            x0: cx - hw,
            y0: cy - hh,
            x1: cx + hw,
            y1: cy + hh,
            angle,
        }
    }

    /// The largest square frame centred on `(x, y)`.
    pub fn square((x, y): (f64, f64)) -> Self {
        let h = MAX_SIDE / 2.0;
        Self::from_corners((x - h, y - h), (x + h, y + h))
    }

    pub fn size(&self) -> (f64, f64) {
        (self.x1 - self.x0, self.y1 - self.y0)
    }

    /// Grid points east and north.
    pub fn points(&self) -> (usize, usize) {
        let (w, h) = self.size();
        ((w / CELL) as usize + 1, (h / CELL) as usize + 1)
    }

    pub fn centre(&self) -> (f64, f64) {
        ((self.x0 + self.x1) / 2.0, (self.y0 + self.y1) / 2.0)
    }

    /// The Lambert-93 point `(u, v)` m from the south-west corner, along the sides.
    pub fn at(&self, u: f64, v: f64) -> (f64, f64) {
        let ((cx, cy), (w, h)) = (self.centre(), self.size());
        let (s, c) = self.angle.sin_cos();
        let (du, dv) = (u - w / 2.0, v - h / 2.0);
        (cx + du * c - dv * s, cy + du * s + dv * c)
    }

    /// A Lambert-93 point as `(u, v)`, the inverse of `at`.
    pub fn local(&self, (x, y): (f64, f64)) -> (f64, f64) {
        let ((cx, cy), (w, h)) = (self.centre(), self.size());
        let (s, c) = self.angle.sin_cos();
        let (dx, dy) = (x - cx, y - cy);
        (w / 2.0 + dx * c + dy * s, h / 2.0 + dy * c - dx * s)
    }

    /// South-west, south-east, north-east, north-west, as turned.
    pub fn corners(&self) -> [(f64, f64); 4] {
        let (w, h) = self.size();
        [
            self.at(0.0, 0.0),
            self.at(w, 0.0),
            self.at(w, h),
            self.at(0.0, h),
        ]
    }

    /// Lambert-93 `[x0, y0, x1, y1]` holding the corners.
    pub fn bounds(&self) -> [f64; 4] {
        let c = self.corners();
        let (xs, ys) = (c.map(|p| p.0), c.map(|p| p.1));
        let min = |v: [f64; 4]| v.into_iter().fold(f64::MAX, f64::min);
        let max = |v: [f64; 4]| v.into_iter().fold(f64::MIN, f64::max);
        [min(xs), min(ys), max(xs), max(ys)]
    }

    fn contains(&self, p: (f64, f64)) -> bool {
        let ((u, v), (w, h)) = (self.local(p), self.size());
        (0.0..=w).contains(&u) && (0.0..=h).contains(&v)
    }

    /// "8,2 × 10,0 km"
    pub fn label(&self) -> String {
        let (w, h) = self.size();
        format!("{} × {} km", km(w), km(h))
    }
}

/// Metres as kilometres, with the language's decimal mark.
pub fn km(m: f64) -> String {
    crate::i18n::decimal(format!("{:.1}", m / 1000.0))
}

/// Moves the map by `delta` screen pixels; `start` is the centre until the map is first moved.
pub fn pan(memory: &mut MapMemory, start: Position, delta: egui::Vec2) {
    // Web Mercator in pixels: the world is 256 × 2^zoom pixels wide.
    let world = 256.0 * 2f64.powf(memory.zoom());
    let c = memory.detached().unwrap_or(start);
    let x = (c.x() + 180.0) / 360.0 * world - delta.x as f64;
    let y = (1.0 - c.y().to_radians().tan().asinh() / PI) / 2.0 * world - delta.y as f64;
    let lat = (PI * (1.0 - 2.0 * y / world)).sinh().atan().to_degrees();
    memory.center_at(lon_lat(x / world * 360.0 - 180.0, lat));
}

/// The zoom at which the world just fills a map of `view` points: the world is
/// 256 × 2^zoom points wide, and as high.
pub fn widest_zoom(view: egui::Vec2) -> f64 {
    (view.x.max(view.y) as f64 / 256.0).log2()
}

/// Keeps the world filling a map of `view` points: no zooming out past it, no panning off its
/// edges. True if the map had to be moved back.
pub fn keep_in_world(memory: &mut MapMemory, start: Position, view: egui::Vec2) -> bool {
    let fit = widest_zoom(view);
    let zoomed = memory.zoom() < fit && memory.set_zoom(fit).is_ok();
    let world = 256.0 * 2f64.powf(memory.zoom());
    let c = memory.detached().unwrap_or(start);
    let x = (c.x() + 180.0) / 360.0 * world;
    let y = (1.0 - c.y().to_radians().tan().asinh() / PI) / 2.0 * world;
    let (hw, hh) = (view.x as f64 / 2.0, view.y as f64 / 2.0);
    // Centred where the world is no wider than the view (rounding can make it a hair less).
    let keep = |v: f64, h: f64| {
        if world <= 2.0 * h {
            world / 2.0
        } else {
            v.clamp(h, world - h)
        }
    };
    let (cx, cy) = (keep(x, hw), keep(y, hh));
    // A hundredth of a point: degrees there and back are not exact.
    if (cx - x).abs() < 0.01 && (cy - y).abs() < 0.01 {
        return zoomed;
    }
    let lat = (PI * (1.0 - 2.0 * cy / world)).sinh().atan().to_degrees();
    memory.center_at(lon_lat(cx / world * 360.0 - 180.0, lat));
    true
}

fn snap(v: f64) -> f64 {
    (v / CELL).round() * CELL
}

/// Mouse gesture in progress on the frame; Lambert-93 points. Drawing or resizing
/// keeps the frame from before, which comes back if the result is too small.
pub enum Edit {
    /// New frame from this corner.
    Draw((f64, f64), Option<Frame>),
    /// Corner drag; the opposite corner stays.
    Resize((f64, f64), Frame),
    /// Handle drag: the frame's angle minus the pointer's, about its centre.
    Rotate(f64),
    /// Whole frame drag from the last pointer position.
    Move((f64, f64)),
}

/// Draws the frame and edits it.
pub struct FrameTool<'a> {
    pub frame: &'a mut Option<Frame>,
    pub edit: &'a mut Option<Edit>,
    /// "Tracer le cadre" mode: the next drag creates a frame.
    pub drawing: &'a mut bool,
}

impl Plugin for FrameTool<'_> {
    fn run(
        self: Box<Self>,
        ui: &mut egui::Ui,
        response: &egui::Response,
        projector: &Projector,
        _: &MapMemory,
    ) {
        let to_l93 = |pos: Pos2| {
            let p = projector.unproject(pos.to_vec2());
            lambert93::from_wgs84(p.x(), p.y())
        };
        let to_screen = |(x, y): (f64, f64)| {
            let (lon, lat) = lambert93::to_wgs84(x, y);
            projector.project(lon_lat(lon, lat)).to_pos2()
        };
        let pointer = ui.input(|i| i.pointer.hover_pos());
        let (pressed, down) = ui.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_down()));
        let screen = self.frame.map(|f| f.corners().map(to_screen));
        // The turning handle, out from the middle of the north side.
        let handle = screen.map(|c| {
            let (centre, top) = (c[0].lerp(c[2], 0.5), c[2].lerp(c[3], 0.5));
            (top, top + (top - centre).normalized() * 24.0)
        });
        // Pointer angle about the frame's centre, in Lambert-93.
        let bearing = |p: Pos2, f: Frame| {
            let ((x, y), (cx, cy)) = (to_l93(p), f.centre());
            (y - cy).atan2(x - cx)
        };

        // What a press would grab: the handle, a corner, the frame itself, or nothing.
        let turn = pointer.is_some_and(|p| handle.is_some_and(|h| h.1.distance(p) < 10.0));
        let corner = pointer.and_then(|p| screen?.iter().position(|c| c.distance(p) < 10.0));
        let inside = pointer.is_some_and(|p| self.frame.is_some_and(|f| f.contains(to_l93(p))));

        if pressed
            && response.hovered()
            && let Some(p) = pointer.map(to_l93)
        {
            *self.edit = if *self.drawing {
                Some(Edit::Draw(p, *self.frame))
            } else if let (true, Some(f), Some(at)) = (turn, *self.frame, pointer) {
                Some(Edit::Rotate(f.angle - bearing(at, f)))
            } else if let (Some(k), Some(f)) = (corner, *self.frame) {
                Some(Edit::Resize(f.corners()[(k + 2) % 4], f))
            } else if inside {
                Some(Edit::Move(p))
            } else {
                None
            };
        }

        if let Some(edit) = self.edit.as_mut() {
            if !down {
                // A click or a tiny frame changes nothing: keep the previous frame (and the mode).
                let tiny = self
                    .frame
                    .is_none_or(|f| f.size().0.min(f.size().1) < MIN_SIDE);
                match edit {
                    Edit::Draw(_, before) if tiny => *self.frame = *before,
                    Edit::Resize(_, before) if tiny => *self.frame = Some(*before),
                    _ => *self.drawing = false,
                }
                *self.edit = None;
            } else if let Some(p) = pointer.map(to_l93) {
                match edit {
                    Edit::Draw(a, _) => *self.frame = Some(Frame::from_corners(*a, p)),
                    Edit::Resize(a, before) => {
                        *self.frame = Some(Frame::spanning(*a, p, before.angle))
                    }
                    Edit::Rotate(offset) => {
                        if let (Some(f), Some(at)) = (self.frame.as_mut(), pointer) {
                            // In whole degrees, so that it comes back to 0 exactly.
                            let degrees = (bearing(at, *f) + *offset).to_degrees().round();
                            f.angle = degrees.to_radians();
                        }
                    }
                    Edit::Move(last) => {
                        if let Some(f) = self.frame.as_mut() {
                            let (dx, dy) = (snap(p.0 - last.0), snap(p.1 - last.1));
                            (f.x0, f.x1, f.y0, f.y1) = (f.x0 + dx, f.x1 + dx, f.y0 + dy, f.y1 + dy);
                            (last.0, last.1) = (last.0 + dx, last.1 + dy);
                        }
                    }
                }
            }
        }

        if response.hovered() {
            let icon = match (&*self.edit, corner) {
                (Some(Edit::Draw(..)), _) => CursorIcon::Crosshair,
                (Some(Edit::Move(_)), _) => CursorIcon::Grabbing,
                (Some(Edit::Resize(..)), _) => CursorIcon::Move,
                (Some(Edit::Rotate(_)), _) => CursorIcon::Grabbing,
                _ if *self.drawing => CursorIcon::Crosshair,
                _ if turn => CursorIcon::Grab,
                (None, Some(_)) => CursorIcon::Move,
                _ if inside => CursorIcon::Grab,
                _ => CursorIcon::Default,
            };
            ui.ctx().set_cursor_icon(icon);
        }

        let Some(frame) = *self.frame else { return };
        // The frame is square in Lambert-93, so it shows slightly rotated on Web Mercator.
        let corners = frame.corners().map(to_screen);
        let red = Color32::from_rgb(220, 40, 40);
        let painter = ui.painter();
        painter.add(egui::Shape::convex_polygon(
            corners.to_vec(),
            Color32::from_rgba_unmultiplied(220, 40, 40, 25),
            Stroke::new(2.0, red),
        ));
        for c in corners {
            painter.rect(
                egui::Rect::from_center_size(c, egui::vec2(9.0, 9.0)),
                1.0,
                Color32::WHITE,
                Stroke::new(1.5, red),
                egui::StrokeKind::Middle,
            );
        }
        if let Some((top, knob)) = handle {
            painter.line_segment([top, knob], Stroke::new(1.5, red));
            painter.circle(knob, 5.0, Color32::WHITE, Stroke::new(1.5, red));
        }
        // The size under the south side, out of the handle's way.
        let (centre, bottom) = (
            corners[0].lerp(corners[2], 0.5),
            corners[0].lerp(corners[1], 0.5),
        );
        let below = bottom + (bottom - centre).normalized() * 16.0;
        let label = painter.layout_no_wrap(frame.label(), egui::FontId::proportional(14.0), red);
        let rect = egui::Align2::CENTER_CENTER.anchor_size(below, label.size());
        painter.rect_filled(rect.expand(3.0), 3.0, Color32::from_white_alpha(220));
        painter.galley(rect.min, label, red);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_is_capped_and_snapped() {
        let f = Frame::from_corners((900_002.0, 6_450_000.0), (880_000.0, 6_470_000.0));
        assert_eq!(f.size(), (MAX_SIDE, MAX_SIDE));
        assert_eq!((f.x1, f.y0), (900_000.0, 6_450_000.0));
        assert_eq!(f.points(), (2001, 2001));
    }

    #[test]
    fn turned_frame_keeps_its_corner_and_sides() {
        let a = (700_000.0, 6_600_000.0);
        let f = Frame::spanning(a, (703_000.0, 6_604_000.0), 30f64.to_radians());
        let sw = f.corners()[0];
        assert!((sw.0 - a.0).abs() < 1e-6 && (sw.1 - a.1).abs() < 1e-6);
        let (u, v) = f.local(f.at(120.0, 340.0));
        assert!((u - 120.0).abs() < 1e-6 && (v - 340.0).abs() < 1e-6);
    }

    #[test]
    fn the_world_fills_the_map() {
        let mut memory = MapMemory::default();
        memory.set_zoom(0.0).unwrap();
        let view = egui::vec2(1024.0, 512.0);
        assert!(keep_in_world(&mut memory, lon_lat(170.0, 80.0), view));
        assert!((memory.zoom() - 2.0).abs() < 1e-9); // 1024 points wide
        // Centred back to the middle of the world's width, a quarter of its height from the top.
        let c = memory.detached().unwrap();
        assert!(c.x().abs() < 1e-9 && c.y() > 60.0);
        assert!(!keep_in_world(&mut memory, c, view));
    }

    #[test]
    fn square_is_centred() {
        let f = Frame::square((700_000.0, 6_600_000.0));
        assert_eq!((f.x0, f.y1), (695_000.0, 6_605_000.0));
    }
}
