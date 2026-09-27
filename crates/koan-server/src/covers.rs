//! Album covers for the web UI and share pages: embedded art, resized to one
//! of a few sizes, encoded as JPEG, and kept.
//!
//! Reading embedded art means opening the audio file and parsing its tags, and
//! the art itself is often megabytes; doing that per tile made an albums grid
//! pull tens of megabytes and seconds of server time. Resized covers are kept
//! on disk under the config directory, keyed by the source file's path, size
//! and mtime so a re-tag is a new key, and the hottest are also held in
//! memory. An album with no art is remembered too, so a grid of them does not
//! reopen every file on every visit.

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use axum::body::Bytes;

use koan_core::db::queries::TrackRow;
use lru::LruCache;

/// The sizes a cover is served at. A request is rounded up to one of these,
/// which bounds what the cache can hold per album.
pub(crate) const SIZES: [u32; 4] = [200, 400, 800, 1200];
/// Grid tiles, at 2x.
pub(crate) const GRID: u32 = 400;
/// Headers, the player, and link previews.
pub(crate) const LARGE: u32 = 800;

const MEMORY_ENTRIES: usize = 512;
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

pub struct Covers {
    dir: PathBuf,
    /// `None` is an album with no art.
    memory: Mutex<LruCache<String, Option<Bytes>>>,
}

impl Covers {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            memory: Mutex::new(LruCache::new(
                NonZeroUsize::new(MEMORY_ENTRIES).expect("non-zero"),
            )),
        }
    }

    /// Covers kept under koan's config directory.
    pub fn in_config_dir() -> Self {
        Self::new(koan_core::config::config_dir().join("covers"))
    }

    /// A cover from the first of `tracks` that has art, at `size` (one of
    /// `SIZES`), as JPEG. Blocking: call it off the async workers.
    pub(crate) fn cover(&self, tracks: &[TrackRow], size: u32) -> Option<Bytes> {
        let sources: Vec<(PathBuf, String)> = tracks
            .iter()
            .filter_map(|t| crate::subsonic::track_file_path(t).map(PathBuf::from))
            .take(TRACKS_TRIED)
            .map(|p| {
                let key = key(&p, size);
                (p, key)
            })
            .collect();
        // Keyed on the first candidate: that is the file whose art is shown
        // whenever it has any.
        let (_, first_key) = sources.first()?;
        let first_key = first_key.clone();
        if let Some(hit) = self.memory.lock().ok()?.get(&first_key).cloned() {
            return hit;
        }
        let found = self.read_disk(&first_key).or_else(|| {
            let art = sources.iter().find_map(|(p, _)| {
                koan_core::index::metadata::extract_cover_art(p)
                    .and_then(|bytes| encode(&bytes, size))
            });
            self.write_disk(&first_key, art.as_deref());
            Some(art.map(Bytes::from))
        })?;
        if let Ok(mut memory) = self.memory.lock() {
            memory.put(first_key, found.clone());
        }
        found
    }

    /// `Some(None)` is a remembered miss.
    fn read_disk(&self, key: &str) -> Option<Option<Bytes>> {
        let bytes = std::fs::read(self.dir.join(key)).ok()?;
        Some((!bytes.is_empty()).then(|| Bytes::from(bytes)))
    }

    /// An empty file records that there is no art. Failure to write is only a
    /// lost cache entry.
    fn write_disk(&self, key: &str, art: Option<&[u8]>) {
        let _ = std::fs::create_dir_all(&self.dir);
        let tmp = self.dir.join(format!("{key}.tmp"));
        if std::fs::write(&tmp, art.unwrap_or_default()).is_ok() {
            let _ = std::fs::rename(&tmp, self.dir.join(key));
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
