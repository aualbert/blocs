//! RGE ALTI 5 m: download per département, conversion to binary tiles, block extraction.
//!
//! On disk, each IGN dalle (1000 × 1000 points, 5 km) becomes `relief/<dep>/<gx>_<gy>.bin`:
//! raw little-endian u16 in decimetres above -500 m, rows south → north, `NODATA` for holes.
//! `gx, gy` are the global 5 m grid indices of the south-west point (Lambert-93 / 5).

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::ign::client;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub const CELL: f64 = 5.0; // grid step (m)
const TILE: usize = 1000; // points per dalle side
/// Grid index of the first dalle row: IGN dalles start 5 m north of round kilometres.
const TILE_ROW0: i64 = 1;
const NODATA: u16 = u16::MAX;
const FEED: &str = "https://data.geopf.fr/telechargement/resource/RGEALTI";

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

/// Lambert-93 squares `[x0, y0, x1, y1]` of the dalles holding points of a rectangle.
pub fn dalles([x0, y0, x1, y1]: [f64; 4]) -> Vec<[f64; 4]> {
    let t = TILE as i64;
    // First grid index of the dalle containing grid index `g`.
    let start = |g: i64, row0: i64| (g - row0).div_euclid(t) * t + row0;
    let index = |v: f64| (v / CELL).round() as i64;
    let side = (TILE - 1) as f64 * CELL;
    let xs = (start(index(x0), 0)..=index(x1)).step_by(TILE);
    xs.flat_map(|gx| {
        (start(index(y0), TILE_ROW0)..=index(y1))
            .step_by(TILE)
            .map(move |gy| {
                let (x, y) = (gx as f64 * CELL, gy as f64 * CELL);
                [x, y, x + side, y + side]
            })
    })
    .collect()
}

/// Fewest départements to download so that every dalle is covered, given the
/// départements touching each dalle. An archive holds every dalle touching its
/// département, so a dalle comes with any one of them; installed ones count first.
pub fn cover(dalles: &[Vec<String>], installed: &[String]) -> Vec<String> {
    let mut left: Vec<&Vec<String>> = dalles
        .iter()
        .filter(|deps| !deps.is_empty() && !deps.iter().any(|d| installed.contains(d)))
        .collect();
    let mut chosen = Vec::new();
    while !left.is_empty() {
        // The département covering the most remaining dalles; ties go to the lowest code.
        let mut counts: Vec<(usize, &String)> = Vec::new();
        for dep in left.iter().flat_map(|deps| deps.iter()) {
            match counts.iter_mut().find(|(_, d)| *d == dep) {
                Some((n, _)) => *n += 1,
                None => counts.push((1, dep)),
            }
        }
        let (_, best) = counts
            .into_iter()
            .max_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(a.1)))
            .unwrap();
        let best = best.clone();
        left.retain(|deps| !deps.contains(&best));
        chosen.push(best);
    }
    chosen
}

/// Total archive size in bytes.
pub fn archive_size(dep: &str) -> Result<u64> {
    Ok(archive_urls(dep)?.iter().map(|(_, n)| n).sum())
}

/// Downloads (resuming any partial file), converts, then deletes the archive.
/// Setting `pause` stops the download with an error; calling again resumes it.
pub fn install(dep: &str, pause: &AtomicBool, mut progress: impl FnMut(Progress)) -> Result<()> {
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
            let result = fetch(&http, url, done - start, &mut file, pause, |n| {
                done += n;
                progress(Progress::Download { done, total });
            });
            if pause.load(Ordering::Relaxed) {
                return Err("téléchargement en pause".into());
            }
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

/// A grid of heights in metres, `heights[j * nx + i]` with `i` east and `j` north.
pub struct Heightmap {
    pub nx: usize,
    pub ny: usize,
    pub heights: Vec<f32>,
    /// Points with no data in any installed département, filled from neighbours.
    pub holes: usize,
}

impl Heightmap {
    /// Reads `nx × ny` points whose south-west corner is near Lambert-93 `(x, y)`.
    pub fn load(x: f64, y: f64, nx: usize, ny: usize) -> Result<Self> {
        let (gx0, gy0) = ((x / CELL).round() as i64, (y / CELL).round() as i64);
        let mut grid = vec![NODATA; nx * ny];

        // Merge every dalle overlapping the block; départements share their border dalles.
        // Dalles are not aligned on round grid indices, so list them rather than guess names.
        let (nx_i, ny_i, t) = (nx as i64, ny as i64, TILE as i64);
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
                if tx >= gx0 + nx_i || tx + t <= gx0 || ty >= gy0 + ny_i || ty + t <= gy0 {
                    continue;
                }
                let tile: Vec<u16> = bytemuck::pod_collect_to_vec(&fs::read(entry.path())?);
                // Overlap in block coordinates.
                let (i0, i1) = ((tx - gx0).max(0), (tx + t - gx0).min(nx_i));
                let (j0, j1) = ((ty - gy0).max(0), (ty + t - gy0).min(ny_i));
                for j in j0..j1 {
                    for i in i0..i1 {
                        let v = tile[((gy0 + j - ty) * t + gx0 + i - tx) as usize];
                        if v != NODATA {
                            grid[(j * nx_i + i) as usize] = v;
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
        fill_holes(&mut heights, nx);
        Ok(Self {
            nx,
            ny,
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
fn fill_holes(h: &mut [f32], nx: usize) {
    let mut todo: Vec<usize> = (0..h.len()).filter(|&k| h[k].is_nan()).collect();
    while !todo.is_empty() {
        let mut filled = Vec::new();
        todo.retain(|&k| {
            let (i, j) = (k % nx, k / nx);
            let neighbours = [
                (i.wrapping_sub(1), j),
                (i + 1, j),
                (i, j.wrapping_sub(1)),
                (i, j + 1),
            ];
            let (sum, n) = neighbours
                .iter()
                .filter(|&&(a, b)| a < nx && b < h.len() / nx)
                .map(|&(a, b)| h[b * nx + a])
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

/// Appends `url` from byte `offset` to `out`, reporting each chunk's size, until done or paused.
fn fetch(
    http: &reqwest::blocking::Client,
    url: &str,
    offset: u64,
    out: &mut fs::File,
    pause: &AtomicBool,
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
        if n == 0 || pause.load(Ordering::Relaxed) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dalles_of_the_clecy_frame() {
        // 10 km frame on Clécy: 3 × 3 dalles, keyed as on disk (87000_1373001…).
        let d = dalles([439_350.0, 6_868_050.0, 449_350.0, 6_878_050.0]);
        assert_eq!(d.len(), 9);
        assert_eq!(d[0], [435_000.0, 6_865_005.0, 439_995.0, 6_870_000.0]);
    }

    #[test]
    fn calvados_alone_covers_clecy() {
        let v = |c: &[&str]| c.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let mut dalles = vec![v(&["D014"]); 6];
        dalles.extend(vec![v(&["D014", "D061"]); 3]);
        assert_eq!(cover(&dalles, &[]), ["D014"]);
        // With the Orne installed, its 3 dalles are covered, the rest still needs the Calvados.
        assert_eq!(cover(&dalles, &v(&["D061"])), ["D014"]);
        assert!(cover(&dalles, &v(&["D014"])).is_empty());
    }
}
