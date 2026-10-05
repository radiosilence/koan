//! Covers kept as image files beside the tracks: `cover.jpg`, `folder.png`,
//! `front.webp`.
//!
//! The order is Navidrome's default `CoverArtPriority` — `cover.*`, `folder.*`,
//! `front.*`, then the art embedded in the files — so a library moved from
//! Navidrome shows the covers it showed there. Names match without regard to
//! case. An album split into disc folders (`CD1`, `Disc 2`) keeps its cover in
//! the album's folder above them, which is looked in when the disc folder
//! has none.
//!
//! Every cover lookup asks for this, so a directory is listed once and the
//! answer kept against the directory's mtime: adding, removing or renaming a
//! file moves that, and nothing else needs to. A cover replaced in place
//! leaves the directory alone and keeps its name, which is all that is kept.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::SystemTime;

use parking_lot::Mutex;

const NAMES: [&str; 3] = ["cover", "folder", "front"];
const EXTENSIONS: [&str; 4] = ["jpg", "jpeg", "png", "webp"];
/// Directories remembered before the memo starts again. A library's worth of
/// albums is well within it; the bound only stops it growing without end.
const REMEMBERED: usize = 16_384;

type Memo = HashMap<PathBuf, (Option<SystemTime>, Option<PathBuf>)>;
static MEMO: LazyLock<Mutex<Memo>> = LazyLock::new(Default::default);

/// Whether `path` is named as a cover image could be.
pub fn is_cover_file(path: &Path) -> bool {
    rank(path).is_some()
}

/// The cover image beside `track`, if its folder (or, for a disc folder, the
/// album's folder above it) has one.
pub fn folder_cover(track: &Path) -> Option<PathBuf> {
    let dir = track.parent()?;
    in_dir(dir).or_else(|| {
        dir.file_name()
            .is_some_and(is_disc_folder)
            .then(|| dir.parent().and_then(in_dir))
            .flatten()
    })
}

/// The cover for `track`: an image beside it first, then the art embedded in
/// the file.
pub fn cover_art(track: &Path) -> Option<Vec<u8>> {
    folder_cover(track)
        .and_then(|p| std::fs::read(p).ok())
        .filter(|bytes| !bytes.is_empty())
        .or_else(|| super::metadata::extract_cover_art(track))
}

fn in_dir(dir: &Path) -> Option<PathBuf> {
    let mtime = std::fs::metadata(dir).and_then(|m| m.modified()).ok();
    if let Some((seen, cover)) = MEMO.lock().get(dir)
        && *seen == mtime
        && mtime.is_some()
    {
        return cover.clone();
    }
    let cover = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file() || t.is_symlink()))
        .filter_map(|e| {
            let path = e.path();
            rank(&path).map(|r| (r, path))
        })
        .min()
        .map(|(_, path)| path);
    let mut memo = MEMO.lock();
    if memo.len() >= REMEMBERED {
        memo.clear();
    }
    memo.insert(dir.to_path_buf(), (mtime, cover.clone()));
    cover
}

/// Where a file named like a cover sits in the order: by name, then by
/// extension, so `cover.jpg` beats `cover.png` beats `folder.jpg`.
fn rank(path: &Path) -> Option<(usize, usize)> {
    let stem = path.file_stem()?.to_str()?.to_ascii_lowercase();
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some((
        NAMES.iter().position(|n| *n == stem)?,
        EXTENSIONS.iter().position(|e| *e == ext)?,
    ))
}

/// `CD1`, `CD 2`, `Disc 1`, `disk02`: a folder holding one disc of an album.
fn is_disc_folder(name: &std::ffi::OsStr) -> bool {
    let name = name.to_string_lossy().to_ascii_lowercase();
    let rest = ["disc", "disk", "cd"]
        .iter()
        .find_map(|p| name.strip_prefix(p))
        .map(|r| r.trim_start_matches([' ', '_', '-', '.']));
    rest.is_some_and(|r| !r.is_empty() && r.chars().all(|c| c.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn names_match_in_navidromes_order_regardless_of_case() {
        let tmp = tempfile::tempdir().unwrap();
        let album = tmp.path().join("Album");
        let track = album.join("01.flac");
        touch(&track, b"");
        assert_eq!(folder_cover(&track), None);

        touch(&album.join("FRONT.PNG"), b"f");
        touch(&album.join("Folder.jpg"), b"d");
        touch(&album.join("notes.jpg"), b"x");
        assert_eq!(folder_cover(&track), Some(album.join("Folder.jpg")));

        touch(&album.join("cover.webp"), b"w");
        touch(&album.join("Cover.JPEG"), b"c");
        assert_eq!(folder_cover(&track), Some(album.join("Cover.JPEG")));
    }

    #[test]
    fn a_disc_folder_falls_back_to_the_albums() {
        let tmp = tempfile::tempdir().unwrap();
        let album = tmp.path().join("Album");
        let track = album.join("CD 2").join("01.flac");
        touch(&track, b"");
        touch(&album.join("cover.jpg"), b"c");
        assert_eq!(folder_cover(&track), Some(album.join("cover.jpg")));

        // An album that only sits inside an artist's folder does not borrow it.
        let other = tmp.path().join("Artist").join("Album").join("01.flac");
        touch(&other, b"");
        touch(&tmp.path().join("Artist").join("cover.jpg"), b"a");
        assert_eq!(folder_cover(&other), None);

        assert!(is_disc_folder("Disc 1".as_ref()));
        assert!(is_disc_folder("cd02".as_ref()));
        assert!(is_disc_folder("Disk_3".as_ref()));
        assert!(!is_disc_folder("CDs".as_ref()));
        assert!(!is_disc_folder("Discography".as_ref()));
    }

    #[test]
    fn a_cover_added_later_is_found() {
        let tmp = tempfile::tempdir().unwrap();
        let album = tmp.path().join("Album");
        let track = album.join("01.flac");
        touch(&track, b"");
        assert_eq!(folder_cover(&track), None, "a miss, remembered");

        // A directory's mtime has a resolution of a second on some systems.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        touch(&album.join("folder.jpg"), b"d");
        assert_eq!(folder_cover(&track), Some(album.join("folder.jpg")));
    }

    #[test]
    fn the_image_beside_a_track_wins_over_embedded_art() {
        let tmp = tempfile::tempdir().unwrap();
        let track = tmp.path().join("Album").join("01.flac");
        touch(&track, b"not audio");
        assert_eq!(cover_art(&track), None);
        touch(
            &tmp.path().join("Album").join("folder.jpg"),
            b"\xFF\xD8jpeg",
        );
        assert_eq!(cover_art(&track), Some(b"\xFF\xD8jpeg".to_vec()));
    }

    #[test]
    fn only_cover_names_count() {
        assert!(is_cover_file(Path::new("/m/a/Cover.JPG")));
        assert!(is_cover_file(Path::new("/m/a/front.webp")));
        assert!(!is_cover_file(Path::new("/m/a/back.jpg")));
        assert!(!is_cover_file(Path::new("/m/a/cover.txt")));
        assert!(!is_cover_file(Path::new("/m/a/cover")));
    }
}
