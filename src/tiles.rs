//! Layer tiles for the map, cached on disk.
//!
//! The Plan IGN levels up to `SEED_ZOOM` are kept for good; every other tile shares a disk
//! cache capped at `CACHE_LIMIT`, least recently used first out. The Géoplateforme sends no
//! cache headers, so an HTTP cache would fetch them again.
//! Tiles live in `<data>/tuiles/<layer>/<z>/<x>/<y>.<ext>`.
//!
//! One `Fetcher` per source downloads and prepares tiles; `TileLayer` draws them as images,
//! `PlanIgnTiles` hands the Plan IGN vector tiles to walkers, which styles them.

use std::cell::RefCell;
use std::collections::HashMap;
use std::f64::consts::PI;
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime};

use bytes::Bytes;
use eframe::egui::{self, ColorImage, Rect, TextureHandle, pos2};
use walkers::sources::Attribution;
use walkers::{SourceData, Stats, Tile, TileData, TileId, TilePiece, TiledSource, Tiles};

use crate::layers::Source;
use crate::relief::{Result, data_dir};

/// Parallel requests per layer: the server is slow to answer each tile, so several in flight hide it.
const WORKERS: usize = 6;
/// Of those, how many may serve preloads and the seed, so visible tiles keep the bandwidth.
const BACKGROUND: usize = 2;
const MAX_READY: usize = 1024;
/// A failed tile is asked again after this delay.
const RETRY: Duration = Duration::from_secs(30);
/// Plan IGN levels kept on disk for all of metropolitan France, so overviews load at once
/// and deeper levels always have a coarser tile to stand in while loading.
const SEED_ZOOM: u8 = 10;
const FRANCE: [f64; 4] = [-5.5, 41.2, 10.0, 51.2]; // lon/lat bounds, Corsica included
/// Disk space for all tiles but the seeded ones.
const CACHE_LIMIT: u64 = 500_000_000;
pub const IGN: Attribution = Attribution {
    text: "Source : IGN – Licence Ouverte",
    url: "https://geoservices.ign.fr/",
    logo_light: None,
    logo_dark: None,
};

/// A tile as a worker hands it back.
enum Fetched {
    Image(ColorImage),
    /// Raw vector tile, for walkers to style.
    Data(Bytes),
}

/// A tile ready to draw.
#[derive(Clone)]
enum Ready {
    Texture(TextureHandle),
    Data(Arc<SourceData>),
}

enum State {
    Loading,
    /// With the egui pass it was last drawn in.
    Ready(Ready, u64),
    Failed(Instant),
}

/// Tiles requested but not yet fetched. Visible tiles and preloads go nearest to the
/// screen centre first, so the map fills from the middle outwards.
#[derive(Default)]
struct Pending {
    /// Tiles on screen.
    shown: Vec<TileId>,
    /// Neighbouring zoom levels.
    preload: Vec<TileId>,
    /// France overview tiles not yet on disk.
    seed: Vec<TileId>,
    /// Preloads and seed tiles being fetched.
    background: usize,
    /// Screen centre, in world units (see `centre`).
    centre: (f64, f64),
}

#[derive(Clone, Copy, PartialEq)]
enum Job {
    Shown,
    Preload,
    Seed,
}

impl Pending {
    /// Next tile to fetch; background work only while a background slot is free.
    fn next(&mut self) -> Option<(TileId, Job)> {
        if let Some(id) = nearest(&mut self.shown, self.centre) {
            return Some((id, Job::Shown));
        }
        if self.background >= BACKGROUND {
            return None;
        }
        let next = (nearest(&mut self.preload, self.centre).map(|id| (id, Job::Preload)))
            .or_else(|| self.seed.pop().map(|id| (id, Job::Seed)))?;
        self.background += 1;
        Some(next)
    }
}

type Queue = Arc<(Mutex<Pending>, Condvar)>;

/// Where one layer's tiles come from and where they are kept.
#[derive(Clone)]
pub struct Store {
    pub source: Arc<Source>,
    pub dir: PathBuf,
}

impl Store {
    pub fn new(id: &str, source: Source) -> Self {
        Self {
            source: Arc::new(source),
            dir: data_dir().join("tuiles").join(id),
        }
    }

    fn path(&self, id: TileId) -> PathBuf {
        self.dir
            .join(id.zoom.to_string())
            .join(id.x.to_string())
            .join(format!("{}.{}", id.y, self.source.extension()))
    }

    /// The tile's bytes, from disk, or downloaded and stored first.
    pub fn bytes(&self, http: &reqwest::blocking::Client, id: TileId) -> Result<Vec<u8>> {
        let path = self.path(id);
        if let Ok(bytes) = fs::read(&path) {
            // Mark the tile as recently used, for the cache trimming.
            if let Ok(file) = fs::File::options().append(true).open(&path) {
                let _ = file.set_modified(SystemTime::now());
            }
            return Ok(bytes);
        }
        let bytes = http
            .get(self.source.url(id.zoom, id.x, id.y))
            .timeout(crate::ign::SHORT)
            .send()?
            .error_for_status()?
            .bytes()?
            .to_vec();
        // Write then rename, so an interrupted write never leaves a broken tile.
        fs::create_dir_all(path.parent().unwrap())?;
        let tmp = path.with_extension(format!("{:?}.tmp", std::thread::current().id()));
        fs::write(&tmp, &bytes)?;
        fs::rename(&tmp, &path)?;
        Ok(bytes)
    }

    /// The tile prepared for the map: decoded, or raw for walkers.
    fn fetch(&self, http: &reqwest::blocking::Client, id: TileId) -> Result<Fetched> {
        let bytes = self.bytes(http, id)?;
        Ok(match &*self.source {
            Source::Image { .. } => {
                let image = image::load_from_memory(&bytes)?.to_rgba8();
                let size = [image.width() as usize, image.height() as usize];
                Fetched::Image(ColorImage::from_rgba_unmultiplied(size, &image))
            }
            Source::PlanIgn => Fetched::Data(Bytes::from(bytes)),
        })
    }
}

/// Downloads and prepares one layer's tiles, in the background, most useful first.
struct Fetcher {
    ctx: egui::Context,
    store: Store,
    tiles: HashMap<TileId, State>,
    queue: Queue,
    done: Receiver<(TileId, Option<Fetched>)>,
    /// Sum and count of the visible tiles' centres this pass.
    seen: (f64, f64, u32),
    /// egui pass whose tiles are queued.
    pass: u64,
}

impl Fetcher {
    /// `seed`: also download the France overview levels once, in the background.
    fn new(ctx: egui::Context, store: Store, seed: bool) -> Self {
        // Coarsest levels last in the list, so they are fetched first.
        let seed = match seed {
            true => (0..=SEED_ZOOM)
                .rev()
                .flat_map(france_tiles)
                .filter(|id| !store.path(*id).exists())
                .collect(),
            false => Vec::new(),
        };
        let queue: Queue = Arc::new((
            Mutex::new(Pending {
                seed,
                ..Default::default()
            }),
            Condvar::new(),
        ));
        let (tx, done) = channel();
        // One client for all workers: they share its connection pool.
        let http = crate::ign::client().expect("client HTTP");
        for _ in 0..WORKERS {
            let (queue, tx, ctx, http, store) = (
                queue.clone(),
                tx.clone(),
                ctx.clone(),
                http.clone(),
                store.clone(),
            );
            std::thread::spawn(move || worker(&http, &store, &queue, &tx, &ctx));
        }
        Self {
            ctx,
            store,
            tiles: HashMap::new(),
            queue,
            done,
            seen: (0.0, 0.0, 0),
            pass: 0,
        }
    }

    /// Bookkeeping before each request of a frame: new frame, arrived tiles, screen centre.
    fn visit(&mut self, id: TileId) {
        self.new_pass();
        self.receive();
        let (x, y) = centre(id);
        self.seen = (self.seen.0 + x, self.seen.1 + y, self.seen.2 + 1);
    }

    /// On a new frame, forgets requests not started yet: the frame queues again
    /// what it shows, so tiles scrolled or zoomed past are never fetched.
    fn new_pass(&mut self) {
        let pass = self.ctx.cumulative_pass_nr();
        if pass == self.pass {
            return;
        }
        self.pass = pass;
        self.seen = (0.0, 0.0, 0);
        let queue = self.queue.clone();
        let mut pending = queue.0.lock().unwrap();
        let pending = &mut *pending;
        for id in pending.shown.drain(..).chain(pending.preload.drain(..)) {
            self.tiles.remove(&id);
        }
        // Drop the least recently drawn half of the ready tiles.
        let mut ages: Vec<u64> = self.tiles.values().filter_map(age).collect();
        if ages.len() > MAX_READY {
            let cut = *ages.select_nth_unstable(MAX_READY / 2).1;
            self.tiles.retain(|_, s| age(s).is_none_or(|a| a > cut));
        }
    }

    fn receive(&mut self) {
        while let Ok((id, fetched)) = self.done.try_recv() {
            let state = match fetched {
                Some(Fetched::Image(image)) => {
                    let texture =
                        self.ctx
                            .load_texture(format!("{id:?}"), image, Default::default());
                    State::Ready(Ready::Texture(texture), self.pass)
                }
                Some(Fetched::Data(bytes)) => {
                    State::Ready(Ready::Data(Arc::new(SourceData::Encoded(bytes))), self.pass)
                }
                None => State::Failed(Instant::now()),
            };
            self.tiles.insert(id, state);
        }
    }

    /// The tile if ready, marked as just used.
    fn ready(&mut self, id: TileId) -> Option<Ready> {
        match self.tiles.get_mut(&id) {
            Some(State::Ready(ready, used)) => {
                *used = self.pass;
                Some(ready.clone())
            }
            _ => None,
        }
    }

    /// Queues a tile unless it is ready, in flight, or failed recently.
    fn request(&mut self, id: TileId, preload: bool) {
        match self.tiles.get(&id) {
            Some(State::Failed(t)) if t.elapsed() > RETRY => {}
            Some(_) => return,
            None if !valid(id, self.store.source.max_zoom()) => return,
            None => {}
        }
        self.tiles.insert(id, State::Loading);
        let (lock, ready) = &*self.queue;
        let mut pending = lock.lock().unwrap();
        let (x, y, n) = self.seen;
        pending.centre = (x / n.max(1) as f64, y / n.max(1) as f64);
        if preload {
            pending.preload.push(id);
        } else {
            pending.shown.push(id);
        }
        ready.notify_one();
    }

    /// Queues a shown tile, then its parent and its four children.
    fn request_around(&mut self, id: TileId) {
        self.request(id, false);
        if id.zoom > 0 {
            self.request(ancestor(id, id.zoom - 1), true);
        }
        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            let (x, y, zoom) = (2 * id.x + dx, 2 * id.y + dy, id.zoom + 1);
            self.request(TileId { x, y, zoom }, true);
        }
    }

    fn stats(&self) -> Stats {
        let loading = self.tiles.values().filter(|s| matches!(s, State::Loading));
        Stats {
            in_progress: loading.count(),
        }
    }
}

/// One layer's tiles drawn as images by walkers.
pub struct TileLayer {
    fetcher: Fetcher,
}

impl TileLayer {
    pub fn new(ctx: egui::Context, store: Store) -> Self {
        Self {
            fetcher: Fetcher::new(ctx, store, false),
        }
    }
}

impl Tiles for TileLayer {
    fn at(&mut self, id: TileId) -> Option<TilePiece> {
        let f = &mut self.fetcher;
        f.visit(id);
        let wanted = ancestor(id, id.zoom.min(f.store.source.max_zoom()));
        // Ready the neighbouring scales too, so zooming in or out stays sharp.
        f.request_around(wanted);
        // Until it is loaded, show the nearest coarser tile, cropped.
        let (texture, zoom) =
            (0..=wanted.zoom)
                .rev()
                .find_map(|zoom| match f.ready(ancestor(id, zoom)) {
                    Some(Ready::Texture(t)) => Some((t, zoom)),
                    _ => None,
                })?;
        let cells = 1u32 << (id.zoom - zoom);
        let n = cells as f32;
        let (u, v) = ((id.x % cells) as f32 / n, (id.y % cells) as f32 / n);
        let uv = Rect::from_min_max(pos2(u, v), pos2(u + 1.0 / n, v + 1.0 / n));
        Some(TilePiece::new(Tile::Raster(texture), uv))
    }

    fn attribution(&self) -> Attribution {
        IGN
    }

    fn tile_size(&self) -> u32 {
        256
    }

    fn stats(&self) -> Stats {
        self.fetcher.stats()
    }
}

/// The Plan IGN vector tiles, handed to walkers, which draws them with IGN's style.
/// Its overview levels are kept on disk for good. Clones share one download, so each
/// category of the style can be drawn on its own from the same tiles.
#[derive(Clone)]
pub struct PlanIgnTiles {
    fetcher: Rc<RefCell<Fetcher>>,
}

impl PlanIgnTiles {
    pub fn new(ctx: egui::Context, store: Store) -> Self {
        Self {
            fetcher: Rc::new(RefCell::new(Fetcher::new(ctx, store, true))),
        }
    }
}

impl TiledSource for PlanIgnTiles {
    fn data(&mut self, id: TileId) -> TileData {
        let f = &mut *self.fetcher.borrow_mut();
        f.visit(id);
        f.request_around(id);
        match f.ready(id) {
            Some(Ready::Data(data)) => TileData::Ready(data),
            // Not ready yet: walkers then asks for the parent, and so on up to a ready one,
            // which it draws cropped. It asks for this tile again on the next frame.
            // (`Loading` would draw nothing unless walkers had already styled a parent.)
            _ => TileData::Missing,
        }
    }

    fn attribution(&self) -> Attribution {
        IGN
    }

    /// IGN's style is made for 512 px tiles, as MapLibre draws it: details and labels come
    /// one zoom level later than with 256 px, and half as many tiles are fetched.
    fn tile_size(&self) -> u32 {
        512
    }

    fn max_zoom(&self) -> u8 {
        self.fetcher.borrow().store.source.max_zoom()
    }

    fn stats(&self) -> Stats {
        self.fetcher.borrow().stats()
    }
}

/// Tile centre in world units: the whole Web Mercator map is 1 × 1 at every zoom.
fn centre(id: TileId) -> (f64, f64) {
    let n = (1u32 << id.zoom) as f64;
    ((id.x as f64 + 0.5) / n, (id.y as f64 + 0.5) / n)
}

/// Removes and returns the tile closest to `to`.
fn nearest(tiles: &mut Vec<TileId>, to: (f64, f64)) -> Option<TileId> {
    let distance = |id: &TileId| {
        let (x, y) = centre(*id);
        (x - to.0).powi(2) + (y - to.1).powi(2)
    };
    let k =
        (0..tiles.len()).min_by(|&a, &b| distance(&tiles[a]).total_cmp(&distance(&tiles[b])))?;
    Some(tiles.swap_remove(k))
}

fn valid(id: TileId, max_zoom: u8) -> bool {
    id.zoom <= max_zoom && id.x < 1 << id.zoom && id.y < 1 << id.zoom
}

/// The tile covering `id` at a coarser `zoom`.
pub fn ancestor(id: TileId, zoom: u8) -> TileId {
    let dz = id.zoom - zoom;
    TileId {
        x: id.x >> dz,
        y: id.y >> dz,
        zoom,
    }
}

/// When a tile was last drawn; `None` for pending and failed tiles.
fn age(state: &State) -> Option<u64> {
    match state {
        State::Ready(_, used) => Some(*used),
        _ => None,
    }
}

fn worker(
    http: &reqwest::blocking::Client,
    store: &Store,
    queue: &Queue,
    done: &Sender<(TileId, Option<Fetched>)>,
    ctx: &egui::Context,
) {
    let (lock, ready) = &**queue;
    loop {
        let (id, job) = {
            let mut pending = lock.lock().unwrap();
            loop {
                if let Some(next) = pending.next() {
                    break next;
                }
                pending = ready.wait(pending).unwrap();
            }
        };
        // Seeded tiles only go to disk; the map asks for them when it needs them.
        let fetched = match job {
            Job::Seed => {
                let _ = store.bytes(http, id);
                None
            }
            _ => store.fetch(http, id).ok(),
        };
        if job != Job::Shown {
            lock.lock().unwrap().background -= 1;
            ready.notify_all();
        }
        if job != Job::Seed {
            if done.send((id, fetched)).is_err() {
                return;
            }
            ctx.request_repaint();
        }
    }
}

/// Deletes the least recently used tiles, seeded ones apart, until they fit in `CACHE_LIMIT`.
pub fn trim_cache() {
    let mut tiles = Vec::new(); // (last use, bytes, path)
    let layers = fs::read_dir(data_dir().join("tuiles"))
        .into_iter()
        .flatten();
    for layer in layers.flatten() {
        let seeded = layer.file_name() == "planign";
        for zoom in fs::read_dir(layer.path()).into_iter().flatten().flatten() {
            let z: u8 = zoom
                .file_name()
                .to_str()
                .and_then(|z| z.parse().ok())
                .unwrap_or(0);
            if seeded && z <= SEED_ZOOM {
                continue;
            }
            for column in fs::read_dir(zoom.path()).into_iter().flatten().flatten() {
                for tile in fs::read_dir(column.path()).into_iter().flatten().flatten() {
                    if let Ok(meta) = tile.metadata() {
                        let used = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                        tiles.push((used, meta.len(), tile.path()));
                    }
                }
            }
        }
    }
    let mut total: u64 = tiles.iter().map(|t| t.1).sum();
    if total <= CACHE_LIMIT {
        return;
    }
    // Trim to 80 % of the limit, so this does not run again at every start.
    tiles.sort();
    for (_, len, path) in tiles {
        if total <= CACHE_LIMIT / 5 * 4 {
            break;
        }
        if fs::remove_file(path).is_ok() {
            total -= len;
        }
    }
}

/// Web Mercator tiles covering metropolitan France at `zoom`.
fn france_tiles(zoom: u8) -> impl Iterator<Item = TileId> {
    let n = (1u32 << zoom) as f64;
    let x = move |lon: f64| ((lon + 180.0) / 360.0 * n) as u32;
    let y = move |lat: f64| ((1.0 - lat.to_radians().tan().asinh() / PI) / 2.0 * n) as u32;
    let [lon0, lat0, lon1, lat1] = FRANCE;
    (x(lon0)..=x(lon1))
        .flat_map(move |tx| (y(lat1)..=y(lat0)).map(move |ty| TileId { x: tx, y: ty, zoom }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fetches_from_the_centre_outwards() {
        let tile = |x, y| TileId { x, y, zoom: 4 };
        let mut queue = vec![tile(0, 0), tile(7, 7), tile(5, 5), tile(10, 10)];
        let middle = centre(tile(7, 7));
        let order: Vec<_> = std::iter::from_fn(|| nearest(&mut queue, middle)).collect();
        assert_eq!(order, [tile(7, 7), tile(5, 5), tile(10, 10), tile(0, 0)]);
    }
}
