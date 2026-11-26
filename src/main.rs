mod buildings;
mod drape;
mod export;
mod ign;
mod lambert93;
mod layers;
mod map;
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

use ign::Place;
use layers::Source;
use map::{Edit, Frame, FrameTool};
use relief::{Heightmap, Progress};
use style::Category;
use tiles::{PlanIgnTiles, Store, TileLayer};
use view3d::View3d;

/// Map centre at start-up.
const START: (f64, f64) = (2.35, 46.6);
/// Export widths offered, in pixels.
/// Glide after a drag pan, gentler than walkers' own (2× the pointer speed, 0.2 s):
/// starting speed as a share of the pointer's, and decay time (s).
const GLIDE_SPEED: f32 = 1.0;
const GLIDE_TAU: f32 = 0.12;
const EXPORT_WIDTHS: [u32; 3] = [1280, 1920, 3840];

/// Messages from background work to the UI.
enum Msg {
    Status(String),
    Places(String, Vec<Place>),
    /// For a frame: département names met, and the codes to download.
    Departments(Frame, HashMap<String, String>, Vec<String>),
    /// Archive size of a département, in bytes.
    Size(String, u64),
    Progress(String, Progress),
    Installed(String),
    /// The download stopped: paused or failed.
    Stopped(String),
    /// A block read from disk, with its description, or why it failed.
    Built(Frame, relief::Result<(Heightmap, String)>),
    /// A layer's image over a block's frame, with its mipmaps.
    Draped(Frame, &'static str, relief::Result<Vec<image::RgbaImage>>),
    /// The 3D buildings of a block's frame.
    Buildings(Frame, buildings::Mesh),
    /// How the image export ended.
    Exported(String),
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
    fraction: f32,
}

struct Blocs {
    ctx: egui::Context,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    status: String,

    /// The layer stack, top first: Plan IGN categories, then the photo.
    layers: Vec<MapLayer>,
    /// The Plan IGN's opaque background, drawn when the photo is not.
    background: walkers::StyledSources,
    plan: Store,
    map_memory: MapMemory,
    /// Map speed after a drag pan, in points per second.
    glide: egui::Vec2,
    frame: Option<Frame>,
    edit: Option<Edit>,
    drawing: bool,
    search: String,
    search_typed: Option<Instant>,
    places: Vec<Place>,

    installed: Vec<String>,
    names: HashMap<String, String>,
    sizes: HashMap<String, u64>,
    in_frame: Lookup,
    download: Download,

    view3d: Option<View3d>,
    built: Option<Frame>,
    building: bool,
    /// Layers whose image is on the block, and those being prepared.
    draped: HashSet<&'static str>,
    draping: HashSet<&'static str>,
    show_3d: bool,
    export_width: u32,
    /// An export is being saved; then how the last one ended.
    exporting: bool,
    export_note: String,
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
        Self {
            ctx: cc.egui_ctx.clone(),
            tx,
            rx,
            status: String::new(),
            layers,
            background,
            plan,
            map_memory,
            glide: egui::Vec2::ZERO,
            frame: None,
            edit: None,
            drawing: false,
            search: String::new(),
            search_typed: None,
            places: Vec::new(),
            installed: relief::installed(),
            names: HashMap::new(),
            sizes: HashMap::new(),
            in_frame: Lookup::default(),
            download: Download {
                queue: Vec::new(),
                pause: Arc::default(),
                running: false,
                text: String::new(),
                fraction: 0.0,
            },
            view3d: None,
            built: None,
            building: false,
            draped: HashSet::new(),
            draping: HashSet::new(),
            show_3d: false,
            export_width: EXPORT_WIDTHS[1],
            exporting: false,
            export_note: String::new(),
        }
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
                Msg::Status(text) => self.status = text,
                Msg::Exported(note) => {
                    self.exporting = false;
                    self.export_note = note;
                }
                Msg::Places(query, places) => {
                    if query == self.search.trim() {
                        self.places = places;
                    }
                }
                Msg::Departments(frame, names, codes) => {
                    self.names.extend(names);
                    if self.in_frame.frame == Some(frame) {
                        self.in_frame.codes = Some(codes);
                    }
                }
                Msg::Size(code, size) => {
                    self.sizes.insert(code, size);
                }
                Msg::Progress(dep, p) => {
                    let name = self.name(&dep);
                    (self.download.text, self.download.fraction) = match p {
                        Progress::Download { done, total } => (
                            format!(
                                "{name} : téléchargement {} / {} Mo",
                                done / 1_000_000,
                                total / 1_000_000
                            ),
                            done as f32 / total.max(1) as f32,
                        ),
                        Progress::Convert { done, total } => (
                            format!("{name} : préparation {done} / {total} dalles"),
                            done as f32 / total.max(1) as f32,
                        ),
                    };
                }
                Msg::Installed(dep) => {
                    self.status = format!("Relief installé : {}.", self.name(&dep));
                    self.download.queue.retain(|d| *d != dep);
                    self.download.running = false;
                    self.installed = relief::installed();
                    self.next_download();
                }
                Msg::Stopped(error) => {
                    self.download.running = false;
                    if !self.download.pause.load(Ordering::Relaxed) {
                        self.download.pause.store(true, Ordering::Relaxed);
                        self.status = format!("Téléchargement interrompu : {error}");
                    }
                }
                Msg::Draped(built, id, result) => {
                    self.draping.remove(id);
                    // A layer for an older block is dropped; the current one asks again.
                    if self.built != Some(built) {
                        continue;
                    }
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
                            self.status = format!("Calque indisponible sur le bloc : {e}");
                        }
                    }
                }
                Msg::Buildings(built, mesh) => {
                    self.draping.remove(BUILDINGS);
                    if let (true, Some(view), Some(rs)) = (
                        self.built == Some(built),
                        &self.view3d,
                        frame.wgpu_render_state(),
                    ) {
                        view.set_buildings(rs, &mesh);
                        self.draped.insert(BUILDINGS);
                    }
                }
                Msg::Built(built, result) => {
                    self.building = false;
                    match result {
                        Ok((map, text)) => {
                            let start = Instant::now();
                            let rs = frame.wgpu_render_state().expect("rendu wgpu");
                            if let Some(old) = &mut self.view3d {
                                old.release(rs);
                            }
                            let (x, y) = ((built.x0 + built.x1) / 2.0, (built.y0 + built.y1) / 2.0);
                            let latitude = lambert93::to_wgs84(x, y).1;
                            self.view3d = Some(View3d::new(rs, &map, latitude));
                            // The new block starts without layer images.
                            self.draped.clear();
                            self.draping.clear();
                            self.status = format!(
                                "{text} · envoyé au GPU en {:.2} s",
                                start.elapsed().as_secs_f64()
                            );
                            self.built = Some(built);
                            self.show_3d = true;
                        }
                        Err(e) => self.status = format!("Échec : {e}"),
                    }
                }
            }
        }
    }

    fn name(&self, dep: &str) -> String {
        self.names
            .get(dep)
            .cloned()
            .unwrap_or_else(|| dep.to_owned())
    }

    /// Finds the départements to download for `frame`, then their archive sizes.
    fn look_up(&self, frame: Frame) {
        let installed = self.installed.clone();
        let known: HashSet<String> = self.sizes.keys().cloned().collect();
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        self.spawn(move |send| {
            let http = match ign::client() {
                Ok(http) => http,
                Err(e) => return send(Msg::Status(format!("Réseau indisponible : {e}"))),
            };
            // Which départements touch each dalle of the frame, asked in parallel.
            let dalles = relief::dalles([frame.x0, frame.y0, frame.x1, frame.y1]);
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
                Err(e) => return send(Msg::Status(format!("Départements introuvables : {e}"))),
            };
            let names: HashMap<_, _> = answers.iter().flatten().cloned().collect();
            let per_dalle: Vec<Vec<String>> = answers
                .iter()
                .map(|deps| deps.iter().map(|d| d.0.clone()).collect())
                .collect();
            let needed = relief::cover(&per_dalle, &installed);
            send(Msg::Departments(frame, names, needed.clone()));

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

    /// Why the 3D view can't open yet, if it can't.
    fn missing_3d(&self) -> Option<String> {
        if self.frame.is_none() {
            return Some("Tracez d'abord un cadre sur la carte.".into());
        }
        let codes = self.in_frame.codes.as_ref()?;
        let missing: Vec<_> = codes
            .iter()
            .filter(|c| !self.installed.contains(c))
            .map(|c| self.name(c))
            .collect();
        (!missing.is_empty())
            .then(|| format!("Téléchargez d'abord le relief : {}.", missing.join(", ")))
    }

    fn open_3d(&mut self) {
        let Some(frame) = self.frame else { return };
        if self.built == Some(frame) {
            self.show_3d = true;
        } else if !self.building {
            self.building = true;
            self.status = "Construction du bloc…".into();
            self.spawn(move |send| send(Msg::Built(frame, load_block(&frame))));
        }
    }

    /// Prepares the block image of each visible layer that has none yet. The Plan IGN
    /// categories share one job, which fetches their tiles once.
    fn drape_visible(&mut self, rs: Option<&eframe::egui_wgpu::RenderState>) {
        let (Some(frame), Some(rs)) = (self.built, rs) else {
            return;
        };
        let background = (!self.photo_visible()).then_some((BACKGROUND, None));
        let wanted: Vec<_> = (self.layers.iter().filter(|l| l.visible))
            .map(|l| (l.id, l.category))
            .chain(background)
            .chain([(BUILDINGS, None)])
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
        let visible = self.layers.iter().rev().filter(|l| l.visible);
        background
            .into_iter()
            .chain(visible.map(|l| (l.id, l.opacity)))
            .collect()
    }

    fn export(&mut self, frame: &eframe::Frame) {
        let (Some(view), Some(rs)) = (&self.view3d, frame.wgpu_render_state()) else {
            return;
        };
        // Parented to the window, so that the dialog never opens behind it.
        let Some(path) = rfd::FileDialog::new()
            .set_parent(frame)
            .set_file_name("bloc.png")
            .add_filter("Image PNG", &["png"])
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
            shown.iter().any(|&id| id != "photo"),
            shown.contains(&"photo"),
        );
        self.exporting = true;
        self.spawn(move |send| {
            send(Msg::Exported(
                match export::save_png(&path, w, h, rgba, &credits) {
                    Ok(()) => format!("Image enregistrée : {}", path.display()),
                    Err(e) => format!("Échec de l'export : {e}"),
                },
            ))
        });
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.show_3d, false, "Carte");
            let missing = self.missing_3d();
            let button = ui
                .add_enabled(
                    missing.is_none(),
                    egui::Button::selectable(self.show_3d, "3D"),
                )
                .on_disabled_hover_text(missing.unwrap_or_default());
            if button.clicked() {
                self.open_3d();
            }
            ui.separator();

            let search = ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("Rechercher un lieu…")
                    .desired_width(260.0),
            );
            if search.changed() {
                self.search_typed = Some(Instant::now());
                self.places.clear();
            }
            if search.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                self.go_to(0);
            }
            self.search_results(ui, &search);

            if !self.show_3d {
                ui.separator();
                ui.toggle_value(&mut self.drawing, "Tracer le cadre")
                    .on_hover_text(
                        "Glissez sur la carte pour tracer le cadre (10 km au plus par côté).",
                    );
                if let Some(frame) = self.frame {
                    ui.label(format!("Cadre {}", frame.label()));
                }
            }
            ui.separator();
            ui.label(&self.status);
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
        let _ = self.map_memory.set_zoom(13.0);
        self.search = place.label.clone();
        self.places.clear();
    }

    /// Offer to download the relief, or show the download in progress.
    fn download_bar(&mut self, ui: &mut egui::Ui) {
        let proposal = self.proposal();
        let offer = !proposal.is_empty();
        if !offer && self.download.queue.is_empty() {
            return;
        }
        egui::Panel::bottom("téléchargement").show(ui, |ui| {
            if !self.download.queue.is_empty() {
                ui.horizontal(|ui| {
                    let paused = !self.download.running;
                    if ui
                        .button(if paused { "Reprendre" } else { "Pause" })
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
                        text += &format!(" · {} en attente", self.download.queue.len() - 1);
                    }
                    if paused {
                        text += " · en pause";
                    }
                    ui.add(egui::ProgressBar::new(self.download.fraction).text(text));
                });
            }
            if offer {
                ui.horizontal(|ui| {
                    let names: Vec<_> = proposal.iter().map(|d| self.name(d)).collect();
                    ui.label(format!("Relief à télécharger : {}", names.join(", ")));
                    // The total shows once every size is known.
                    let sizes: Option<Vec<u64>> = proposal
                        .iter()
                        .map(|d| self.sizes.get(d).copied())
                        .collect();
                    let label = match sizes {
                        Some(s) => format!("Télécharger ({})", megabytes(s.iter().sum())),
                        None => "Télécharger".into(),
                    };
                    if ui.button(label).clicked() {
                        self.download.queue.extend(proposal);
                        self.next_download();
                    }
                });
            }
        });
    }

    /// The layers, top first, each shown or hidden, with its opacity. Their order is
    /// IGN's cartographic one, the photo always at the bottom.
    fn layers_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Calques");
        if self.map_memory.zoom() < layers::STACK_ZOOM as f64 {
            ui.label("Vue d'ensemble : Plan IGN complet.\nZoomez pour choisir les calques.");
        }
        for layer in &mut self.layers {
            ui.checkbox(&mut layer.visible, layer.name);
            ui.add_enabled(
                layer.visible,
                egui::Slider::new(&mut layer.opacity, 0.0..=1.0)
                    .show_value(false)
                    .text("opacité"),
            );
            ui.separator();
        }
    }

    fn settings_3d(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame) {
        if !self.draping.is_empty() {
            let names: Vec<_> = (self.layers.iter())
                .map(|l| (l.id, l.name))
                .chain([(BACKGROUND, "Fond du Plan IGN"), (BUILDINGS, "Bâtiments")])
                .filter(|(id, _)| self.draping.contains(id))
                .map(|(_, name)| name)
                .collect();
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!("Préparation : {}", names.join(", ")));
            });
            ui.separator();
        }
        let Some(view) = &mut self.view3d else { return };
        ui.label("Étirement du relief");
        ui.add(egui::Slider::new(&mut view.exaggeration, 1.0..=2.0).prefix("×"));
        ui.label("Profondeur du socle");
        ui.add(egui::Slider::new(&mut view.base_depth, 50.0..=1000.0).suffix(" m"));
        ui.separator();
        ui.label("Glisser : tourner\nClic droit : déplacer\nMolette : zoom\nMaj + molette : gauche-droite\nCtrl + molette : haut-bas\nDisque jaune : soleil");
        ui.separator();
        ui.label("Image PNG (fond transparent)");
        egui::ComboBox::from_id_salt("largeur")
            .selected_text(format!("{} px", self.export_width))
            .show_ui(ui, |ui| {
                for w in EXPORT_WIDTHS {
                    ui.selectable_value(&mut self.export_width, w, format!("{w} px"));
                }
            });
        if ui
            .add_enabled(!self.exporting, egui::Button::new("Exporter…"))
            .clicked()
        {
            self.export(frame);
        }
        if self.exporting {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Enregistrement de l'image…");
            });
        } else {
            ui.label(&self.export_note);
        }
    }
}

impl eframe::App for Blocs {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.poll(frame);
        self.refresh_lookup();
        self.drape_visible(frame.wgpu_render_state());

        egui::Panel::top("barre").show(ui, |ui| self.top_bar(ui));

        if self.show_3d && self.view3d.is_some() {
            egui::Panel::right("réglages").show(ui, |ui| self.settings_3d(ui, frame));
            let stack = self.stack();
            if let (Some(view), Some(rs)) = (&mut self.view3d, frame.wgpu_render_state()) {
                view.set_stack(rs, &stack);
                view.ui(ui, rs);
            }
            return;
        }

        egui::Panel::left("calques").show(ui, |ui| self.layers_panel(ui));
        self.download_bar(ui);
        // Shift + wheel pans east-west, Ctrl + wheel north-south; the plain wheel zooms.
        let scroll = ui.input(|i| i.smooth_scroll_delta);
        let modifiers = ui.input(|i| i.modifiers);
        if (modifiers.shift || modifiers.ctrl)
            && scroll != egui::Vec2::ZERO
            && ui.rect_contains_pointer(ui.available_rect_before_wrap())
        {
            map::pan(&mut self.map_memory, lon_lat(START.0, START.1), scroll);
            ui.input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
        }
        // White under the map, where tiles have not arrived yet.
        ui.painter()
            .rect_filled(ui.available_rect_before_wrap(), 0.0, egui::Color32::WHITE);
        let pan = !self.drawing && self.edit.is_none();
        let overview = self.map_memory.zoom() < layers::STACK_ZOOM as f64;
        let photo = self.photo_visible();
        let mut map = Map::new(None, &mut self.map_memory, lon_lat(START.0, START.1))
            // walkers defaults to Ctrl + wheel zoom and plain-wheel panning.
            .zoom_with_ctrl(false)
            .drag_pan_buttons(if pan {
                egui::DragPanButtons::PRIMARY
            } else {
                egui::DragPanButtons::empty()
            })
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
                true if layer.id != "photo" => 1.0,
                false if layer.visible => layer.opacity,
                _ => continue,
            };
            map = match &mut layer.tiles {
                MapTiles::Images(tiles) => map.with_layer(tiles, opacity),
                MapTiles::Styled(styled) => map.with_layer(styled, opacity),
            };
        }
        let response = ui.add(map);

        // Replace walkers' glide on release, which carries the map too far, with ours.
        let (velocity, dt) = ui.input(|i| (i.pointer.velocity(), i.stable_dt));
        if pan && response.dragged_by(egui::PointerButton::Primary) {
            self.glide = velocity * GLIDE_SPEED;
        } else if response.drag_stopped() {
            if let Some(center) = self.map_memory.detached() {
                self.map_memory.center_at(center);
            }
        } else if self.glide.length() > 10.0 {
            map::pan(
                &mut self.map_memory,
                lon_lat(START.0, START.1),
                self.glide * dt,
            );
            self.glide *= (-dt / GLIDE_TAU).exp();
            ui.ctx().request_repaint();
        }
    }
}

fn megabytes(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.1} Go", bytes as f64 / 1e9).replace('.', ",")
    } else {
        format!("{} Mo", bytes / 1_000_000)
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
    let map = Heightmap::load(frame.x0, frame.y0, nx, ny)?;
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
