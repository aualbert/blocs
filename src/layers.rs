//! Where the map layers' tiles come from: the aerial photo (WMTS images) and the Plan IGN
//! vector tiles, which walkers draws with IGN's style, category by category (see `style`).

/// Below this zoom the map shows the whole Plan IGN, as kept on disk; above, the chosen layers.
pub const STACK_ZOOM: u8 = 11;

pub enum Source {
    /// WMTS layer, style and format, deepest zoom level served.
    Image {
        layer: &'static str,
        style: &'static str,
        format: &'static str,
        max_zoom: u8,
    },
    /// Plan IGN vector tiles.
    PlanIgn,
}

pub const PHOTO: Source = Source::Image {
    layer: "ORTHOIMAGERY.ORTHOPHOTOS",
    style: "normal",
    format: "image/jpeg",
    max_zoom: 19,
};

impl Source {
    pub fn max_zoom(&self) -> u8 {
        match self {
            Source::Image { max_zoom, .. } => *max_zoom,
            Source::PlanIgn => 18,
        }
    }

    pub fn extension(&self) -> &'static str {
        match self {
            Source::Image { format, .. } if format.ends_with("jpeg") => "jpg",
            Source::Image { .. } => "png",
            Source::PlanIgn => "pbf",
        }
    }

    pub fn url(&self, zoom: u8, x: u32, y: u32) -> String {
        match self {
            Source::Image {
                layer,
                style,
                format,
                ..
            } => format!(
                "https://data.geopf.fr/wmts?SERVICE=WMTS&REQUEST=GetTile&VERSION=1.0.0\
                &LAYER={layer}&STYLE={style}&FORMAT={format}\
                &TILEMATRIXSET=PM&TILEMATRIX={zoom}&TILEROW={y}&TILECOL={x}"
            ),
            Source::PlanIgn => {
                format!("https://data.geopf.fr/tms/1.0.0/PLAN.IGN/{zoom}/{x}/{y}.pbf")
            }
        }
    }
}
