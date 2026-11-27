//! IGN's official Plan IGN style (MapLibre), read by walkers, split into categories the user
//! shows or hides.
//!
//! The style is shipped with the app (assets/plan_ign.json, the « standard » style from
//! data.geopf.fr), so the map draws offline. It uses MapLibre's legacy syntax, which walkers
//! does not read: it is rewritten into expressions when loaded.

use std::sync::OnceLock;

use eframe::egui;
use serde_json::{Value, json};
use walkers::Style;

/// Name of the style's source, which the Plan IGN tiles are given under.
pub const SOURCE: &str = "plan_ign";

/// Source Sans 3 (SIL Open Font License, assets/fonts/OFL.md), the Plan IGN's font.
pub const FONT: &[u8] = include_bytes!("../assets/fonts/SourceSans3-Regular.ttf");
/// Widest text halo kept: walkers draws a halo as 8 copies of the text this far away,
/// which show as ghost letters beyond about 2 points.
const MAX_HALO: f64 = 2.0;
/// Factor on the style's label sizes.
const TEXT_SCALE: f64 = 1.1;

/// Makes Source Sans the proportional font of `ctx`: map labels (walkers draws them all
/// in it) and the interface.
pub fn install_font(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "source-sans".into(),
        std::sync::Arc::new(egui::FontData::from_static(FONT)),
    );
    if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        family.insert(0, "source-sans".into());
    }
    ctx.set_fonts(fonts);
}

/// Groups of style rules, by the tile data they draw.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Category {
    Vegetation,
    Relief,
    Water,
    Buildings,
    Roads,
    Railways,
    Boundaries,
    Labels,
}

impl Category {
    /// In drawing order, bottom first, as IGN's style draws them.
    pub const ALL: [Category; 8] = [
        Category::Vegetation,
        Category::Relief,
        Category::Water,
        Category::Buildings,
        Category::Roads,
        Category::Railways,
        Category::Boundaries,
        Category::Labels,
    ];

    /// Stable identifier, for the block's layer slots.
    pub fn id(self) -> &'static str {
        match self {
            Category::Vegetation => "vegetation",
            Category::Relief => "relief",
            Category::Water => "eau",
            Category::Buildings => "bati",
            Category::Roads => "routes",
            Category::Railways => "ferre",
            Category::Boundaries => "limites",
            Category::Labels => "toponymes",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Category::Vegetation => "Végétation",
            Category::Relief => "Relief",
            Category::Water => "Eau",
            Category::Buildings => "Bâti",
            Category::Roads => "Routes",
            Category::Railways => "Voies ferrées",
            Category::Boundaries => "Limites",
            Category::Labels => "Toponymes",
        }
    }

    /// The category of a style rule; `None` for the opaque background.
    fn of(layer: &Value) -> Option<Category> {
        let id = layer["id"].as_str().unwrap_or_default();
        let data = layer["source-layer"].as_str().unwrap_or_default();
        Some(match data {
            // Labels first: some draw from other data, such as contour heights.
            _ if id.starts_with("toponyme") || data.starts_with("toponyme") => Category::Labels,
            "fond_opaque" => return None,
            _ if data.starts_with("hydro") || id.contains("hydro") => Category::Water,
            _ if data.starts_with("ocs") => Category::Vegetation,
            _ if data.starts_with("oro") => Category::Relief,
            _ if data.starts_with("routier") => Category::Roads,
            _ if data.starts_with("ferre") => Category::Railways,
            _ if data.starts_with("limite") || data.starts_with("parcellaire") => {
                Category::Boundaries
            }
            _ => Category::Buildings,
        })
    }
}

/// The rules of one category, or the opaque background alone for `None`.
pub fn plan_ign(category: Option<Category>) -> Style {
    let mut json = full().clone();
    if let Some(layers) = json["layers"].as_array_mut() {
        layers.retain(|layer| Category::of(layer) == category);
    }
    serde_json::from_value(json).expect("style Plan IGN lisible par walkers")
}

/// The whole style, read and rewritten once.
fn full() -> &'static Value {
    static STYLE: OnceLock<Value> = OnceLock::new();
    STYLE.get_or_init(|| {
        let mut json: Value = serde_json::from_str(include_str!("../assets/plan_ign.json"))
            .expect("style Plan IGN valide");
        for layer in json["layers"].as_array_mut().into_iter().flatten() {
            // walkers 0.66 reads `source_layer` on circle layers (a missing kebab-case rename).
            if layer["type"] == "circle" {
                layer["source_layer"] = layer["source-layer"].clone();
            }
            // Labels a little larger: at the style's sizes, the letters' pixels show on screen.
            if let Some(size) = layer["layout"].get_mut("text-size") {
                *size = json!(["*", TEXT_SCALE, size.take()]);
            }
            let paint = &mut layer["paint"];
            if let Some(width) = paint["text-halo-width"].as_f64() {
                paint["text-halo-width"] = json!(width.min(MAX_HALO));
            }
            // Road numbers sit on shields, icons walkers cannot draw: the shield's colour
            // becomes an opaque halo, so the light text stays readable.
            if layer["layout"].get("icon-text-fit").is_some() {
                layer["paint"]["text-halo-color"] = layer["paint"]["icon-color"].clone();
            }
        }
        modernise(&mut json);
        json
    })
}

/// Rewrites legacy functions and tokens into expressions, recursively.
fn modernise(value: &mut Value) {
    match value {
        // Zoom functions: {"stops": [[zoom, value], ...], "base": b}.
        Value::Object(map) if map.contains_key("stops") => {
            let stops = map["stops"].as_array().cloned().unwrap_or_default();
            let base = map.get("base").and_then(Value::as_f64).unwrap_or(1.0);
            *value = if stops.iter().all(|s| s[1].is_number()) {
                let mut e = vec![
                    json!("interpolate"),
                    json!(["exponential", base]),
                    json!(["zoom"]),
                ];
                for s in &stops {
                    e.extend([s[0].clone(), s[1].clone()]);
                }
                Value::Array(e)
            } else {
                // Colours: one per zoom range.
                let mut e = vec![json!("case")];
                for pair in stops.windows(2) {
                    e.extend([json!(["<", ["zoom"], pair[1][0]]), pair[0][1].clone()]);
                }
                e.push(stops.last().map_or(json!("#000000"), |s| s[1].clone()));
                Value::Array(e)
            };
        }
        // Label tokens: "{texte}".
        Value::String(s)
            if s.starts_with('{') && s.ends_with('}') && s.matches('{').count() == 1 =>
        {
            *value = json!(["get", &s[1..s.len() - 1]]);
        }
        Value::Object(map) => map.values_mut().for_each(modernise),
        Value::Array(items) => items.iter_mut().for_each(modernise),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_rule_has_one_category() {
        let total = full()["layers"].as_array().unwrap().len();
        let counts: Vec<usize> = Category::ALL
            .iter()
            .map(|c| plan_ign(Some(*c)).layers.len())
            .collect();
        assert!(counts.iter().all(|&n| n > 0), "{counts:?}");
        assert_eq!(
            counts.iter().sum::<usize>() + plan_ign(None).layers.len(),
            total
        );
    }

    #[test]
    fn legacy_syntax_is_rewritten() {
        let mut v = json!({"line-width": {"stops": [[10, 1], [14, 3]]}, "text-field": "{texte}"});
        modernise(&mut v);
        assert_eq!(
            v["line-width"],
            json!(["interpolate", ["exponential", 1.0], ["zoom"], 10, 1, 14, 3])
        );
        assert_eq!(v["text-field"], json!(["get", "texte"]));
    }
}
