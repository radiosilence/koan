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
    /// Whether the folder is on a filesystem whose every change passes through
    /// this machine's kernel. A network share is not: another machine writing
    /// to it raises no event here. See [`is_local`].
    pub local: bool,
}

impl WatchedRoot {
    /// The folder as it is now, or `None` when there is nothing there to watch.
    pub fn resolve(path: &Path) -> Option<Self> {
        let identity = identity(path)?;
        Some(Self {
            path: path.to_path_buf(),
            real: path.canonicalize().unwrap_or_else(|_| path.to_path_buf()),
            identity: Some(identity),
            local: is_local(path),
        })
    }
}

/// The nearest existing ancestor of `path`, `path` itself included, as the
/// kernel spells it, and `path` spelled from there. What to watch for a path
/// that is not there yet: creating it, or any directory on the way to it, is
/// an event on that ancestor.
pub fn nearest_existing(path: &Path) -> Option<(PathBuf, PathBuf)> {
    let mut at = path;
    loop {
        if let Ok(real) = at.canonicalize() {
            let rest = path.strip_prefix(at).unwrap_or(Path::new(""));
            let spelled = if rest.as_os_str().is_empty() {
                real.clone()
            } else {
                real.join(rest)
            };
            return Some((real, spelled));
        }
        at = at.parent()?;
    }
}

/// Whether every change to the filesystem holding `path` is made through this
/// machine's kernel, and so heard by FSEvents or inotify. A network share
/// (NFS, SMB, AFP, 9P, Ceph) is not, and neither is a FUSE mount, which may
/// be one: a file another machine adds to it is found only by looking.
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos"))]
pub fn is_local(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return true;
    };
    // SAFETY: `c` is a NUL-terminated path; `fs` is written by statfs.
    let mut fs: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut fs) } != 0 {
        return true;
    }
    fs.f_flags & libc::MNT_LOCAL as u32 != 0
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn is_local(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    const REMOTE: [u32; 10] = [
        0x6969,     // NFS
        0x517B,     // SMB
        0xFF534D42, // CIFS
        0xFE534D42, // SMB2
        0x65735546, // FUSE
        0x00C36400, // Ceph
        0x01021997, // 9P
        0x5346414F, // AFS
        0x564C,     // NCP
        0x73757245, // Coda
    ];
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return true;
    };
    // SAFETY: `c` is a NUL-terminated path; `fs` is written by statfs.
    let mut fs: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut fs) } != 0 {
        return true;
    }
    // Compared as the kernel's 32 bits: `f_type` is signed on some targets,
    // where widening would sign-extend the magic numbers with the top bit set.
    !REMOTE.contains(&(fs.f_type as u32))
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "tvos",
    target_os = "linux",
    target_os = "android"
)))]
pub fn is_local(_path: &Path) -> bool {
    true
}

/// Call `changed` whenever a filesystem is mounted or unmounted anywhere on
/// the machine, from a thread that sleeps in the kernel between times, until
/// it returns `false`. Nothing is called on a platform that cannot say.
///
/// A library folder on a volume unplugged and plugged back in is a different
/// directory at the same path, which the watch on the old one never hears of.
pub fn on_mount_change(changed: impl Fn() -> bool + Send + 'static) {
    let spawned = std::thread::Builder::new()
        .name("koan-mounts".into())
        .spawn(move || {
            if let Err(e) = wait_for_mounts(changed) {
                log::debug!("mount changes: {e}");
            }
        });
    if let Err(e) = spawned {
        log::warn!("could not follow mounts: {e}");
    }
}

/// `EVFILT_FS` is raised on every mount, unmount and volume state change.
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos"))]
fn wait_for_mounts(changed: impl Fn() -> bool) -> std::io::Result<()> {
    // SAFETY: kqueue takes no arguments; its descriptor is owned from here.
    let kq = unsafe { libc::kqueue() };
    if kq < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: as above.
    let kq = unsafe { <std::os::fd::OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(kq) };
    let kq = std::os::fd::AsRawFd::as_raw_fd(&kq);
    // SAFETY: a zeroed kevent is a valid value of the C struct.
    let mut add: libc::kevent = unsafe { std::mem::zeroed() };
    add.filter = libc::EVFILT_FS;
    add.flags = libc::EV_ADD | libc::EV_CLEAR;
    // SAFETY: one live change and no event buffer.
    if unsafe { libc::kevent(kq, &add, 1, std::ptr::null_mut(), 0, std::ptr::null()) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    loop {
        // SAFETY: as above.
        let mut out: libc::kevent = unsafe { std::mem::zeroed() };
        // SAFETY: no changes, one live event slot, no timeout.
        let n = unsafe { libc::kevent(kq, std::ptr::null(), 0, &mut out, 1, std::ptr::null()) };
        if n < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        if n > 0 && !changed() {
            return Ok(());
        }
    }
}

/// `/proc/self/mountinfo` polls as exceptional whenever the mount table
/// changes, and is read through again to clear it.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn wait_for_mounts(changed: impl Fn() -> bool) -> std::io::Result<()> {
    use std::io::{Read, Seek};
    use std::os::fd::AsRawFd;
    let mut table = std::fs::File::open("/proc/self/mountinfo")?;
    let mut buf = Vec::new();
    table.read_to_end(&mut buf)?;
    loop {
        let mut fd = libc::pollfd {
            fd: table.as_raw_fd(),
            events: libc::POLLPRI | libc::POLLERR,
            revents: 0,
        };
        // SAFETY: one live pollfd, no timeout.
        if unsafe { libc::poll(&mut fd, 1, -1) } < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        table.rewind()?;
        buf.clear();
        table.read_to_end(&mut buf)?;
        if !changed() {
            return Ok(());
        }
    }
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "tvos",
    target_os = "linux",
    target_os = "android"
)))]
fn wait_for_mounts(_changed: impl Fn() -> bool) -> std::io::Result<()> {
    Ok(())
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

    #[test]
    fn a_missing_path_is_awaited_from_its_nearest_existing_ancestor() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().canonicalize().unwrap();
        let wanted = dir.path().join("a").join("b");
        assert_eq!(
            nearest_existing(&wanted),
            Some((real.clone(), real.join("a").join("b")))
        );
        std::fs::create_dir(dir.path().join("a")).unwrap();
        assert_eq!(
            nearest_existing(&wanted),
            Some((real.join("a"), real.join("a").join("b")))
        );
        std::fs::create_dir(&wanted).unwrap();
        let here = real.join("a").join("b");
        assert_eq!(nearest_existing(&wanted), Some((here.clone(), here)));
    }

    #[test]
    fn a_temporary_directory_is_local() {
        let dir = tempfile::tempdir().unwrap();
        assert!(is_local(dir.path()));
        assert!(WatchedRoot::resolve(dir.path()).unwrap().local);
    }

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
