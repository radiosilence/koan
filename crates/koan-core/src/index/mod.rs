pub mod folder_art;
mod id3v2_pictures;
pub mod lane;
pub mod metadata;
pub mod playlist_files;
pub mod scanner;
pub mod spelling;
pub mod watch;

/// Whether `path` is known not to exist, as opposed to unreachable.
///
/// `try_exists` answers `Ok(false)` through a symlink whose target is gone,
/// which is what a symlinked network share looks like while it is away: every
/// file under it would read as deleted. So a path counts as missing only when
/// the nearest part of it that is there is a real directory, or a symlink that
/// still leads somewhere. An error at any step is "cannot tell".
pub fn known_missing(path: &std::path::Path) -> bool {
    if !matches!(path.try_exists(), Ok(false)) {
        return false;
    }
    for ancestor in path.ancestors() {
        match std::fs::symlink_metadata(ancestor) {
            // The path itself being there at all means a dangling link.
            Ok(_) if ancestor == path => return false,
            Ok(meta) => {
                return !meta.file_type().is_symlink() || matches!(ancestor.try_exists(), Ok(true));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return false,
        }
    }
    false
}
