//! Album covers for the web UI, share pages and Subsonic: an image beside the
//! tracks or the art embedded in them, resized to one of a few sizes, encoded
//! as JPEG, and kept.
//!
//! Reading embedded art means opening the audio file and parsing its tags, and
//! the art itself is often megabytes; doing that per tile made an albums grid
//! pull tens of megabytes and seconds of server time. Resized covers are kept
//! on disk under the config directory, keyed by the source file's path, size
//! and mtime so a re-tag or a replaced image is a new key. An album with no
//! art is remembered too, so a grid of them does not reopen every file on
//! every visit. The directory is held to `BUDGET`: every so many writes the
//! covers read least recently are removed, and so are the stale keys a re-tag
//! leaves and temporary files an interrupted write left behind.
//!
//! Nothing is held in memory: the kernel's page cache keeps hot files for
//! free and gives the memory back under pressure, where a cache of our own
//! would count against the process for as long as it runs. Decoding is the
//! expensive part — a full-size cover is tens of megabytes of pixels — so it
//! runs on a few threads of its own. A grid opening asks for hundreds of covers
//! at once; on the blocking pool each request would decode on its own thread,
//! and the allocator keeps what every one of those threads used.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use axum::body::Bytes;

use koan_core::db::queries::TrackRow;

/// The sizes a cover is served at. A request is rounded up to one of these,
/// which bounds what the cache can hold per album.
pub(crate) const SIZES: [u32; 4] = [200, 400, 800, 1200];
/// Grid tiles, at 2x.
pub(crate) const GRID: u32 = 400;
/// Headers, the player, and link previews.
pub(crate) const LARGE: u32 = 800;

/// How many of an album's tracks are opened looking for art before giving up.
const TRACKS_TRIED: usize = 3;
const JPEG_QUALITY: u8 = 85;

pub(crate) fn snap(size: Option<u32>) -> u32 {
    let size = size.unwrap_or(LARGE);
    SIZES
        .into_iter()
        .find(|s| *s >= size)
        .unwrap_or(SIZES[SIZES.len() - 1])
}

/// The threads covers are decoded and resized on.
static DECODE: LazyLock<rayon::ThreadPool> = LazyLock::new(|| {
    let threads = std::thread::available_parallelism().map_or(2, |n| n.get().clamp(1, 4));
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .thread_name(|i| format!("koan-covers-{i}"))
        .build()
        .expect("cover threads")
});

/// What the cover directory may hold before the least recently read go.
const BUDGET: u64 = 1024 * 1024 * 1024;
/// Writes between looks at the directory's size.
const SWEEP_EVERY: u64 = 256;
/// A hit moves a cover's mtime, which is its place in line for eviction, no
/// more than once in this long.
const TOUCH_AFTER: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);
/// A temporary file older than this was left by a write that never finished.
const TMP_LEFT: std::time::Duration = std::time::Duration::from_secs(60 * 60);

pub struct Covers {
    dir: PathBuf,
    budget: u64,
    writes: std::sync::atomic::AtomicU64,
    sweeping: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Covers {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            budget: BUDGET,
            writes: Default::default(),
            sweeping: Default::default(),
        }
    }

    /// Covers kept under koan's config directory.
    pub fn in_config_dir() -> Self {
        Self::new(koan_core::config::config_dir().join("covers"))
    }

    /// A cover from the first of `tracks` that has art, at `size` (one of
    /// `SIZES`), as JPEG. An image beside a track comes before the art
    /// embedded in it (see `koan_core::index::folder_art`). Blocking: call it
    /// off the async workers.
    pub(crate) fn cover(&self, tracks: &[TrackRow], size: u32) -> Option<Bytes> {
        let sources: Vec<(Source, String)> = tracks
            .iter()
            .filter_map(|t| crate::subsonic::track_file_path(t).map(PathBuf::from))
            .take(TRACKS_TRIED)
            .map(|p| {
                let source = match koan_core::index::folder_art::folder_cover(&p) {
                    Some(image) => Source::Image(image),
                    None => Source::Embedded(p),
                };
                let key = key(source.path(), size);
                (source, key)
            })
            .collect();
        // Keyed on the first candidate: that is the file whose art is shown
        // whenever it has any. An image added beside it is a new key, so a
        // remembered miss does not outlive it.
        let (_, first_key) = sources.first()?;
        if let Some(hit) = self.read_disk(first_key) {
            return hit;
        }
        let art = DECODE.install(|| {
            sources.iter().find_map(|(source, _)| {
                let bytes = match source {
                    Source::Image(p) => std::fs::read(p).ok(),
                    Source::Embedded(p) => koan_core::index::metadata::extract_cover_art(p),
                };
                bytes.and_then(|bytes| encode(&bytes, size))
            })
        });
        self.write_disk(first_key, art.as_deref());
        art.map(Bytes::from)
    }

    /// `Some(None)` is a remembered miss. A hit is marked as read, so the
    /// covers in use are the last evicted.
    fn read_disk(&self, key: &str) -> Option<Option<Bytes>> {
        use std::io::Read as _;
        let mut file = std::fs::File::open(self.dir.join(key)).ok()?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).ok()?;
        let now = std::time::SystemTime::now();
        if file
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|t| now.duration_since(t).unwrap_or_default() > TOUCH_AFTER)
        {
            let _ = file.set_modified(now);
        }
        Some((!bytes.is_empty()).then(|| Bytes::from(bytes)))
    }

    /// An empty file records that there is no art. Failure to write is only a
    /// lost cache entry. Requests that miss the same cover together each write
    /// a temporary file of their own, so the one renamed into place last is
    /// whole rather than another's half-written file.
    fn write_disk(&self, key: &str, art: Option<&[u8]>) {
        use std::sync::atomic::Ordering;
        let _ = std::fs::create_dir_all(&self.dir);
        let n = self.writes.fetch_add(1, Ordering::Relaxed);
        if n % SWEEP_EVERY == 0 && !self.sweeping.swap(true, Ordering::AcqRel) {
            let (dir, budget, sweeping) = (self.dir.clone(), self.budget, self.sweeping.clone());
            std::thread::spawn(move || {
                sweep(&dir, budget);
                sweeping.store(false, Ordering::Release);
            });
        }
        let tmp = self
            .dir
            .join(format!("{key}.{}-{n}.tmp", std::process::id()));
        if std::fs::write(&tmp, art.unwrap_or_default()).is_ok()
            && std::fs::rename(&tmp, self.dir.join(key)).is_err()
        {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

/// Hold `dir` to `budget` bytes, removing the covers read least recently
/// until it is a tenth under, and every temporary file older than `TMP_LEFT`.
fn sweep(dir: &Path, budget: u64) {
    let now = std::time::SystemTime::now();
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut kept: Vec<(std::time::SystemTime, u64, PathBuf)> = Vec::new();
    for entry in read.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        let modified = meta.modified().unwrap_or(now);
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "tmp") {
            if now.duration_since(modified).unwrap_or_default() > TMP_LEFT {
                let _ = std::fs::remove_file(&path);
            }
            continue;
        }
        kept.push((modified, meta.len(), path));
    }
    let mut total: u64 = kept.iter().map(|(_, len, _)| len).sum();
    if total <= budget {
        return;
    }
    kept.sort_by_key(|(modified, _, _)| *modified);
    let target = budget - budget / 10;
    for (_, len, path) in kept {
        if total <= target {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= len;
        }
    }
}

/// Where a cover is read from.
enum Source {
    /// An image file beside the track.
    Image(PathBuf),
    /// The art embedded in the track itself.
    Embedded(PathBuf),
}

impl Source {
    fn path(&self) -> &Path {
        match self {
            Self::Image(p) | Self::Embedded(p) => p,
        }
    }
}

/// The source's path, size and mtime, and the size asked for: a re-tagged or
/// replaced file is a different key, and stale entries are simply never read.
fn key(path: &Path, size: u32) -> String {
    let meta = std::fs::metadata(path).ok();
    let mtime = meta
        .as_ref()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    let len = meta.map_or(0, |m| m.len());
    let digest = md5::compute(format!("{}\0{len}\0{mtime}", path.display()));
    format!("{digest:x}-{size}.jpg")
}

/// Fit within `size` pixels on a side and encode as JPEG. A JPEG already
/// within bounds is passed through untouched.
fn encode(bytes: &[u8], size: u32) -> Option<Vec<u8>> {
    use image::GenericImageView as _;
    let img = image::load_from_memory(bytes).ok()?;
    let (w, h) = img.dimensions();
    if w.max(h) <= size && bytes.starts_with(&[0xFF, 0xD8]) {
        return Some(bytes.to_vec());
    }
    let img = if w.max(h) > size {
        img.thumbnail(size, size)
    } else {
        img
    };
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY)
        .encode_image(&img.to_rgb8())
        .ok()?;
    Some(out)
}

/// The ensō on a `surface` square, as PNG: what a record without art is
/// served, so every client draws an image. The apps' placeholder geometry —
/// the mark fitted inside a 26% inset, stroked at 4.5% of the side — in the
/// dark appearance's `muted` on `surface`, which reads as a dark sleeve in a
/// light client too. Drawn once per size and kept.
pub(crate) fn placeholder(size: u32) -> Bytes {
    static DRAWN: [std::sync::OnceLock<Bytes>; SIZES.len()] =
        [const { std::sync::OnceLock::new() }; SIZES.len()];
    let size = snap(Some(size));
    let slot = SIZES
        .iter()
        .position(|s| *s == size)
        .expect("snap answers one of SIZES");
    DRAWN[slot].get_or_init(|| draw_placeholder(size)).clone()
}

fn draw_placeholder(size: u32) -> Bytes {
    // The dark appearance's `--surface` (#2a2a2a) and `--muted` (#919191),
    // from site/src/theme.css.
    const SURFACE: [f32; 3] = [42.0, 42.0, 42.0];
    const MUTED: [f32; 3] = [145.0, 145.0, 145.0];
    // `just icons` writes each glyph as absolute `M`/`L` polylines, which is
    // all this reads; the test below fails if that changes.
    // The glyph's 24-unit box, with the ring spanning 2.5–21.5, scaled so the
    // ring is as wide as the apps' (60% of the inset box).
    let side = size as f32;
    let scale = side * 0.48 * 0.6 / 19.0;
    let points: Vec<(f32, f32)> = crate::icon::ENSO
        .strokes
        .split(['M', 'L'])
        .filter_map(|p| {
            let (x, y) = p.split_once(' ')?;
            let (x, y): (f32, f32) = (x.trim().parse().ok()?, y.trim().parse().ok()?);
            Some((
                side / 2.0 + (x - 12.0) * scale,
                side / 2.0 + (y - 12.0) * scale,
            ))
        })
        .collect();
    let radius = side * 0.045 / 2.0;
    let mut img = image::RgbImage::from_pixel(size, size, image::Rgb(SURFACE.map(|c| c as u8)));
    // Only the pixels the stroke can reach are measured.
    let (mut lo, mut hi) = (side, 0.0f32);
    for &(x, y) in &points {
        lo = lo.min(x.min(y) - radius - 1.0);
        hi = hi.max(x.max(y) + radius + 1.0);
    }
    let span = (lo.max(0.0) as u32)..(hi.min(side) as u32);
    for y in span.clone() {
        for x in span.clone() {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let distance = points
                .windows(2)
                .map(|w| {
                    let ((ax, ay), (bx, by)) = (w[0], w[1]);
                    let (dx, dy) = (bx - ax, by - ay);
                    let t =
                        (((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
                    (px - ax - t * dx).hypot(py - ay - t * dy)
                })
                .fold(f32::INFINITY, f32::min);
            let cover = (radius + 0.5 - distance).clamp(0.0, 1.0);
            if cover > 0.0 {
                let mix = |i: usize| (SURFACE[i] + (MUTED[i] - SURFACE[i]) * cover).round() as u8;
                img.put_pixel(x, y, image::Rgb([mix(0), mix(1), mix(2)]));
            }
        }
    }
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png)
        .expect("PNG encodes in memory");
    Bytes::from(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_snap_up_to_a_served_size() {
        assert_eq!(snap(Some(1)), 200);
        assert_eq!(snap(Some(400)), 400);
        assert_eq!(snap(Some(401)), 800);
        assert_eq!(snap(Some(99_999)), 1200);
        assert_eq!(snap(None), LARGE);
    }

    #[test]
    fn the_enso_is_one_absolute_polyline() {
        let strokes = crate::icon::ENSO.strokes;
        assert!(strokes.starts_with('M'));
        assert_eq!(strokes.matches('M').count(), 1);
        assert!(
            strokes
                .chars()
                .all(|c| matches!(c, 'M' | 'L' | ' ' | '.' | '-' | '0'..='9'))
        );
    }

    #[test]
    fn the_placeholder_is_the_size_asked_with_the_mark_inside_the_inset() {
        let png = placeholder(300);
        let img = image::load_from_memory(&png).unwrap().to_rgb8();
        assert_eq!(img.dimensions(), (400, 400));
        let surface = image::Rgb([42, 42, 42]);
        assert_eq!(*img.get_pixel(0, 0), surface);
        assert_eq!(*img.get_pixel(200, 200), surface, "the ring is open");
        let inked: Vec<_> = img
            .enumerate_pixels()
            .filter(|(_, _, p)| **p != surface)
            .collect();
        assert!(!inked.is_empty());
        let inset = (400.0 * 0.26) as u32;
        assert!(
            inked
                .iter()
                .all(|(x, y, _)| (inset..400 - inset).contains(x)
                    && (inset..400 - inset).contains(y))
        );
    }

    #[test]
    fn writers_racing_on_one_cover_leave_it_whole() {
        let dir = tempfile::tempdir().unwrap();
        let covers = Covers::new(dir.path().to_path_buf());
        let arts: Vec<Vec<u8>> = (0..8u8).map(|i| vec![i; 256 * 1024]).collect();
        for _ in 0..20 {
            std::thread::scope(|s| {
                for art in &arts {
                    s.spawn(|| covers.write_disk("k-400.jpg", Some(art)));
                }
            });
            let got = std::fs::read(dir.path().join("k-400.jpg")).unwrap();
            assert!(arts.contains(&got), "a torn write of {} bytes", got.len());
        }
        let left: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(left.len(), 1, "no temporary files left behind");
    }

    #[test]
    fn the_directory_is_held_to_its_budget_oldest_read_first() {
        let dir = tempfile::tempdir().unwrap();
        let day = std::time::Duration::from_secs(24 * 60 * 60);
        let now = std::time::SystemTime::now();
        let put = |name: &str, len: usize, age: u32| {
            let path = dir.path().join(name);
            std::fs::write(&path, vec![0u8; len]).unwrap();
            std::fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(now - day * age)
                .unwrap();
        };
        for i in 0..10u32 {
            put(&format!("c{i}-400.jpg"), 1000, 10 - i);
        }
        put("c0-400.jpg.1-2.tmp", 10, 2);
        put("c9-400.jpg.1-3.tmp", 10, 0);
        sweep(dir.path(), 5600);
        let mut left: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                "c5-400.jpg",
                "c6-400.jpg",
                "c7-400.jpg",
                "c8-400.jpg",
                "c9-400.jpg",
                "c9-400.jpg.1-3.tmp"
            ],
            "the five read longest ago go, and the abandoned temporary file"
        );
    }

    #[test]
    fn covers_are_bounded_jpegs() {
        use image::GenericImageView as _;
        let png = {
            let mut out = std::io::Cursor::new(Vec::new());
            image::DynamicImage::new_rgba8(2400, 1200)
                .write_to(&mut out, image::ImageFormat::Png)
                .unwrap();
            out.into_inner()
        };
        let out = encode(&png, 400).unwrap();
        assert!(out.starts_with(&[0xFF, 0xD8]));
        assert_eq!(
            image::load_from_memory(&out).unwrap().dimensions(),
            (400, 200)
        );

        let small = encode(&png, 400).unwrap();
        assert_eq!(
            encode(&small, 800).unwrap(),
            small,
            "a small JPEG passes through"
        );
    }
}
