//! PNG export: the rendered block on a transparent background, with source credits.

use std::path::Path;

use ab_glyph::{Font, FontRef, PxScale, ScaleFont, point};

use crate::relief::Result;

/// The sources of a block: always the relief, then the layers seen on it.
pub fn credits(plan_ign: bool, photo: bool) -> String {
    let mut sources = vec!["RGE ALTI® 5 m"];
    if plan_ign {
        sources.push("Plan IGN");
    }
    if photo {
        sources.push("BD ORTHO®");
    }
    format!(
        "Source : IGN – {} – Licence Ouverte 2.0",
        sources.join(", ")
    )
}

/// Writes `credits` in the bottom-right corner, then saves the image.
pub fn save_png(
    path: &Path,
    width: u32,
    height: u32,
    mut rgba: Vec<u8>,
    credits: &str,
) -> Result<()> {
    draw_text(&mut rgba, width, height, credits);
    image::save_buffer(path, &rgba, width, height, image::ColorType::Rgba8)?;
    Ok(())
}

/// Dark grey text, scaled with the image, blended over what is already there.
fn draw_text(rgba: &mut [u8], width: u32, height: u32, text: &str) {
    let font = FontRef::try_from_slice(epaint_default_fonts::UBUNTU_LIGHT).expect("police egui");
    let scaled = font.as_scaled(PxScale::from((width as f32 / 90.0).max(12.0)));
    let margin = scaled.height();
    let text_width: f32 = text
        .chars()
        .map(|c| scaled.h_advance(scaled.glyph_id(c)))
        .sum();
    let mut x = width as f32 - margin - text_width;
    let y = height as f32 - margin;

    for c in text.chars() {
        let glyph = scaled
            .glyph_id(c)
            .with_scale_and_position(scaled.scale(), point(x, y));
        x += scaled.h_advance(glyph.id);
        let Some(outline) = font.outline_glyph(glyph) else {
            continue;
        };
        let bounds = outline.px_bounds();
        outline.draw(|gx, gy, coverage| {
            let (px, py) = (
                bounds.min.x as i32 + gx as i32,
                bounds.min.y as i32 + gy as i32,
            );
            if px < 0 || py < 0 || px >= width as i32 || py >= height as i32 {
                return;
            }
            let k = 4 * (py as usize * width as usize + px as usize);
            blend(&mut rgba[k..k + 4], [50, 50, 50], coverage);
        });
    }
}

/// Source-over blending of a colour with opacity `a` on a straight-alpha pixel.
fn blend(px: &mut [u8], color: [u8; 3], a: f32) {
    let below = px[3] as f32 / 255.0;
    let out = a + below * (1.0 - a);
    if out <= 0.0 {
        return;
    }
    for c in 0..3 {
        px[c] = ((color[c] as f32 * a + px[c] as f32 * below * (1.0 - a)) / out).round() as u8;
    }
    px[3] = (out * 255.0).round() as u8;
}

#[cfg(test)]
mod tests {
    #[test]
    fn credits_cite_the_layers_seen() {
        assert_eq!(
            super::credits(true, true),
            "Source : IGN – RGE ALTI® 5 m, Plan IGN, BD ORTHO® – Licence Ouverte 2.0"
        );
        assert_eq!(
            super::credits(false, false),
            "Source : IGN – RGE ALTI® 5 m – Licence Ouverte 2.0"
        );
    }
}
