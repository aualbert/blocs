// First, so that its `tr!` macro reaches every module.
#[macro_use]
mod i18n;

mod buildings;
mod drape;
mod export;
mod ign;
mod lambert93;
mod layers;
mod map;
mod mvt;
mod relief;
mod shadow;
mod style;
mod sun;
mod tiles;
mod view3d;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use eframe::egui;
use walkers::{Map, MapMemory, lon_lat};

use i18n::Lang;
use ign::Place;
use layers::Source;
use map::{Edit, Frame, FrameTool};
use relief::{CELL, Heightmap, Progress};
use style::Category;
use tiles::{PlanIgnTiles, Store, TileLayer};
use view3d::View3d;

/// Map centre at start-up.
const START: (f64, f64) = (2.35, 46.6);
/// Glide after a drag pan, gentler than walkers' own (2× the pointer speed, 0.2 s):
/// starting speed as a share of the pointer's, and decay time (s).
const GLIDE_SPEED: f32 = 1.0;
const GLIDE_TAU: f32 = 0.12;
/// Offline, waits between connection checks: the first, then doubled up to the last.
const RECHECK_FIRST: Duration = Duration::from_secs(10);
const RECHECK_MAX: Duration = Duration::from_secs(600);
/// How long a job's outcome stays shown, in seconds.
const NOTE_SECS: f32 = 10.0;
/// Export sizes offered: the image's longer side, in pixels.
const EXPORT_WIDTHS: [u32; 5] = [1280, 1920, 3840, 5760, 7680];

/// Messages from background work to the UI.
enum Msg {
    Status(String),
    /// Shown until closed.
    Warning(String),
    /// The connection is back after a start offline.
    Online,
    Places(String, Vec<Place>),
    /// For a frame: the départements to download.
    Departments(Frame, Vec<String>),
    /// Archive size of a département, in bytes.
    Size(String, u64),
    Progress(String, Progress),
    Installed(String),
    /// The download stopped: paused or failed.
    Stopped(String),
    /// A block read from disk, prepared for the GPU, or why it failed.
    Built(Frame, relief::Result<(Heightmap, view3d::Prepared)>),
    /// A layer's image over a block's frame, with its mipmaps.
    Draped(Frame, &'static str, relief::Result<Vec<image::RgbaImage>>),
    /// The 3D buildings of a block's frame.
    Buildings(Frame, buildings::Mesh),
    /// How the image export ended.
    Exported(String),
    /// What the downloaded data takes on disk.
    Storage(Storage),
}

/// Disk space, in bytes, shown in the options: relief per département, seeded and cached tiles.
struct Storage {
    reliefs: Vec<(u64, String)>,
    seed: u64,
    cache: u64,
}

/// What the main area shows.
#[derive(Clone, Copy, PartialEq)]
enum View {
    Map,
    Block,
    /// Map and block side by side.
    Split,
}

/// One layer of the stack, with its map tiles.
struct MapLayer {
    id: &'static str,
    name: &'static str,
    visible: bool,
    opacity: f32,
    store: Store,
    /// The Plan IGN category drawn; `None` for the photo.
    category: Option<Category>,
    tiles: MapTiles,
}

/// Toggle and opacity of a family of layers.
#[derive(Clone, Copy)]
struct Family {
    visible: bool,
    opacity: f32,
}

impl MapLayer {
    /// Its opacity if shown; a Plan IGN category also needs its family `plan` shown.
    /// Labels are always opaque: see-through, they only blur.
    fn shown(&self, plan: Family) -> Option<f32> {
        let family = match self.category {
            Some(Category::Labels) => Family {
                opacity: 1.0,
                ..plan
            },
            Some(_) => plan,
            None => Family {
                visible: true,
                opacity: 1.0,
            },
        };
        (family.visible && self.visible).then_some(family.opacity * self.opacity)
    }
}

/// The Plan IGN's opaque background on the block, under the stack when the photo is hidden.
const BACKGROUND: &str = "fond";
/// Pseudo-layer of the 3D buildings, always on, made from the Plan IGN tiles.
const BUILDINGS: &str = "bâtiments";

/// How a layer is drawn on the map.
enum MapTiles {
    Images(TileLayer),
    /// Vector tiles styled by walkers (the Plan IGN).
    Styled(walkers::StyledSources),
}

/// Départements to download for the frame last queried.
#[derive(Default)]
struct Lookup {
    frame: Option<Frame>,
    codes: Option<Vec<String>>,
}

/// Relief downloads, one département at a time.
struct Download {
    queue: Vec<String>,
    pause: Arc<AtomicBool>,
    running: bool,
    text: String,
    /// Tiles being converted after the download: too short to pause.
    converting: bool,
}

struct Blocs {
    ctx: egui::Context,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    /// How the last job ended, when, and whether it warns: shown for `NOTE_SECS`, or
    /// until closed for a warning.
    status: Option<(String, Instant, bool)>,

    /// The layer stack, top first: Plan IGN categories, then the photo.
    layers: Vec<MapLayer>,
    /// The Plan IGN categories together.
    vector: Family,
    /// The Plan IGN's opaque background, drawn when the photo is not.
    background: walkers::StyledSources,
    plan: Store,
    /// Its tiles, for the overview's download progress.
    plan_tiles: PlanIgnTiles,
    map_memory: MapMemory,
    /// Map speed after a drag pan, in points per second.
    glide: egui::Vec2,
    /// The map is being dragged.
    panning: bool,
    frame: Option<Frame>,
    edit: Option<Edit>,
    drawing: bool,
    search: String,
    search_typed: Option<Instant>,
    places: Vec<Place>,

    installed: Vec<String>,
    sizes: HashMap<String, u64>,
    in_frame: Lookup,
    download: Download,

    view3d: Option<View3d>,
    built: Option<Frame>,
    building: bool,
    /// Frame last built, or tried: not built again until its relief changes.
    tried: Option<Frame>,
    /// Layers whose image is on the block, and those being prepared.
    draped: HashSet<&'static str>,
    draping: HashSet<&'static str>,
    view: View,
    /// The map's share of the width, side by side.
    split: f32,
    export_width: u32,
    /// An export is being saved.
    exporting: bool,
    /// `None` while measuring.
    storage: Option<Storage>,
    /// Cache limit set in the options, in MB.
    cache_mb: u32,
}

impl Blocs {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut map_memory = MapMemory::default();
        map_memory.set_zoom(6.0).expect("zoom valide");
        // Ctrl + wheel scrolls vertically (map panning) instead of zooming.
        cc.egui_ctx.options_mut(|o| {
            o.input_options.zoom_modifier = egui::Modifiers::NONE;
            o.input_options.vertical_scroll_modifier = egui::Modifiers::CTRL;
        });
        let (tx, rx) = channel();
        std::thread::spawn(tiles::trim_cache);
        // walkers draws vector tiles with its own wgpu pipeline.
        walkers::install_renderer(cc.wgpu_render_state.as_ref());
        style::install_font(&cc.egui_ctx);
        // All text in the strong colour (white on dark), the default grey being hard to read.
        cc.egui_ctx.all_styles_mut(|s| {
            s.visuals.override_text_color = Some(s.visuals.strong_text_color())
        });
        // The Plan IGN categories, top first, all drawn from one download; the photo below.
        let ctx = cc.egui_ctx.clone();
        let plan = Store::new("planign", Source::PlanIgn);
        let plan_tiles = PlanIgnTiles::new(ctx.clone(), plan.clone());
        let styled = |category| {
            walkers::StyledSources::new(style::plan_ign(category), ctx.clone())
                .with_tiled_source(style::SOURCE, plan_tiles.clone())
        };
        let mut layers: Vec<MapLayer> = (Category::ALL.iter().rev())
            .map(|&c| MapLayer {
                id: c.id(),
                name: c.name(),
                visible: true,
                opacity: 1.0,
                store: plan.clone(),
                category: Some(c),
                tiles: MapTiles::Styled(styled(Some(c))),
            })
            .collect();
        let photo = Store::new("photo", layers::PHOTO);
        layers.push(MapLayer {
            id: "photo",
            name: "Photo aérienne",
            visible: false,
            opacity: 1.0,
            tiles: MapTiles::Images(TileLayer::new(ctx.clone(), photo.clone())),
            store: photo,
            category: None,
        });
        let background = styled(None);
        let app = Self {
            ctx: cc.egui_ctx.clone(),
            tx,
            rx,
            status: None,
            layers,
            vector: Family {
                visible: true,
                opacity: 1.0,
            },
            background,
            plan,
            plan_tiles,
            map_memory,
            glide: egui::Vec2::ZERO,
            panning: false,
            frame: None,
            edit: None,
            drawing: false,
            search: String::new(),
            search_typed: None,
            places: Vec::new(),
            installed: relief::installed(),
            sizes: HashMap::new(),
            in_frame: Lookup::default(),
            download: Download {
                queue: Vec::new(),
                pause: Arc::default(),
                running: false,
                text: String::new(),
                converting: false,
            },
            view3d: None,
            built: None,
            building: false,
            tried: None,
            draped: HashSet::new(),
            draping: HashSet::new(),
            view: View::Map,
            split: 0.5,
            export_width: EXPORT_WIDTHS[1],
            exporting: false,
            storage: None,
            cache_mb: (tiles::cache_limit() / 1_000_000) as u32,
        };
        app.spawn(|send| {
            if ign::online() {
                return;
            }
            let text =
                tr!("Pas de connexion à Internet : seules les données en cache sont disponibles.");
            send(Msg::Warning(text.into()));
            // Offline, look again after 10 s, 20 s, 40 s…, until the connection is back.
            let mut wait = RECHECK_FIRST;
            loop {
                std::thread::sleep(wait);
                if ign::online() {
                    return send(Msg::Online);
                }
                wait = (wait * 2).min(RECHECK_MAX);
            }
        });
        app
    }

    /// Shows `text` in the activity stack for `NOTE_SECS`.
    fn note(&mut self, text: String) {
        self.status = Some((text, Instant::now(), false));
    }

    /// Runs `work` on a thread; its messages wake the UI.
    fn spawn(&self, work: impl FnOnce(&dyn Fn(Msg)) + Send + 'static) {
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            work(&|msg| {
                let _ = tx.send(msg);
                ctx.request_repaint();
            })
        });
    }

    fn poll(&mut self, frame: &eframe::Frame) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Status(text) => self.note(text),
                Msg::Warning(text) => self.status = Some((text, Instant::now(), true)),
                Msg::Online => {
                    self.note(tr!("Connexion rétablie.").into());
                    // Ask again what offline left out: the frame's départements, the block's layers.
                    self.in_frame = Lookup::default();
                    self.draped.clear();
                }
                Msg::Storage(storage) => {
                    // A relief was deleted: look the frame's départements up again.
                    let deps: Vec<_> = storage.reliefs.iter().map(|r| r.1.clone()).collect();
                    if deps != self.installed {
                        self.installed = deps;
                        self.in_frame = Lookup::default();
                    }
                    self.storage = Some(storage);
                }
                Msg::Exported(note) => {
                    self.exporting = false;
                    self.note(note);
                }
                Msg::Places(query, places) => {
                    if query == self.search.trim() {
                        self.places = places;
                    }
                }
                Msg::Departments(frame, codes) => {
                    if self.in_frame.frame == Some(frame) {
                        self.in_frame.codes = Some(codes);
                    }
                }
                Msg::Size(code, size) => {
                    self.sizes.insert(code, size);
                }
                Msg::Progress(dep, p) => {
                    let name = ign::department_name(&dep);
                    self.download.converting = matches!(p, Progress::Convert { .. });
                    self.download.text = match p {
                        Progress::Download { done, total } => tr!(
                            "{} : téléchargement {} / {} Mo",
                            name,
                            done / 1_000_000,
                            total / 1_000_000
                        ),
                        Progress::Convert { done, total } => {
                            tr!("{} : préparation {} / {} dalles", name, done, total)
                        }
                    };
                }
                Msg::Installed(dep) => {
                    self.note(tr!("Relief installé : {}.", ign::department_name(&dep)));
                    self.download.queue.retain(|d| *d != dep);
                    self.download.running = false;
                    self.installed = relief::installed();
                    // Built again only if the new relief is the frame's.
                    if self
                        .in_frame
                        .codes
                        .as_ref()
                        .is_some_and(|c| c.contains(&dep))
                    {
                        self.tried = None;
                    }
                    // The options menu, if open, lists it.
                    if self.storage.is_some() {
                        self.measure(|| ());
                    }
                    self.next_download();
                }
                Msg::Stopped(error) => {
                    self.download.running = false;
                    if !self.download.pause.load(Ordering::Relaxed) {
                        self.download.pause.store(true, Ordering::Relaxed);
                        self.note(tr!("Téléchargement interrompu : {}", error));
                    }
                }
                Msg::Draped(built, id, result) => {
                    // A layer for an older block is dropped, leaving the current block's job be.
                    if self.built != Some(built) {
                        continue;
                    }
                    self.draping.remove(id);
                    match result {
                        Ok(levels) => {
                            if let (Some(view), Some(rs)) =
                                (&self.view3d, frame.wgpu_render_state())
                            {
                                view.set_layer(rs, id, &levels);
                                self.draped.insert(id);
                            }
                        }
                        Err(e) => {
                            // Not retried for this block: moving the frame or reopening asks again.
                            self.draped.insert(id);
                            self.note(tr!("Calque indisponible sur le bloc : {}", e));
                        }
                    }
                }
                Msg::Buildings(built, mesh) => {
                    if self.built != Some(built) {
                        continue;
                    }
                    self.draping.remove(BUILDINGS);
                    if let (Some(view), Some(rs)) = (&self.view3d, frame.wgpu_render_state()) {
                        view.set_buildings(rs, &mesh);
                        self.draped.insert(BUILDINGS);
                    }
                }
                Msg::Built(built, result) => {
                    self.building = false;
                    match result {
                        Ok((map, prepared)) => {
                            let rs = frame.wgpu_render_state().expect("rendu wgpu");
                            if let Some(old) = &mut self.view3d {
                                old.release(rs);
                            }
                            let (x, y) = built.centre();
                            let latitude = lambert93::to_wgs84(x, y).1;
                            self.view3d =
                                Some(View3d::new(rs, &map, &prepared, latitude, built.angle));
                            // The new block starts without layer images.
                            self.draped.clear();
                            self.draping.clear();
                            self.built = Some(built);
                            self.view = View::Split;
                        }
                        Err(e) => self.note(tr!("Échec : {}", e)),
                    }
                }
            }
        }
    }

    /// Finds the départements to download for `frame`, then their archive sizes.
    fn look_up(&self, frame: Frame) {
        let installed = self.installed.clone();
        let known: HashSet<String> = self.sizes.keys().cloned().collect();
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        self.spawn(move |send| {
            // Offline, nothing to offer: the block is built from what is installed.
            if ign::reachable().is_err() {
                return send(Msg::Departments(frame, Vec::new()));
            }
            let fail = |text: String| {
                send(Msg::Status(text));
                send(Msg::Departments(frame, Vec::new()));
            };
            let http = match ign::shared() {
                Ok(http) => http,
                Err(e) => return fail(tr!("Réseau indisponible : {}", e)),
            };
            // Which départements touch each dalle of the frame, asked in parallel.
            let dalles = relief::dalles(frame.bounds());
            let answers: Vec<_> = std::thread::scope(|s| {
                let jobs: Vec<_> = dalles
                    .iter()
                    .map(|&d| {
                        let http = &http;
                        s.spawn(move || ign::departments(http, d))
                    })
                    .collect();
                jobs.into_iter().map(|j| j.join().unwrap()).collect()
            });
            let answers = match answers.into_iter().collect::<relief::Result<Vec<_>>>() {
                Ok(answers) => answers,
                Err(e) => return fail(tr!("Départements introuvables : {}", e)),
            };
            let needed = relief::cover(&answers, &installed);
            send(Msg::Departments(frame, needed.clone()));

            // Sizes arrive one by one, so the offer shows without waiting for them.
            for dep in needed.into_iter().filter(|d| !known.contains(d)) {
                let (tx, ctx) = (tx.clone(), ctx.clone());
                std::thread::spawn(move || {
                    if let Ok(size) = relief::archive_size(&dep) {
                        let _ = tx.send(Msg::Size(dep, size));
                        ctx.request_repaint();
                    }
                });
            }
        });
    }

    /// Looks up the frame's départements once it stops moving.
    fn refresh_lookup(&mut self) {
        if let Some(frame) = self.frame
            && self.edit.is_none()
            && self.in_frame.frame != Some(frame)
        {
            self.in_frame = Lookup {
                frame: Some(frame),
                codes: None,
            };
            self.look_up(frame);
        }
    }

    /// Départements of the frame still to download.
    fn proposal(&self) -> Vec<String> {
        let codes = self.in_frame.codes.iter().flatten();
        codes
            .filter(|c| !self.installed.contains(c) && !self.download.queue.contains(c))
            .cloned()
            .collect()
    }

    fn next_download(&mut self) {
        let Some(dep) = self.download.queue.first().cloned() else {
            return;
        };
        if self.download.running {
            return;
        }
        self.download.running = true;
        self.download.pause.store(false, Ordering::Relaxed);
        let pause = self.download.pause.clone();
        self.spawn(move |send| {
            let result = relief::install(&dep, &pause, |p| send(Msg::Progress(dep.clone(), p)));
            send(match result {
                Ok(()) => Msg::Installed(dep),
                Err(e) => Msg::Stopped(e.to_string()),
            });
        });
    }

    /// The frame is drawn, and all its départements are known and installed.
    fn relief_ready(&self) -> bool {
        let codes = self.in_frame.codes.as_ref();
        self.frame.is_some() && codes.is_some_and(|c| c.iter().all(|c| self.installed.contains(c)))
    }

    /// Builds the frame's block as soon as it is drawn and its relief installed.
    fn build_if_ready(&mut self) {
        let Some(frame) = self.frame else { return };
        let ready = self.edit.is_none() && self.relief_ready();
        if ready && !self.building && self.tried != Some(frame) {
            self.tried = Some(frame);
            self.building = true;
            // The description is for the headless command only.
            self.spawn(move |send| {
                let block = load_block(&frame).map(|(map, _)| {
                    let prepared = view3d::prepare(&map);
                    (map, prepared)
                });
                send(Msg::Built(frame, block))
            });
        }
    }

    /// The sources on screen: the relief with a block, then the layers drawn.
    fn credits(&self) -> String {
        let block = self.view != View::Map && self.view3d.is_some();
        let overview =
            self.view != View::Block && self.map_memory.zoom() < layers::STACK_ZOOM as f64;
        let photo = self.photo_visible();
        let category =
            (self.layers.iter()).any(|l| l.category.is_some() && l.shown(self.vector).is_some());
        // The Plan IGN's background is drawn whenever the photo is hidden.
        export::credits(
            block,
            overview || !photo || category,
            photo,
            &export::today(),
        )
    }

    /// Prepares the block image of each visible layer that has none yet. The Plan IGN
    /// categories share one job, which fetches their tiles once.
    fn drape_visible(&mut self, rs: Option<&eframe::egui_wgpu::RenderState>) {
        let (Some(frame), Some(rs)) = (self.built, rs) else {
            return;
        };
        let background = (!self.photo_visible()).then_some((BACKGROUND, None));
        // Bottom first, so the block fills in from its background up to the labels.
        let visible = (self.layers.iter().rev()).filter(|l| l.shown(self.vector).is_some());
        let wanted: Vec<_> = (background.into_iter())
            .chain([(BUILDINGS, None)])
            .chain(visible.map(|l| (l.id, l.category)))
            .filter(|(id, _)| !self.draped.contains(id) && !self.draping.contains(id))
            .collect();
        let mut plan = Vec::new();
        for (id, category) in wanted {
            self.draping.insert(id);
            if id == "photo" {
                let store = self
                    .layers
                    .iter()
                    .find(|l| l.id == id)
                    .unwrap()
                    .store
                    .clone();
                self.spawn(move |send| {
                    let image = drape::drape(&store, &frame).map(view3d::mipmaps);
                    send(Msg::Draped(frame, id, image))
                });
            } else {
                plan.push((id, category));
            }
        }
        if plan.is_empty() {
            return;
        }
        let (store, rs) = (self.plan.clone(), rs.clone());
        self.spawn(move |send| match drape::PlanIgn::fetch(&store, &frame) {
            Ok(tiles) => {
                for (id, category) in plan {
                    if id == BUILDINGS {
                        match tiles.buildings(&store) {
                            Ok(mesh) => send(Msg::Buildings(frame, mesh)),
                            Err(e) => send(Msg::Draped(frame, id, Err(e))),
                        }
                        continue;
                    }
                    let image = tiles.draw(&style::plan_ign(category), &rs);
                    let image = image.map(view3d::mipmaps);
                    send(Msg::Draped(frame, id, image));
                }
            }
            Err(e) => {
                for (id, _) in plan {
                    send(Msg::Draped(frame, id, Err(e.to_string().into())));
                }
            }
        });
    }

    fn photo_visible(&self) -> bool {
        self.layers.iter().any(|l| l.id == "photo" && l.visible)
    }

    /// The visible layers, bottom first, with their opacity.
    fn stack(&self) -> Vec<(&'static str, f32)> {
        let background = (!self.photo_visible()).then_some((BACKGROUND, 1.0));
        let shown = (self.layers.iter().rev()).filter_map(|l| Some((l.id, l.shown(self.vector)?)));
        background.into_iter().chain(shown).collect()
    }

    fn export(&mut self, frame: &eframe::Frame) {
        let (Some(view), Some(rs)) = (&self.view3d, frame.wgpu_render_state()) else {
            return;
        };
        // Parented to the window, so that the dialog never opens behind it.
        let Some(path) = rfd::FileDialog::new()
            .set_parent(frame)
            .set_file_name("bloc.png")
            .add_filter(tr!("Image PNG"), &["png"])
            .save_file()
        else {
            return;
        };
        let path = path.with_extension("png");
        let (w, h, rgba) = view.render_image(rs, self.export_width);
        // Cite the layers seen on the image.
        let shown: Vec<_> = (self.stack().into_iter())
            .filter(|&(id, opacity)| opacity > 0.0 && self.draped.contains(id))
            .map(|(id, _)| id)
            .collect();
        let credits = export::credits(
            true,
            shown.iter().any(|&id| id != "photo"),
            shown.contains(&"photo"),
            &export::today(),
        );
        self.exporting = true;
        self.spawn(move |send| {
            send(Msg::Exported(
                match export::save_png(&path, w, h, rgba, &credits) {
                    Ok(()) => tr!("Image enregistrée : {}", path.display()),
                    Err(e) => tr!("Échec de l'export : {}", e),
                },
            ))
        });
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let search = ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text(tr!("Rechercher un lieu…"))
                    .desired_width(220.0),
            );
            if search.changed() {
                self.search_typed = Some(Instant::now());
                self.places.clear();
            }
            if search.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                self.go_to(0);
            }
            self.search_results(ui, &search);
            // Greyed out without the map, so that the bar keeps its layout.
            ui.add_enabled_ui(self.view != View::Block, |ui| {
                // A button, grey like the others, highlighted while drawing.
                let button = egui::Button::new(tr!("Tracer le cadre")).selected(self.drawing);
                let tip =
                    tr!("Glissez sur la carte pour tracer le cadre (10 km au plus par côté).");
                if ui.add(button).on_hover_text(tip).clicked() {
                    self.drawing = !self.drawing;
                }
            });
            self.options(ui);
            egui::containers::menu::MenuButton::new(tr!("Aide")).ui(ui, help);
        });
    }

    /// One view left by the separator: switch it, or show both again.
    fn view_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let (other, text) = match self.view {
                View::Map => (View::Block, tr!("Vue 3D")),
                _ => (View::Map, tr!("Carte")),
            };
            if ui.button(text).clicked() {
                self.view = other;
            }
            if ui.button(tr!("Vue double")).clicked() {
                self.view = View::Split;
            }
        });
    }

    /// Autocompletion list under the search box.
    fn search_results(&mut self, ui: &mut egui::Ui, search: &egui::Response) {
        if let Some(t) = self.search_typed {
            let query = self.search.trim().to_owned();
            if t.elapsed() < Duration::from_millis(300) {
                ui.ctx().request_repaint_after(Duration::from_millis(300));
            } else {
                self.search_typed = None;
                if query.chars().count() >= 3 {
                    self.spawn(move |send| {
                        if let Ok(places) = ign::search(&query) {
                            send(Msg::Places(query, places));
                        }
                    });
                }
            }
        }
        if self.places.is_empty() {
            return;
        }
        let mut chosen = None;
        let area = egui::Area::new(egui::Id::unique("résultats"))
            .order(egui::Order::Foreground)
            .fixed_pos(search.rect.left_bottom())
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_width(search.rect.width());
                    for (k, place) in self.places.iter().enumerate() {
                        if ui.selectable_label(false, &place.label).clicked() {
                            chosen = Some(k);
                        }
                    }
                });
            });
        if let Some(k) = chosen {
            self.go_to(k);
        } else {
            // Escape, or a click elsewhere, closes the list.
            let (escape, pressed) =
                ui.input(|i| (i.key_pressed(egui::Key::Escape), i.pointer.any_pressed()));
            let outside = !area.response.contains_pointer() && !search.contains_pointer();
            if escape || (pressed && outside) {
                self.places.clear();
            }
        }
    }

    /// Centres the map on the `k`-th search result.
    fn go_to(&mut self, k: usize) {
        let Some(place) = self.places.get(k) else {
            return;
        };
        self.map_memory.center_at(lon_lat(place.lon, place.lat));
        // About 5 km across on a laptop screen.
        let _ = self.map_memory.set_zoom(15.0);
        self.search = place.label.clone();
        self.places.clear();
    }

    /// The relief download in progress, with its pause button.
    fn download_progress(&mut self, ui: &mut egui::Ui) {
        let paused = !self.download.running;
        // No pause while converting; a failed conversion can still be resumed.
        let pausable = paused || !self.download.converting;
        if pausable
            && ui
                .button(if paused {
                    tr!("Reprendre")
                } else {
                    tr!("Pause")
                })
                .clicked()
        {
            if paused {
                self.next_download();
            } else {
                self.download.pause.store(true, Ordering::Relaxed);
            }
        }
        let mut text = self.download.text.clone();
        if self.download.queue.len() > 1 {
            text += &tr!(" · {} en attente", self.download.queue.len() - 1);
        }
        if paused {
            text += tr!(" · en pause");
        }
        ui.label(text);
    }

    /// Everything under way, then how the last job ended: one line each, stacked.
    fn activity(&mut self, ui: &mut egui::Ui) {
        let busy = |ui: &mut egui::Ui, text: &str| {
            line(ui, |ui| {
                ui.spinner();
                ui.label(text);
            })
        };
        if !self.download.queue.is_empty() {
            line(ui, |ui| self.download_progress(ui));
        }
        let proposal = self.proposal();
        if !proposal.is_empty() {
            line(ui, |ui| {
                let names: Vec<_> = proposal.iter().map(|d| ign::department_name(d)).collect();
                ui.label(tr!("Relief à télécharger : {}", names.join(", ")));
                // The total shows once every size is known.
                let sizes: Option<Vec<u64>> = proposal
                    .iter()
                    .map(|d| self.sizes.get(d).copied())
                    .collect();
                let label = match sizes {
                    Some(s) => tr!("Télécharger ({})", megabytes(s.iter().sum())),
                    None => tr!("Télécharger").into(),
                };
                if ui.button(label).clicked() {
                    self.download.queue.extend(proposal);
                    self.next_download();
                }
            });
        }
        if self.in_frame.frame.is_some() && self.in_frame.codes.is_none() {
            busy(ui, tr!("Recherche des départements du cadre…"));
        }
        if self.building {
            busy(ui, tr!("Construction du bloc…"));
        }
        if !self.draping.is_empty() {
            let names: Vec<_> = (self.layers.iter())
                .map(|l| (l.id, l.name))
                .chain([(BACKGROUND, "Fond du Plan IGN"), (BUILDINGS, "Bâtiments")])
                .filter(|(id, _)| self.draping.contains(id))
                .map(|(_, name)| i18n::tr(name))
                .collect();
            busy(ui, &tr!("Préparation : {}", names.join(", ")));
        }
        if self.exporting {
            busy(ui, tr!("Enregistrement de l'image…"));
        }
        if let Some((done, total)) = self.plan_tiles.seed_progress() {
            line(ui, |ui| {
                ui.label(tr!("Plan IGN de base : {} / {} tuiles", done, total));
            });
            // Its tiles arrive without waking the UI.
            ui.ctx().request_repaint_after_secs(1.0);
        }
        if let Some((text, at, warn)) = self.status.clone() {
            // Fades out over its last second, or goes at once on "x"; a warning waits for "x".
            let left = match warn {
                true => NOTE_SECS,
                false => NOTE_SECS - at.elapsed().as_secs_f32(),
            };
            let text = match warn {
                true => egui::RichText::new(text).color(ui.visuals().warn_fg_color),
                false => text.into(),
            };
            let mut close = left <= 0.0;
            ui.scope(|ui| {
                ui.set_opacity(left.clamp(0.0, 1.0));
                line(ui, |ui| {
                    ui.label(text);
                    close |= ui.small_button("x").clicked();
                });
            });
            ui.ctx().request_repaint_after_secs((left - 1.0).max(0.0));
            if close {
                self.status = None;
            }
        }
    }

    /// The options menu: downloaded data, its disk space, and what to delete.
    fn options(&mut self, ui: &mut egui::Ui) {
        use egui::containers::menu::{MenuButton, MenuConfig};
        // Stays open while deleting or sliding; a click elsewhere closes it.
        let config =
            MenuConfig::new().close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
        let (button, _) = MenuButton::new(tr!("Options")).config(config).ui(ui, |ui| {
            self.options_menu(ui);
            ui.separator();
            language(ui);
        });
        if button.clicked() {
            self.measure(|| ());
        }
    }

    /// Runs `job` on a thread, then measures the disk space again.
    fn measure(&mut self, job: impl FnOnce() + Send + 'static) {
        self.storage = None;
        self.spawn(move |send| {
            job();
            let reliefs = relief::installed().into_iter();
            send(Msg::Storage(Storage {
                reliefs: reliefs.map(|d| (relief::disk_use(&d), d)).collect(),
                seed: tiles::disk_use(true),
                cache: tiles::disk_use(false),
            }))
        });
    }

    fn options_menu(&mut self, ui: &mut egui::Ui) {
        // A fixed width, which the département list fills, its scroll bar on the edge.
        ui.set_width(270.0);
        let Some(storage) = &self.storage else {
            ui.spinner();
            return;
        };
        let (seed, cache) = (storage.seed, storage.cache);
        // What a click asks for, run after the menu is drawn.
        let mut job: Option<Box<dyn FnOnce() + Send>> = None;

        ui.strong(tr!("Relief téléchargé"));
        if storage.reliefs.is_empty() {
            ui.label(tr!("Aucun département."));
        }
        // Past 10 départements, the list scrolls rather than lengthening the menu.
        let row = ui.spacing().interact_size.y + ui.spacing().item_spacing.y;
        egui::ScrollArea::vertical()
            .auto_shrink([false, true])
            .max_height(10.0 * row)
            .show(ui, |ui| {
                for (size, dep) in &storage.reliefs {
                    ui.horizontal(|ui| {
                        // Not while it downloads.
                        let idle = !self.download.queue.contains(dep);
                        if confirmed(ui, dep, idle) {
                            let dir = relief::department_dir(dep);
                            job = Some(Box::new(move || drop(std::fs::remove_dir_all(dir))));
                        }
                        ui.label(format!(
                            "{} · {}",
                            ign::department_name(dep),
                            megabytes(*size)
                        ));
                    });
                }
            });
        ui.separator();

        ui.strong(tr!("Plan IGN de base"));
        ui.horizontal(|ui| {
            if confirmed(ui, "plan", true) {
                job = Some(Box::new(|| tiles::delete(true)));
            }
            ui.label(tr!("{} · téléchargé au lancement", megabytes(seed)));
        });
        ui.separator();

        ui.strong(tr!("Cache des tuiles"));
        ui.horizontal(|ui| {
            if confirmed(ui, "cache", true) {
                job = Some(Box::new(|| tiles::delete(false)));
            }
            ui.label(megabytes(cache));
        });
        let slider = ui.add(
            egui::Slider::new(&mut self.cache_mb, 100..=5000)
                .logarithmic(true)
                .suffix(format!(" {}", tr!("Mo")))
                .text(tr!("au plus")),
        );
        // Saved once the handle is let go, which trims the cache if needed.
        if slider.drag_stopped() || (slider.changed() && !slider.dragged()) {
            let bytes = self.cache_mb as u64 * 1_000_000;
            job = Some(Box::new(move || tiles::set_cache_limit(bytes)));
        }
        if let Some(job) = job {
            self.measure(job);
        }
    }

    /// The layers, top first, each shown or hidden, with its opacity. Their order is
    /// IGN's cartographic one, the photo always at the bottom.
    fn layers_panel(&mut self, ui: &mut egui::Ui) {
        ui.strong(tr!("Calques"));
        if self.map_memory.zoom() < layers::STACK_ZOOM as f64 {
            ui.weak(tr!("Zoomez pour afficher les calques."));
        }
        // A fixed width, so the sliders line up on the right edge.
        ui.set_width(CARD_WIDTH);
        ui.spacing_mut().slider_width = 90.0;
        // The photo comes last.
        let last = self.layers.len() - 1;
        let (plan, photo) = self.layers.split_at_mut(last);
        // The Plan IGN row unfolds to its categories.
        let id = ui.make_persistent_id("plan ign");
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false)
            .show_header(ui, |ui| {
                layer_row(
                    ui,
                    &mut self.vector.visible,
                    Some(&mut self.vector.opacity),
                    "Plan IGN",
                )
            })
            .body(|ui| {
                ui.add_enabled_ui(self.vector.visible, |ui| {
                    for layer in plan {
                        // Labels without opacity: see `MapLayer::shown`.
                        let labels = layer.category == Some(Category::Labels);
                        let opacity = (!labels).then_some(&mut layer.opacity);
                        layer_row(ui, &mut layer.visible, opacity, layer.name);
                    }
                });
            });
        for layer in photo {
            layer_row(ui, &mut layer.visible, Some(&mut layer.opacity), layer.name);
        }
    }

    fn settings_3d(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame) {
        let block = self.view3d.is_some();
        // As wide as the layer card: the title, then the export flush right.
        ui.set_width(CARD_WIDTH);
        ui.horizontal(|ui| {
            ui.strong(tr!("Bloc"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Right to left: the size, then the button before it.
                egui::ComboBox::from_id_salt("largeur")
                    .width(0.0) // as wide as its text
                    .selected_text(format!("{} px", self.export_width))
                    .show_ui(ui, |ui| {
                        for w in EXPORT_WIDTHS {
                            ui.selectable_value(&mut self.export_width, w, format!("{w} px"));
                        }
                    });
                let export = ui
                    .add_enabled(
                        block && !self.exporting,
                        egui::Button::new(tr!("Exporter…")),
                    )
                    .on_hover_text(tr!("Image PNG, fond transparent"));
                if export.clicked() {
                    self.export(frame);
                }
            });
        });
        // The sliders stretched to the right edge, by how short of it they were last frame.
        let id = ui.scope_id().with("sliders");
        let slider_width: f32 = ui.data(|d| d.get_temp(id)).unwrap_or(110.0);
        ui.spacing_mut().slider_width = slider_width;
        // Without a block, the same controls, greyed out on placeholder values.
        let mut none = (1.5, 300.0);
        let (exaggeration, base_depth) = match &mut self.view3d {
            Some(view) => (&mut view.exaggeration, &mut view.base_depth),
            None => (&mut none.0, &mut none.1),
        };
        let sliders = ui.add_enabled_ui(block, |ui| {
            egui::Grid::new("bloc").show(ui, |ui| {
                ui.label(tr!("Relief"));
                ui.add(egui::Slider::new(exaggeration, 1.0..=2.0).prefix("×"));
                ui.end_row();
                ui.label(tr!("Socle"));
                ui.add(
                    egui::Slider::new(base_depth, view3d::BASE_DEPTHS)
                        .logarithmic(true)
                        .suffix(" m"),
                );
                ui.end_row();
            });
        });
        let short = CARD_WIDTH - sliders.response.rect.width();
        if short.abs() > 0.5 {
            ui.data_mut(|d| d.insert_temp(id, (slider_width + short).max(40.0)));
            ui.ctx().request_repaint();
        }
    }
}

impl eframe::App for Blocs {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.poll(frame);
        self.refresh_lookup();
        self.drape_visible(frame.wgpu_render_state());
        self.build_if_ready();

        // The views fill the window; the controls float over them on cards.
        let all = ui.available_rect_before_wrap();
        match self.view {
            View::Map => self.map_view(ui),
            View::Block => self.block_view(ui, frame),
            View::Split => self.split_view(ui, frame, all),
        }
        // Down the left edge: toolbar, layers, then the block's settings.
        let ctx = ui.ctx().clone();
        let below = |r: egui::Response| egui::pos2(all.left(), r.rect.bottom());
        let bar = card(&ctx, "barre", all.left_top(), |ui| self.top_bar(ui));
        if self.view3d.is_some() && self.view != View::Split {
            let corner = egui::pos2(bar.response.rect.right(), all.top());
            card(&ctx, "vues", corner, |ui| self.view_bar(ui));
        }
        let layers = card(&ctx, "calques", below(bar.response), |ui| {
            self.layers_panel(ui)
        });
        if self.view != View::Map {
            card(&ctx, "réglages", below(layers.response), |ui| {
                self.settings_3d(ui, frame)
            });
        }
        // The sources seen, bottom right, as on an export.
        egui::Area::new(egui::Id::unique("sources"))
            .pivot(egui::Align2::RIGHT_BOTTOM)
            .fixed_pos(all.right_bottom() - egui::Vec2::splat(MARGIN))
            .interactable(false)
            .show(&ctx, |ui| {
                // A thin card, coloured like the others.
                let visuals = ui.visuals();
                // Text grey as the search box's hint, a little larger than small text.
                let (fill, color) = (visuals.window_fill, visuals.weak_text_color());
                egui::Frame::NONE
                    .fill(fill)
                    .corner_radius(3.0)
                    .inner_margin(egui::Margin::symmetric(4, 1))
                    .show(ui, |ui| {
                        // On one line, however long.
                        let text = egui::RichText::new(self.credits()).size(11.0).color(color);
                        ui.add(egui::Label::new(text).extend())
                    });
            });
        egui::Area::new(egui::Id::unique("activité"))
            .pivot(egui::Align2::LEFT_BOTTOM)
            .fixed_pos(all.left_bottom() + egui::vec2(MARGIN, -MARGIN))
            .show(&ctx, |ui| self.activity(ui));
    }
}

impl Blocs {
    /// The map on the left, the block on the right, their separator dragged. Let go
    /// against an edge, it leaves the other view alone.
    fn split_view(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame, all: egui::Rect) {
        let x = all.left() + self.split * all.width();
        let (left, right) = all.split_left_right_at_x(x);
        for (rect, map) in [(left, true), (right, false)] {
            ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                ui.set_clip_rect(rect);
                if map {
                    self.map_view(ui)
                } else {
                    self.block_view(ui, frame)
                }
            });
        }
        // Added last, the handle gets the pointer over both views.
        let handle = egui::Rect::from_x_y_ranges(x - 4.0..=x + 4.0, all.y_range());
        let drag = ui.interact(
            handle,
            ui.scope_id().with("séparateur"),
            egui::Sense::drag(),
        );
        let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
        ui.painter()
            .vline(x, all.y_range(), egui::Stroke::new(2.0, stroke.color));
        if drag.hovered() || drag.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        if let Some(p) = drag.interact_pointer_pos() {
            // Near an edge, it snaps to it.
            let edge = EDGE / all.width();
            self.split = match (p.x - all.left()) / all.width() {
                s if s < edge => 0.0,
                s if s > 1.0 - edge => 1.0,
                s => s,
            };
        }
        if drag.drag_stopped() {
            if self.split == 0.0 {
                self.view = View::Block;
            } else if self.split == 1.0 {
                self.view = View::Map;
            }
            // Back to halves for the next time both show.
            if self.view != View::Split {
                self.split = 0.5;
            }
        }
    }

    /// The block, or nothing until there is one.
    fn block_view(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame) {
        let stack = self.stack();
        if let (Some(view), Some(rs)) = (&mut self.view3d, frame.wgpu_render_state()) {
            view.set_stack(rs, &stack);
            view.ui(ui, rs);
        }
    }

    fn map_view(&mut self, ui: &mut egui::Ui) {
        let start = lon_lat(START.0, START.1);
        // Shift + wheel pans east-west, Ctrl + wheel north-south; the plain wheel zooms.
        let scroll = ui.input(|i| i.smooth_scroll_delta);
        let modifiers = ui.input(|i| i.modifiers);
        if (modifiers.shift || modifiers.ctrl)
            && scroll != egui::Vec2::ZERO
            && ui.rect_contains_pointer(ui.available_rect_before_wrap())
        {
            map::pan(&mut self.map_memory, start, scroll);
            ui.input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
        }
        // Drag panning is ours rather than walkers', so that the map is kept in the world
        // before it is drawn: the pointer's move while dragging, then the glide after it.
        let (delta, dt) = ui.input(|i| (i.pointer.delta(), i.stable_dt));
        if self.panning {
            map::pan(&mut self.map_memory, start, delta);
        } else if self.glide.length() > 10.0 {
            map::pan(&mut self.map_memory, start, self.glide * dt);
            self.glide *= (-dt / GLIDE_TAU).exp();
            ui.ctx().request_repaint();
        }
        if map::keep_in_world(&mut self.map_memory, start, ui.available_size()) {
            self.glide = egui::Vec2::ZERO;
        }
        // At the widest, the wheel no longer zooms out: walkers would first show the world
        // too small, with white borders, until `keep_in_world` zooms back in.
        let widest = map::widest_zoom(ui.available_size());
        if self.map_memory.zoom() <= widest + 1e-6 {
            ui.input_mut(|i| i.smooth_scroll_delta.y = i.smooth_scroll_delta.y.max(0.0));
        }
        // White under the map, where tiles have not arrived yet.
        ui.painter()
            .rect_filled(ui.available_rect_before_wrap(), 0.0, egui::Color32::WHITE);
        let overview = self.map_memory.zoom() < layers::STACK_ZOOM as f64;
        let (photo, vector) = (self.photo_visible(), self.vector);
        let mut map = Map::new(None, &mut self.map_memory, start)
            // walkers defaults to Ctrl + wheel zoom and plain-wheel panning.
            .zoom_with_ctrl(false)
            .drag_pan_buttons(egui::DragPanButtons::empty())
            .with_plugin(FrameTool {
                frame: &mut self.frame,
                edit: &mut self.edit,
                drawing: &mut self.drawing,
            });
        // Zoomed out, the whole Plan IGN kept on disk; zoomed in, the chosen layers, bottom
        // first. The Plan IGN's white background would hide the photo, so it goes without.
        if overview || !photo {
            map = map.with_layer(&mut self.background, 1.0);
        }
        for layer in self.layers.iter_mut().rev() {
            let opacity = match overview {
                true => (layer.id != "photo").then_some(1.0),
                false => layer.shown(vector),
            };
            let Some(opacity) = opacity else { continue };
            map = match &mut layer.tiles {
                MapTiles::Images(tiles) => map.with_layer(tiles, opacity),
                MapTiles::Styled(styled) => map.with_layer(styled, opacity),
            };
        }
        let response = ui.add(map);
        // A zoom past the world's edges, kept in it for the next frame.
        if map::keep_in_world(&mut self.map_memory, start, response.rect.size()) {
            ui.ctx().request_repaint();
        }
        // The primary button pans, unless it draws or edits the frame; letting go, it glides.
        self.panning = !self.drawing
            && self.edit.is_none()
            && response.dragged_by(egui::PointerButton::Primary);
        if self.panning {
            self.glide = ui.input(|i| i.pointer.velocity()) * GLIDE_SPEED;
        }
    }
}

/// The separator snaps to an edge this close, in points; let go there, it leaves one view.
const EDGE: f32 = 100.0;

/// Space between the cards and the window's edges.
const MARGIN: f32 = 8.0;

/// Width inside the layer and block cards, one above the other.
const CARD_WIDTH: f32 = 220.0;

/// A card floating over the views, its top left corner a margin away from `corner`.
fn card<R>(
    ctx: &egui::Context,
    id: &str,
    corner: egui::Pos2,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    egui::Area::new(egui::Id::unique(id))
        .fixed_pos(corner + egui::Vec2::splat(MARGIN))
        .show(ctx, |ui| egui::Frame::popup(ui.style()).show(ui, add).inner)
}

/// A layer's toggle, then its opacity slider, if any, at the row's right end.
fn layer_row(ui: &mut egui::Ui, visible: &mut bool, opacity: Option<&mut f32>, name: &str) {
    ui.horizontal(|ui| {
        ui.checkbox(visible, i18n::tr(name));
        let Some(opacity) = opacity else { return };
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let slider = egui::Slider::new(opacity, 0.0..=1.0).show_value(false);
            ui.add_enabled(*visible, slider)
                .on_hover_text(tr!("Opacité"));
        });
    });
}

/// One line of the activity stack, on its own small card.
fn line(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::popup(ui.style()).show(ui, |ui| ui.horizontal(add));
}

/// The help menu: the mouse gestures, on the map and in 3D.
fn help(ui: &mut egui::Ui) {
    let sections = [
        (
            tr!("Carte"),
            tr!(
                "Glisser : déplacer\nMolette : zoom\nMaj + molette : gauche-droite\nCtrl + molette : haut-bas"
            ),
        ),
        (
            tr!("Vue 3D"),
            tr!(
                "Glisser : tourner\nClic droit : déplacer\nMolette : zoom\nMaj + molette : gauche-droite\nCtrl + molette : haut-bas"
            ),
        ),
    ];
    // A little room after the longest lines.
    let room = egui::Margin {
        right: 8,
        ..egui::Margin::ZERO
    };
    egui::Frame::NONE.inner_margin(room).show(ui, |ui| {
        for (k, (title, text)) in sections.into_iter().enumerate() {
            if k > 0 {
                ui.separator();
            }
            ui.strong(title);
            ui.label(text);
        }
    });
}

/// A "Supprimer" button that asks "Confirmer" on the first click; leaving it disarms it.
fn confirmed(ui: &mut egui::Ui, salt: impl egui::AsIdSalt, enabled: bool) -> bool {
    let id = ui.scope_id().with(salt);
    let armed = ui.data(|d| d.get_temp(id)).unwrap_or(false);
    let label = if armed {
        tr!("Confirmer")
    } else {
        tr!("Supprimer")
    };
    let response = ui.add_enabled(enabled, egui::Button::new(label));
    let clicked = response.clicked();
    ui.data_mut(|d| d.insert_temp(id, response.hovered() && (armed != clicked)));
    armed && clicked
}

/// The language picker, at the bottom of the options: its title, then a toggle per
/// language on the same line, each in its own words.
fn language(ui: &mut egui::Ui) {
    let mut lang = i18n::lang();
    ui.horizontal(|ui| {
        ui.strong(tr!("Langue"));
        ui.selectable_value(&mut lang, Lang::Fr, "Français");
        ui.selectable_value(&mut lang, Lang::En, "English");
    });
    if lang != i18n::lang() {
        i18n::set(lang);
    }
}

fn megabytes(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        i18n::decimal(format!("{:.1} {}", bytes as f64 / 1e9, tr!("Go")))
    } else {
        format!("{} {}", bytes / 1_000_000, tr!("Mo"))
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

/// Reads the block of `frame` and describes it with its read time.
fn load_block(frame: &Frame) -> relief::Result<(Heightmap, String)> {
    let start = Instant::now();
    let (nx, ny) = frame.points();
    let map = if frame.angle == 0.0 {
        Heightmap::load(frame.x0, frame.y0, nx, ny)?
    } else {
        // Turned: the box holding it, resampled along its sides.
        let [x0, y0, x1, y1] = frame.bounds().map(|v| (v / CELL).floor());
        let (bx, by) = ((x1 - x0) as usize + 2, (y1 - y0) as usize + 2);
        let held = Heightmap::load(x0 * CELL, y0 * CELL, bx, by)?;
        held.resample(nx, ny, |i, j| {
            let (x, y) = frame.at(i as f64 * CELL, j as f64 * CELL);
            (x / CELL - x0, y / CELL - y0)
        })
    };
    let (lo, hi) = map.range();
    let text = format!(
        "Bloc {nx} × {ny} points lu en {:.2} s · altitudes {lo:.0}–{hi:.0} m · {} points comblés",
        start.elapsed().as_secs_f64(),
        map.holes,
    );
    Ok((map, text))
}

/// `blocs` opens the app. Headless commands, for measurements:
/// `blocs installer D038`, `blocs convertir <archive.7z> D038`, `blocs bloc <lon> <lat>`.
fn main() -> eframe::Result {
    // Library warnings and errors (file dialog, GPU…) in the terminal; RUST_LOG for more.
    // Known, harmless: walkers warns on every tile about style features it skips (circles,
    // long dashes); arboard on GNOME about its clipboard fallback.
    let filter = env_logger::Env::default().default_filter_or("warn,walkers=error,arboard=error");
    env_logger::Builder::from_env(filter).init();
    i18n::load();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["bloc", lon, lat] => {
            let frame = Frame::square(lambert93::from_wgs84(
                lon.parse().unwrap_or(0.0),
                lat.parse().unwrap_or(0.0),
            ));
            cli(|_| load_block(&frame).map(|(_, text)| println!("{text}")))
        }
        ["installer", dep] => cli(|p| relief::install(dep, &AtomicBool::new(false), p)),
        ["convertir", archive, dep] => {
            cli(|p| relief::convert(archive.as_ref(), &relief::department_dir(dep), p))
        }
        _ => {
            let options = eframe::NativeOptions {
                viewport: egui::ViewportBuilder::default().with_inner_size([1280.0, 800.0]),
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
