//! Album thumbnails kept on disk, already shrunk to what a tile draws, so a
//! cover seen once comes back as a small JPEG decode instead of a read and
//! decode of the whole embedded picture. The budget is fixed: once it is
//! full the least recently used go first.

use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use image::DynamicImage;
use image::codecs::jpeg::JpegEncoder;
use parking_lot::Mutex;

const BUDGET: u64 = 256 * 1024 * 1024;
/// What a trim leaves, so the next one is a long way off.
const TRIMMED_TO: u64 = BUDGET / 4 * 3;
const TRIM_EVERY: usize = 64;
const QUALITY: u8 = 85;
/// A use refreshes a file's age at most this often.
const TOUCH_AFTER: Duration = Duration::from_secs(60 * 60);
const ORPHAN_AFTER: Duration = Duration::from_secs(10 * 60);

static WRITES: AtomicUsize = AtomicUsize::new(0);
static TRIMMING: Mutex<()> = Mutex::new(());

/// Beside the config, not inside the download cache, whose size and clearing
/// would otherwise count these files as music.
fn dir() -> PathBuf {
    koan_core::config::config_dir().join("thumbs")
}

/// What a thumbnail was made from: the newest change to the track's file or
/// to the folder it is in, which is where a `cover.jpg` would come and go. An
/// album with no file here has only the server, whose art is looked at again
/// each week.
pub fn stamp(source: Option<&Path>) -> u64 {
    const WEEK: u64 = 7 * 24 * 60 * 60;
    let secs = |t: std::io::Result<SystemTime>| {
        t.ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
    };
    match source {
        Some(path) => {
            let file = secs(fs::metadata(path).and_then(|m| m.modified()));
            let folder = path
                .parent()
                .and_then(|p| secs(fs::metadata(p).and_then(|m| m.modified())));
            file.max(folder).unwrap_or(0)
        }
        None => secs(Ok(SystemTime::now())).unwrap_or(0) / WEEK,
    }
}

/// The thumbnail kept for the album with this uid and source stamp. A hit
/// counts as a use, so what is looked at stays and what is not goes first.
pub fn read(uid: &str, stamp: u64) -> Option<DynamicImage> {
    read_in(&dir(), uid, stamp)
}

pub fn write(uid: &str, stamp: u64, img: &DynamicImage) {
    let dir = dir();
    if write_in(&dir, uid, stamp, img).is_some()
        && WRITES
            .fetch_add(1, Ordering::Relaxed)
            .is_multiple_of(TRIM_EVERY)
    {
        trim(&dir, BUDGET, TRIMMED_TO);
    }
}

fn name(uid: &str, stamp: u64) -> String {
    format!("{uid}-{stamp}.jpg")
}

fn read_in(dir: &Path, uid: &str, stamp: u64) -> Option<DynamicImage> {
    let path = dir.join(name(uid, stamp));
    let img = image::open(&path).ok()?;
    let used = fs::metadata(&path).ok()?.modified().ok()?;
    if used.elapsed().is_ok_and(|age| age > TOUCH_AFTER)
        && let Ok(file) = fs::File::options().write(true).open(&path)
    {
        let _ = file.set_modified(SystemTime::now());
    }
    Some(img)
}

fn write_in(dir: &Path, uid: &str, stamp: u64, img: &DynamicImage) -> Option<()> {
    koan_core::config::create_cache_dir(dir).ok()?;
    let tmp = dir.join(format!("{uid}-{stamp}.{}.tmp", std::process::id()));
    let written = (|| {
        let mut out = BufWriter::new(fs::File::create(&tmp).ok()?);
        JpegEncoder::new_with_quality(&mut out, QUALITY)
            .encode_image(&img.to_rgb8())
            .ok()?;
        out.flush().ok()?;
        fs::rename(&tmp, dir.join(name(uid, stamp))).ok()
    })();
    if written.is_none() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

/// Deletes the least recently used thumbnails until what is left fits in
/// `keep`, when the directory holds more than `budget`, and any temporary
/// file a crashed writer left behind. One trim at a time; a second caller
/// leaves it to the first.
fn trim(dir: &Path, budget: u64, keep: u64) {
    let Some(_one) = TRIMMING.try_lock() else {
        return;
    };
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut files = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        match path.extension().and_then(|x| x.to_str()) {
            Some("jpg") => files.push((modified, meta.len(), path)),
            Some("tmp") if modified.elapsed().is_ok_and(|age| age > ORPHAN_AFTER) => {
                let _ = fs::remove_file(path);
            }
            _ => {}
        }
    }
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
        write_in(dir.path(), "a", 5, &cover(90)).unwrap();
        let img = read_in(dir.path(), "a", 5).unwrap();
        assert_eq!((img.width(), img.height()), (64, 64));
        assert!(read_in(dir.path(), "b", 5).is_none());
    }

    #[test]
    fn a_changed_source_is_not_served_stale() {
        let dir = tempfile::tempdir().unwrap();
        write_in(dir.path(), "a", 5, &cover(90)).unwrap();
        assert!(read_in(dir.path(), "a", 6).is_none());
    }

    #[test]
    fn a_folder_change_moves_the_stamp() {
        let dir = tempfile::tempdir().unwrap();
        let track = dir.path().join("track.flac");
        fs::write(&track, b"x").unwrap();
        let before = stamp(Some(&track));
        let later = SystemTime::now() + Duration::from_secs(120);
        fs::File::open(dir.path())
            .unwrap()
            .set_modified(later)
            .unwrap();
        assert!(stamp(Some(&track)) > before);
    }

    #[test]
    fn a_trim_drops_the_least_recently_used_first() {
        let dir = tempfile::tempdir().unwrap();
        for uid in ["old", "mid", "new"] {
            write_in(dir.path(), uid, 1, &cover(10)).unwrap();
            std::thread::sleep(Duration::from_millis(20));
        }
        let size = fs::metadata(dir.path().join(name("new", 1))).unwrap().len();
        let long_ago = SystemTime::now() - Duration::from_secs(3 * 60 * 60);
        for uid in ["old", "mid"] {
            fs::File::options()
                .write(true)
                .open(dir.path().join(name(uid, 1)))
                .unwrap()
                .set_modified(long_ago - Duration::from_secs(if uid == "old" { 60 } else { 0 }))
                .unwrap();
        }
        // Looked at again, so it outlives "new".
        assert!(read_in(dir.path(), "old", 1).is_some());
        trim(dir.path(), size * 2, size);
        assert!(read_in(dir.path(), "mid", 1).is_none());
        assert!(read_in(dir.path(), "new", 1).is_none());
        assert!(read_in(dir.path(), "old", 1).is_some());
    }
}
