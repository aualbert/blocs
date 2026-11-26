//! RGE ALTI 5 m: download per département, conversion to binary tiles, block extraction.
//!
//! On disk, each IGN dalle (1000 × 1000 points, 5 km) becomes `relief/<dep>/<gx>_<gy>.bin`:
//! raw little-endian u16 in decimetres above -500 m, rows south → north, `NODATA` for holes.
//! `gx, gy` are the global 5 m grid indices of the south-west point (Lambert-93 / 5).

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub const CELL: f64 = 5.0; // grid step (m)
const TILE: usize = 1000; // points per dalle side
const NODATA: u16 = u16::MAX;
const FEED: &str = "https://data.geopf.fr/telechargement/resource/RGEALTI";
const USER_AGENT: &str = concat!("blocs/", env!("CARGO_PKG_VERSION"));

pub enum Progress {
    Download { done: u64, total: u64 },
    Convert { done: usize, total: usize },
}

pub fn data_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| ".".into()).join("blocs")
}

/// Where a département's converted tiles live.
pub fn department_dir(dep: &str) -> PathBuf {
    relief_dir().join(dep)
}

fn relief_dir() -> PathBuf {
    data_dir().join("relief")
}

/// Départements already converted, e.g. `["D038"]`.
pub fn installed() -> Vec<String> {
    let Ok(entries) = fs::read_dir(relief_dir()) else {
        return Vec::new();
    };
    let mut deps: Vec<_> = entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    deps.sort();
    deps
}

/// Archive volumes `(url, bytes)` of the latest 5 m ASC edition for a département (`"D038"`).
pub fn archive_urls(dep: &str) -> Result<Vec<(String, u64)>> {
    let http = client()?;
    let feed = http
        .get(format!("{FEED}?zone={dep}"))
        .send()?
        .error_for_status()?
        .text()?;
    // Titles look like RGEALTI_2-0_5M_ASC_LAMB93-IGN69_D038_2020-11-13; the date sorts last.
    let edition = attr_values(&feed, "<title>", "<")
        .filter(|t| t.contains("_5M_ASC_") && t.contains(dep))
        .max_by_key(|t| t.rsplit('_').next().unwrap_or_default().to_owned())
        .ok_or(format!("pas de relief 5 m pour {dep}"))?;

    let entries = http
        .get(format!("{FEED}/{edition}"))
        .send()?
        .error_for_status()?
        .text()?;
    let urls = attr_values(&entries, "href=\"", "\"").filter(|u| u.contains("/download/"));
    let sizes = attr_values(&entries, "length=\"", "\"").map(|l| l.parse().unwrap_or(0));
    let mut volumes: Vec<_> = urls.map(str::to_owned).zip(sizes).collect();
    volumes.sort(); // multi-volume archives: .7z.001, .7z.002…
    Ok(volumes)
}

/// Downloads (resuming any partial file), converts, then deletes the archive.
pub fn install(dep: &str, mut progress: impl FnMut(Progress)) -> Result<()> {
    let volumes = archive_urls(dep)?;
    let total: u64 = volumes.iter().map(|(_, n)| n).sum();
    let archive = data_dir().join("download").join(format!("{dep}.7z"));
    fs::create_dir_all(archive.parent().unwrap())?;

    // Volumes are plain byte splits: appending them in order rebuilds the archive.
    // The server drops long transfers (seen after ~10 min), so resume until complete.
    let http = client()?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&archive)?;
    let mut done = file.metadata()?.len();
    let mut start = 0;
    for (url, len) in &volumes {
        let mut retries = 0;
        while done < start + len {
            let before = done;
            let result = fetch(&http, url, done - start, &mut file, |n| {
                done += n;
                progress(Progress::Download { done, total });
            });
            retries = if done > before { 0 } else { retries + 1 };
            if retries >= 5 {
                result?;
                return Err("le téléchargement n'avance plus".into());
            }
        }
        start += len;
    }
    drop(file);

    convert(&archive, &department_dir(dep), &mut progress)?;
    fs::remove_file(&archive)?;
    Ok(())
}

/// Streams the ASC dalles out of the 7z archive into binary tiles, without extracting to disk.
pub fn convert(archive: &Path, out: &Path, mut progress: impl FnMut(Progress)) -> Result<()> {
    let tmp = out.with_extension("tmp");
    fs::create_dir_all(&tmp)?;
    let mut reader = sevenz_rust2::ArchiveReader::open(archive, Default::default())?;
    let total = reader
        .archive()
        .files
        .iter()
        .filter(|f| f.name().ends_with(".asc"))
        .count();
    let mut done = 0;
    let mut text = String::new();
    reader.for_each_entries(|entry, data| {
        if entry.name().ends_with(".asc") {
            text.clear();
            data.read_to_string(&mut text)?;
            let (key, tile) = parse_asc(&text)
                .map_err(|e| std::io::Error::other(format!("{}: {e}", entry.name())))?;
            fs::write(
                tmp.join(format!("{}_{}.bin", key.0, key.1)),
                bytemuck::cast_slice(&tile),
            )?;
            done += 1;
            progress(Progress::Convert { done, total });
        }
        Ok(true)
    })?;
    // Only a complete conversion becomes visible.
    if out.exists() {
        fs::remove_dir_all(out)?;
    }
    fs::rename(&tmp, out)?;
    Ok(())
}

/// Parses an ESRI ASCII grid dalle into its grid key and encoded heights (rows south → north).
fn parse_asc(text: &str) -> Result<((i64, i64), Vec<u16>)> {
    let mut words = text.split_ascii_whitespace();
    let mut header = [0.0; 6]; // ncols nrows xllcorner yllcorner cellsize nodata
    for value in &mut header {
        words.next(); // key
        *value = words.next().ok_or("en-tête tronqué")?.parse()?;
    }
    let [ncols, nrows, xll, yll, cellsize, nodata] = header;
    if (ncols, nrows, cellsize) != (TILE as f64, TILE as f64, CELL) {
        return Err(format!("dalle inattendue {ncols}×{nrows} au pas {cellsize}").into());
    }
    // xll/yll are cell corners; points sit at cell centres.
    let key = (
        ((xll + CELL / 2.0) / CELL).round() as i64,
        ((yll + CELL / 2.0) / CELL).round() as i64,
    );

    let mut tile = vec![NODATA; TILE * TILE];
    for row in (0..TILE).rev() {
        // The file starts with the northern row.
        for h in &mut tile[row * TILE..(row + 1) * TILE] {
            let v: f64 = words.next().ok_or("dalle tronquée")?.parse()?;
            if v != nodata {
                *h = ((v + 500.0) * 10.0).round().clamp(0.0, (NODATA - 1) as f64) as u16;
            }
        }
    }
    Ok((key, tile))
}

/// A square of heights in metres, `heights[j * size + i]` with `i` east and `j` north.
pub struct Heightmap {
    pub size: usize,
    pub heights: Vec<f32>,
    /// Points with no data in any installed département, filled from neighbours.
    pub holes: usize,
}

impl Heightmap {
    /// Reads `size × size` points whose south-west corner is near Lambert-93 `(x, y)`.
    pub fn load(x: f64, y: f64, size: usize) -> Result<Self> {
        let (gx0, gy0) = ((x / CELL).round() as i64, (y / CELL).round() as i64);
        let mut grid = vec![NODATA; size * size];

        // Merge every dalle overlapping the block; départements share their border dalles.
        // Dalles are not aligned on round grid indices, so list them rather than guess names.
        let (size_i, t) = (size as i64, TILE as i64);
        for dep in installed() {
            for entry in fs::read_dir(department_dir(&dep))?.flatten() {
                let name = entry.file_name().into_string().unwrap_or_default();
                let Some((tx, ty)) = name.strip_suffix(".bin").and_then(|n| n.split_once('_'))
                else {
                    continue;
                };
                let (Ok(tx), Ok(ty)) = (tx.parse::<i64>(), ty.parse::<i64>()) else {
                    continue;
                };
                if tx >= gx0 + size_i || tx + t <= gx0 || ty >= gy0 + size_i || ty + t <= gy0 {
                    continue;
                }
                let tile: Vec<u16> = bytemuck::pod_collect_to_vec(&fs::read(entry.path())?);
                // Overlap in block coordinates.
                let (i0, i1) = ((tx - gx0).max(0), (tx + t - gx0).min(size_i));
                let (j0, j1) = ((ty - gy0).max(0), (ty + t - gy0).min(size_i));
                for j in j0..j1 {
                    for i in i0..i1 {
                        let v = tile[((gy0 + j - ty) * t + gx0 + i - tx) as usize];
                        if v != NODATA {
                            grid[(j * size_i + i) as usize] = v;
                        }
                    }
                }
            }
        }

        let holes = grid.iter().filter(|&&v| v == NODATA).count();
        if holes == grid.len() {
            return Err("aucun relief téléchargé ici".into());
        }
        let mut heights: Vec<f32> = grid
            .iter()
            .map(|&v| {
                if v == NODATA {
                    f32::NAN
                } else {
                    v as f32 / 10.0 - 500.0
                }
            })
            .collect();
        fill_holes(&mut heights, size);
        Ok(Self {
            size,
            heights,
            holes,
        })
    }

    pub fn range(&self) -> (f32, f32) {
        self.heights
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), &h| (lo.min(h), hi.max(h)))
    }
}

/// Fills NaN holes by repeatedly averaging valid 4-neighbours, growing inwards from the edges.
fn fill_holes(h: &mut [f32], size: usize) {
    let mut todo: Vec<usize> = (0..h.len()).filter(|&k| h[k].is_nan()).collect();
    while !todo.is_empty() {
        let mut filled = Vec::new();
        todo.retain(|&k| {
            let (i, j) = (k % size, k / size);
            let neighbours = [
                (i.wrapping_sub(1), j),
                (i + 1, j),
                (i, j.wrapping_sub(1)),
                (i, j + 1),
            ];
            let (sum, n) = neighbours
                .iter()
                .filter(|&&(a, b)| a < size && b < size)
                .map(|&(a, b)| h[b * size + a])
                .filter(|v| !v.is_nan())
                .fold((0.0, 0), |(s, n), v| (s + v, n + 1));
            if n > 0 {
                filled.push((k, sum / n as f32));
            }
            n == 0
        });
        for (k, v) in filled {
            h[k] = v;
        }
    }
}

fn client() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(None)
        .build()?)
}

/// Appends `url` from byte `offset` to `out`, reporting each chunk's size.
fn fetch(
    http: &reqwest::blocking::Client,
    url: &str,
    offset: u64,
    out: &mut fs::File,
    mut on_bytes: impl FnMut(u64),
) -> Result<()> {
    let mut resp = http
        .get(url)
        .header("Range", format!("bytes={offset}-"))
        .send()?
        .error_for_status()?;
    let mut buf = vec![0; 1 << 16];
    loop {
        let n = resp.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        out.write_all(&buf[..n])?;
        on_bytes(n as u64);
    }
}

/// Values between `start` and `end` markers, in document order.
fn attr_values<'a>(text: &'a str, start: &'a str, end: &'a str) -> impl Iterator<Item = &'a str> {
    text.split(start)
        .skip(1)
        .filter_map(move |s| s.split(end).next())
}
