//! Album thumbnails kept on disk, already shrunk to what a tile draws, so a
//! cover seen once comes back as a small JPEG decode instead of a read and
//! decode of the whole embedded picture. The budget is fixed: once it is
//! full the oldest written go first, like a ring.

use std::fs;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use image::DynamicImage;
use image::codecs::jpeg::JpegEncoder;
use parking_lot::Mutex;

const BUDGET: u64 = 256 * 1024 * 1024;
/// What a trim leaves, so the next one is a long way off.
const TRIMMED_TO: u64 = BUDGET / 4 * 3;
const TRIM_EVERY: usize = 64;
const QUALITY: u8 = 85;

static WRITES: AtomicUsize = AtomicUsize::new(0);
static TRIMMING: Mutex<()> = Mutex::new(());

fn dir() -> PathBuf {
    koan_core::config::Config::cached()
        .cache_dir()
        .join("thumbs")
}

/// The thumbnail kept for the album with this uid, unless the file it was made
/// from has changed since.
pub fn read(uid: &str, source: Option<&Path>) -> Option<DynamicImage> {
    read_in(&dir(), uid, source)
}

pub fn write(uid: &str, img: &DynamicImage) {
    let dir = dir();
    if write_in(&dir, uid, img).is_some()
        && WRITES
            .fetch_add(1, Ordering::Relaxed)
            .is_multiple_of(TRIM_EVERY)
    {
        trim(&dir, BUDGET, TRIMMED_TO);
    }
}

fn read_in(dir: &Path, uid: &str, source: Option<&Path>) -> Option<DynamicImage> {
    let path = dir.join(format!("{uid}.jpg"));
    let made = fs::metadata(&path).ok()?.modified().ok()?;
    let changed = source
        .and_then(|p| fs::metadata(p).ok())
        .and_then(|m| m.modified().ok());
    if changed.is_some_and(|changed| changed > made) {
        return None;
    }
    image::open(path).ok()
}

fn write_in(dir: &Path, uid: &str, img: &DynamicImage) -> Option<()> {
    koan_core::config::create_cache_dir(dir).ok()?;
    let tmp = dir.join(format!("{uid}.tmp"));
    let written = (|| {
        let mut out = BufWriter::new(fs::File::create(&tmp).ok()?);
        JpegEncoder::new_with_quality(&mut out, QUALITY)
            .encode_image(&img.to_rgb8())
            .ok()?;
        fs::rename(&tmp, dir.join(format!("{uid}.jpg"))).ok()
    })();
    if written.is_none() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

/// Deletes the oldest thumbnails until what is left fits in `keep`, when the
/// directory holds more than `budget`. One trim at a time; a second caller
/// leaves it to the first.
fn trim(dir: &Path, budget: u64, keep: u64) {
    let Some(_one) = TRIMMING.try_lock() else {
        return;
    };
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<_> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "jpg"))
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((m.modified().ok()?, m.len(), e.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    if total <= budget {
        return;
    }
    files.sort();
    for (_, len, path) in files {
        if total <= keep {
            break;
        }
        if fs::remove_file(path).is_ok() {
            total -= len;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cover(shade: u8) -> DynamicImage {
        DynamicImage::ImageRgb8(image::RgbImage::from_pixel(64, 64, image::Rgb([shade; 3])))
    }

    #[test]
    fn a_written_thumbnail_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        write_in(dir.path(), "a", &cover(90)).unwrap();
        let img = read_in(dir.path(), "a", None).unwrap();
        assert_eq!((img.width(), img.height()), (64, 64));
        assert!(read_in(dir.path(), "b", None).is_none());
    }

    #[test]
    fn a_source_changed_since_is_not_served_stale() {
        let dir = tempfile::tempdir().unwrap();
        write_in(dir.path(), "a", &cover(90)).unwrap();
        let source = dir.path().join("track.flac");
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(&source, b"x").unwrap();
        assert!(read_in(dir.path(), "a", Some(&source)).is_none());
    }

    #[test]
    fn a_trim_drops_the_oldest_first() {
        let dir = tempfile::tempdir().unwrap();
        for uid in ["old", "mid", "new"] {
            write_in(dir.path(), uid, &cover(10)).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let size = fs::metadata(dir.path().join("new.jpg")).unwrap().len();
        trim(dir.path(), size * 2, size);
        assert!(read_in(dir.path(), "old", None).is_none());
        assert!(read_in(dir.path(), "mid", None).is_none());
        assert!(read_in(dir.path(), "new", None).is_some());
    }
}
