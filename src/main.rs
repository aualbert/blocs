mod lambert93;
mod relief;
mod view3d;

use std::sync::mpsc::{Receiver, channel};
use std::time::Instant;

use eframe::egui;
use walkers::{
    HttpOptions, HttpTiles, Map, MapMemory, Plugin, Position, Projector, TileId, lon_lat, sources,
};

use relief::{Heightmap, Progress};
use view3d::View3d;

/// Block side (m) and points per side at 5 m.
const BLOCK: f64 = 10_000.0;
const BLOCK_POINTS: usize = (BLOCK / relief::CELL) as usize + 1;

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

/// Messages from background work to the UI.
enum Msg {
    Status(String),
    Built(Heightmap, String),
}

struct Blocs {
    tiles: HttpTiles,
    map_memory: MapMemory,
    /// Lambert-93 centre of the block frame.
    center: Option<(f64, f64)>,
    department: String,
    status: String,
    task: Option<Receiver<Msg>>,
    view: Option<View3d>,
    show_3d: bool,
}

impl Blocs {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut map_memory = MapMemory::default();
        map_memory.set_zoom(6.0).expect("zoom valide");
        let installed = relief::installed();
        Self {
            // HttpOptions::default() envoie « blocs/<version> » comme User-Agent.
            tiles: HttpTiles::with_options(PlanIgn, HttpOptions::default(), cc.egui_ctx.clone()),
            map_memory,
            center: None,
            department: "D038".into(),
            status: if installed.is_empty() {
                "Aucun relief téléchargé.".into()
            } else {
                format!("Relief disponible : {}", installed.join(", "))
            },
            task: None,
            view: None,
            show_3d: false,
        }
    }

    /// Runs `work` on a thread; it reports through the channel and wakes the UI.
    fn spawn(&mut self, ctx: &egui::Context, work: impl FnOnce(&dyn Fn(Msg)) + Send + 'static) {
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            work(&|msg| {
                let _ = tx.send(msg);
                ctx.request_repaint();
            })
        });
        self.task = Some(rx);
    }

    fn install(&mut self, ctx: &egui::Context) {
        let dep = self.department.trim().to_uppercase();
        self.spawn(ctx, move |send| {
            let start = Instant::now();
            let result = relief::install(&dep, |p| send(Msg::Status(progress_text(&p))));
            send(Msg::Status(match result {
                Ok(()) => format!("{dep} installé en {:.0} s.", start.elapsed().as_secs_f64()),
                Err(e) => format!("Échec : {e}"),
            }));
        });
    }

    fn build(&mut self, ctx: &egui::Context, (x, y): (f64, f64)) {
        self.spawn(ctx, move |send| {
            send(Msg::Status("Construction du bloc…".into()));
            match load_block(x, y) {
                Ok((map, text)) => send(Msg::Built(map, text)),
                Err(e) => send(Msg::Status(format!("Échec : {e}"))),
            }
        });
    }

    fn poll(&mut self, frame: &eframe::Frame) {
        let Some(rx) = &self.task else { return };
        for msg in rx.try_iter().collect::<Vec<_>>() {
            match msg {
                Msg::Status(text) => self.status = text,
                Msg::Built(map, text) => {
                    let start = Instant::now();
                    let rs = frame.wgpu_render_state().expect("rendu wgpu");
                    self.view = Some(View3d::new(rs, &map));
                    self.status = format!(
                        "{text} · envoyé au GPU en {:.2} s",
                        start.elapsed().as_secs_f64()
                    );
                    self.show_3d = true;
                }
            }
        }
    }
}

impl eframe::App for Blocs {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.poll(frame);

        egui::Panel::top("barre").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.show_3d, false, "Carte");
                let can_3d = self.view.is_some();
                ui.add_enabled_ui(can_3d, |ui| {
                    ui.selectable_value(&mut self.show_3d, true, "3D")
                })
                .response
                .on_disabled_hover_text("Cliquez sur la carte pour construire un bloc.");
                ui.separator();
                ui.label("Département");
                ui.add(egui::TextEdit::singleline(&mut self.department).desired_width(50.0));
                if ui.button("Télécharger le relief").clicked() {
                    self.install(ui.ctx());
                }
                ui.separator();
                ui.label(&self.status);
            });
        });

        if self.show_3d
            && let Some(view) = &mut self.view
        {
            egui::Panel::right("réglages").show(ui, |ui| {
                ui.label("Étirement du relief");
                ui.add(egui::Slider::new(&mut view.exaggeration, 1.0..=5.0).prefix("×"));
                ui.label("Profondeur du socle");
                ui.add(egui::Slider::new(&mut view.base_depth, 50.0..=3000.0).suffix(" m"));
            });
            view.ui(ui);
            return;
        }

        let mut clicked = None;
        ui.add(
            Map::new(
                Some(&mut self.tiles),
                &mut self.map_memory,
                lon_lat(2.35, 46.6),
            )
            .with_plugin(Frame {
                center: self.center,
                clicked: &mut clicked,
            }),
        );
        if let Some(center) = clicked {
            self.center = Some(center);
            self.build(ui.ctx(), center);
        }
    }
}

/// Draws the block frame on the map and reports clicks as Lambert-93 centres.
struct Frame<'a> {
    center: Option<(f64, f64)>,
    clicked: &'a mut Option<(f64, f64)>,
}

impl Plugin for Frame<'_> {
    fn run(
        self: Box<Self>,
        ui: &mut egui::Ui,
        response: &egui::Response,
        projector: &Projector,
        _: &MapMemory,
    ) {
        if response.clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            let p: Position = projector.unproject(pos.to_vec2());
            *self.clicked = Some(lambert93::from_wgs84(p.x(), p.y()));
        }
        let Some((x, y)) = self.center else { return };

        // The square is in Lambert-93, so it shows slightly rotated on Web Mercator.
        let h = BLOCK / 2.0;
        let corners = [(-h, -h), (h, -h), (h, h), (-h, h)].map(|(dx, dy)| {
            let (lon, lat) = lambert93::to_wgs84(x + dx, y + dy);
            projector.project(lon_lat(lon, lat)).to_pos2()
        });
        let stroke = egui::Stroke::new(2.0, egui::Color32::from_rgb(220, 40, 40));
        ui.painter()
            .add(egui::Shape::closed_line(corners.to_vec(), stroke));
    }
}

fn progress_text(p: &Progress) -> String {
    match p {
        Progress::Download { done, total } => format!(
            "Téléchargement {:.0} / {:.0} Mo",
            *done as f64 / 1e6,
            *total as f64 / 1e6
        ),
        Progress::Convert { done, total } => format!("Conversion {done} / {total} dalles"),
    }
}

/// Reads the block centred on Lambert-93 `(x, y)` and describes it with its read time.
fn load_block(x: f64, y: f64) -> relief::Result<(Heightmap, String)> {
    let start = Instant::now();
    let map = Heightmap::load(x - BLOCK / 2.0, y - BLOCK / 2.0, BLOCK_POINTS)?;
    let (lo, hi) = map.range();
    let text = format!(
        "Bloc {n} × {n} points lu en {:.2} s · altitudes {lo:.0}–{hi:.0} m · {} points comblés",
        start.elapsed().as_secs_f64(),
        map.holes,
        n = map.size,
    );
    Ok((map, text))
}

/// `blocs` opens the app. Headless commands, for measurements:
/// `blocs installer D038`, `blocs convertir <archive.7z> D038`, `blocs bloc <lon> <lat>`.
fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["bloc", lon, lat] => {
            let (x, y) =
                lambert93::from_wgs84(lon.parse().unwrap_or(0.0), lat.parse().unwrap_or(0.0));
            cli(|_| load_block(x, y).map(|(_, text)| println!("{text}")))
        }
        ["installer", dep] => cli(|p| relief::install(dep, p)),
        ["convertir", archive, dep] => {
            cli(|p| relief::convert(archive.as_ref(), &relief::department_dir(dep), p))
        }
        _ => {
            let options = eframe::NativeOptions {
                viewport: egui::ViewportBuilder::default().with_inner_size([1280.0, 800.0]),
                depth_buffer: 24, // matches view3d::DEPTH_FORMAT
                ..Default::default()
            };
            eframe::run_native(
                "Blocs",
                options,
                Box::new(|cc| Ok(Box::new(Blocs::new(cc)))),
            )
        }
    }
}

/// Runs a relief job in the terminal and reports its duration.
fn cli(job: impl FnOnce(&mut dyn FnMut(Progress)) -> relief::Result<()>) -> eframe::Result {
    let start = Instant::now();
    let mut last = String::new();
    let result = job(&mut |p| {
        let text = progress_text(&p);
        if text != last {
            eprint!("\r{text}   ");
            last = text;
        }
    });
    eprintln!();
    match result {
        Ok(()) => println!("Terminé en {:.1} s.", start.elapsed().as_secs_f64()),
        Err(e) => {
            eprintln!("Échec : {e}");
            std::process::exit(1);
        }
    }
    Ok(())
}
