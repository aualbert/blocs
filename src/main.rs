use eframe::egui;
use walkers::{HttpOptions, HttpTiles, Map, MapMemory, TileId, lon_lat, sources};

/// Fond Plan IGN (Géoplateforme, WMTS en Web Mercator).
struct PlanIgn;

impl sources::TileUrls for PlanIgn {
    fn tile_url(&self, tile_id: TileId) -> String {
        format!(
            "https://data.geopf.fr/wmts?SERVICE=WMTS&REQUEST=GetTile&VERSION=1.0.0\
            &LAYER=GEOGRAPHICALGRIDSYSTEMS.PLANIGNV2&STYLE=normal&FORMAT=image/png\
            &TILEMATRIXSET=PM&TILEMATRIX={}&TILEROW={}&TILECOL={}",
            tile_id.zoom, tile_id.y, tile_id.x
        )
    }

    fn attribution(&self) -> sources::Attribution {
        sources::Attribution {
            text: "Source : IGN – Licence Ouverte",
            url: "https://geoservices.ign.fr/",
            logo_light: None,
            logo_dark: None,
        }
    }
}

struct Blocs {
    tiles: HttpTiles,
    map_memory: MapMemory,
    gpu: String,
}

impl Blocs {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let gpu = cc
            .wgpu_render_state
            .as_ref()
            .map(|rs| {
                let info: wgpu::AdapterInfo = rs.adapter.get_info();
                format!("{} ({:?})", info.name, info.backend)
            })
            .unwrap_or_else(|| "inconnu".to_owned());

        let mut map_memory = MapMemory::default();
        map_memory.set_zoom(6.0).expect("zoom valide");

        Self {
            // HttpOptions::default() envoie « blocs/<version> » comme User-Agent.
            tiles: HttpTiles::with_options(PlanIgn, HttpOptions::default(), cc.egui_ctx.clone()),
            map_memory,
            gpu,
        }
    }
}

impl eframe::App for Blocs {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.heading("Bonjour, Blocs !");
        ui.label(format!("Carte graphique : {}", self.gpu));
        ui.add(Map::new(
            Some(&mut self.tiles),
            &mut self.map_memory,
            lon_lat(2.35, 46.6),
        ));
    }
}

fn main() -> eframe::Result {
    eframe::run_native(
        "Blocs",
        eframe::NativeOptions::default(),
        Box::new(|cc| Ok(Box::new(Blocs::new(cc)))),
    )
}
