//! What a filesystem event under the library folders means for the index.
//!
//! The watcher hears about everything that happens in the library — Syncthing
//! shuffling its temp files, access times, a download writing its `.part`, a
//! cover image replaced. Most of it cannot change a single row. This decides,
//! per event path, whether it can and which directory a scan has to look at.

use std::path::{Path, PathBuf};

use notify::event::{CreateKind, EventKind, ModifyKind, RemoveKind};

use super::folder_art::is_cover_file;
use super::metadata::is_audio_file;
use super::playlist_files::is_playlist_file;

/// A library folder being watched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchedRoot {
    /// As configured — what the rows in `tracks` start with.
    pub path: PathBuf,
    /// As the kernel spells it. FSEvents reports canonical paths, so a folder
    /// configured through a symlink hears its events under another prefix.
    pub real: PathBuf,
    /// Device and inode of the folder, so a volume unmounted and mounted again
    /// at the same path reads as a different folder.
    pub identity: Option<(u64, u64)>,
}

impl WatchedRoot {
    /// The folder as it is now, or `None` when there is nothing there to watch.
    pub fn resolve(path: &Path) -> Option<Self> {
        let identity = identity(path)?;
        Some(Self {
            path: path.to_path_buf(),
            real: path.canonicalize().unwrap_or_else(|_| path.to_path_buf()),
            identity: Some(identity),
        })
    }
}

#[cfg(unix)]
fn identity(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).ok()?;
    meta.is_dir().then(|| (meta.dev(), meta.ino()))
}

#[cfg(not(unix))]
fn identity(path: &Path) -> Option<(u64, u64)> {
    std::fs::metadata(path).ok()?.is_dir().then_some((0, 0))
}

/// The directory a scan must cover for this event path, if the event can
/// change the index at all.
///
/// An audio file's, playlist's or cover image's own directory, a directory
/// itself, and a path that has gone without saying what it was: that may have
/// been a directory of tracks, and a scan of a directory that is not there only
/// removes the rows under it. The path is given back under the root's
/// configured spelling.
pub fn scan_target(kind: &EventKind, path: &Path, roots: &[WatchedRoot]) -> Option<PathBuf> {
    if !can_change_index(kind) {
        return None;
    }
    let (root, rel) = roots.iter().find_map(|root| {
        path.strip_prefix(&root.path)
            .or_else(|_| path.strip_prefix(&root.real))
            .ok()
            .map(|rel| (root, rel))
    })?;
    // The folder itself coming and going is a mount, and `WatchedRoot::identity`
    // is what notices that.
    if rel.as_os_str().is_empty() || rel.components().any(|c| is_ignored(c.as_os_str())) {
        return None;
    }
    let path = root.path.join(rel);

    if is_audio_file(&path) || is_playlist_file(&path) || is_cover_file(&path) {
        return path.parent().map(Path::to_path_buf);
    }
    match std::fs::metadata(&path) {
        Ok(meta) if meta.is_dir() => Some(path),
        Ok(_) => None,
        Err(_) if is_file_event(kind) => None,
        Err(_) => Some(path),
    }
}

/// Access and metadata-only events: reading a file, touching its permissions,
/// its xattrs, its Finder info. None of it changes what a scan would read.
fn can_change_index(kind: &EventKind) -> bool {
    !matches!(
        kind,
        EventKind::Access(_) | EventKind::Modify(ModifyKind::Metadata(_)) | EventKind::Other
    )
}

/// Events that say the path was a file, which a non-audio file being is no
/// business of the index's.
fn is_file_event(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(CreateKind::File)
            | EventKind::Remove(RemoveKind::File)
            | EventKind::Modify(ModifyKind::Data(_))
    )
}

/// Names the scanner never indexes under: what Syncthing keeps beside the
/// music (`.stfolder`, `.stversions`, `.stignore`, `.syncthing.*.tmp`, and
/// `~syncthing~*.tmp` from Windows peers), what macOS and version control leave
/// in folders, and downloads still in progress.
///
/// Not every name with a leading dot: artists and records have them, as
/// "...And You Will Know Us by the Trail of Dead" and ".5: The Gray Chapter"
/// do, and organize keeps them.
pub(crate) fn is_ignored(name: &std::ffi::OsStr) -> bool {
    let name = name.to_string_lossy();
    matches!(
        name.as_ref(),
        ".stfolder"
            | ".stversions"
            | ".stignore"
            | ".DS_Store"
            | ".AppleDouble"
            | ".Spotlight-V100"
            | ".fseventsd"
            | ".TemporaryItems"
            | ".DocumentRevisions-V100"
            | ".git"
    ) || name.starts_with(".syncthing.")
        || name.starts_with("~syncthing~")
        // AppleDouble: a file's resource fork, beside it on a non-Mac volume.
        || name.starts_with("._")
        || name.starts_with(".Trash")
        || name.ends_with(".part")
        || name.ends_with(".tmp")
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{AccessKind, DataChange, MetadataKind, RenameMode};

    fn root(dir: &Path) -> Vec<WatchedRoot> {
        vec![WatchedRoot::resolve(dir).unwrap()]
    }

    #[test]
    fn an_audio_file_names_its_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let album = tmp.path().join("Artist/Album");
        std::fs::create_dir_all(&album).unwrap();
        let track = album.join("01.flac");
        std::fs::write(&track, b"").unwrap();

        let roots = root(tmp.path());
        for kind in [
            EventKind::Create(CreateKind::File),
            EventKind::Modify(ModifyKind::Data(DataChange::Content)),
            EventKind::Modify(ModifyKind::Name(RenameMode::Any)),
            EventKind::Remove(RemoveKind::File),
        ] {
            assert_eq!(scan_target(&kind, &track, &roots), Some(album.clone()));
        }
        // Gone by the time the event is read: still its directory.
        assert_eq!(
            scan_target(
                &EventKind::Remove(RemoveKind::File),
                &album.join("02.flac"),
                &roots
            ),
            Some(album.clone())
        );
    }

    #[test]
    fn access_and_metadata_change_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let track = tmp.path().join("a.flac");
        std::fs::write(&track, b"").unwrap();
        let roots = root(tmp.path());
        for kind in [
            EventKind::Access(AccessKind::Any),
            EventKind::Modify(ModifyKind::Metadata(MetadataKind::Any)),
            EventKind::Modify(ModifyKind::Metadata(MetadataKind::Extended)),
            EventKind::Other,
        ] {
            assert_eq!(scan_target(&kind, &track, &roots), None, "{kind:?}");
        }
    }

    #[test]
    fn syncthing_hidden_and_partial_files_are_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = root(tmp.path());
        let create = EventKind::Create(CreateKind::Any);
        for rel in [
            ".stfolder",
            ".stversions/Artist/Album/01~20260101-000000.flac",
            "Artist/Album/.syncthing.01.flac.tmp",
            "Artist/Album/~syncthing~01.flac.tmp",
            "Artist/Album/01.flac.part",
            "Artist/.DS_Store",
            "Artist/Album/._01.flac",
            ".Trashes/501/01.flac",
        ] {
            assert_eq!(
                scan_target(&create, &tmp.path().join(rel), &roots),
                None,
                "{rel}"
            );
        }
    }

    #[test]
    fn a_name_starting_with_a_dot_can_be_music() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = root(tmp.path());
        let album = tmp.path().join("Britney Spears/...Baby One More Time");
        let create = EventKind::Create(CreateKind::File);
        assert_eq!(
            scan_target(&create, &album.join("01.flac"), &roots),
            Some(album)
        );
    }

    #[test]
    fn other_files_are_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let album = tmp.path().join("Album");
        std::fs::create_dir_all(&album).unwrap();
        std::fs::write(album.join("back.jpg"), b"").unwrap();
        let roots = root(tmp.path());

        let any = EventKind::Modify(ModifyKind::Any);
        assert_eq!(scan_target(&any, &album.join("back.jpg"), &roots), None);
        // Removed, and the event says it was a file.
        assert_eq!(
            scan_target(
                &EventKind::Remove(RemoveKind::File),
                &album.join("notes.txt"),
                &roots
            ),
            None
        );
    }

    #[test]
    fn a_cover_image_names_its_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let album = tmp.path().join("Album");
        std::fs::create_dir_all(&album).unwrap();
        std::fs::write(album.join("Folder.JPG"), b"").unwrap();
        let roots = root(tmp.path());

        for kind in [
            EventKind::Create(CreateKind::File),
            EventKind::Modify(ModifyKind::Data(DataChange::Content)),
            EventKind::Remove(RemoveKind::File),
        ] {
            assert_eq!(
                scan_target(&kind, &album.join("Folder.JPG"), &roots),
                Some(album.clone())
            );
        }
    }

    #[test]
    fn a_directory_names_itself_and_a_vanished_one_too() {
        let tmp = tempfile::tempdir().unwrap();
        let album = tmp.path().join("Artist/Album (Vol. 2)");
        std::fs::create_dir_all(&album).unwrap();
        let roots = root(tmp.path());

        assert_eq!(
            scan_target(&EventKind::Create(CreateKind::Folder), &album, &roots),
            Some(album.clone())
        );
        let gone = tmp.path().join("Artist/Old Album");
        for kind in [
            EventKind::Remove(RemoveKind::Folder),
            EventKind::Modify(ModifyKind::Name(RenameMode::Any)),
        ] {
            assert_eq!(scan_target(&kind, &gone, &roots), Some(gone.clone()));
        }
    }

    #[test]
    fn the_root_itself_and_paths_outside_it_are_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let music = tmp.path().join("music");
        std::fs::create_dir_all(&music).unwrap();
        let roots = root(&music);
        let any = EventKind::Any;
        assert_eq!(scan_target(&any, &music, &roots), None);
        assert_eq!(scan_target(&any, &tmp.path().join("x.flac"), &roots), None);
    }

    /// FSEvents reports the canonical path; the rows use the configured one.
    #[cfg(unix)]
    #[test]
    fn events_under_the_real_path_come_back_under_the_configured_one() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("disk/music");
        std::fs::create_dir_all(real.join("Album")).unwrap();
        let link = tmp.path().join("music");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let roots = root(&link);

        let event = roots[0].real.join("Album/01.flac");
        assert_eq!(
            scan_target(&EventKind::Create(CreateKind::File), &event, &roots),
            Some(link.join("Album"))
        );
    }
}
