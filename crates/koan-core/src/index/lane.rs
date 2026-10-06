//! One scan at a time.
//!
//! A full scan and the folder watcher's rescans write the same rows, and two
//! at once over the same folders would race: one prunes what the other is
//! about to index, or both read every file. Every scan entry point in
//! `scanner` enters the lane first; a second waits on the lock, woken when the
//! first leaves, with no polling.
//!
//! An import of dropped files does not wait for the lane: it only adds, and a
//! scan's prune keeps what it adds, since the files are there. Someone dropped
//! an album to play it, and a scan of a large library can take minutes. It
//! waits only for forgetting a folder, which would otherwise remove rows it is
//! about to hand back; imports share `IMPORTS` and a forget holds it alone.
//!
//! The scan in the lane can be stopped from outside: by the person, through
//! [`cancel_all`], or by forgetting a folder it is scanning, through
//! [`cancel_under`]. Its cancel flag is the one in its [`ScanOptions`], made if
//! the caller gave none, so a caller's own flag and these reach the same scan.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::{Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

use super::scanner::ScanOptions;

static LANE: Mutex<()> = Mutex::new(());

/// Held shared by imports, and alone by a forget.
static IMPORTS: RwLock<()> = RwLock::new(());

/// The scan in the lane: the paths it covers, and how to stop it.
static RUNNING: Mutex<Option<Running>> = Mutex::new(None);

struct Running {
    roots: Vec<PathBuf>,
    cancel: Arc<AtomicBool>,
}

/// A scan's turn in the lane, held for as long as it runs.
pub(crate) struct Turn {
    _lane: MutexGuard<'static, ()>,
}

impl Drop for Turn {
    fn drop(&mut self) {
        *RUNNING.lock() = None;
    }
}

/// Wait for the lane, then hold it for a scan of `roots`. Gives `opts` a
/// cancel flag if it has none, and makes it the one [`cancel_all`] and
/// [`cancel_under`] set.
pub(crate) fn enter(roots: &[PathBuf], opts: &mut ScanOptions) -> Turn {
    let lane = LANE.lock();
    let cancel = opts
        .cancel
        .get_or_insert_with(|| Arc::new(AtomicBool::new(false)))
        .clone();
    *RUNNING.lock() = Some(Running {
        roots: roots.to_vec(),
        cancel,
    });
    Turn { _lane: lane }
}

/// Held while rows are being forgotten: no scan and no import underneath.
pub(crate) struct Quiet {
    _lane: MutexGuard<'static, ()>,
    _imports: RwLockWriteGuard<'static, ()>,
}

/// Wait for the lane and for every import, for changing what they read, such
/// as forgetting a folder, without one running underneath.
pub(crate) fn wait() -> Quiet {
    let lane = LANE.lock();
    Quiet {
        _lane: lane,
        _imports: IMPORTS.write(),
    }
}

/// Held by an import for as long as it writes. Waits only for a forget.
pub(crate) fn importing() -> RwLockReadGuard<'static, ()> {
    IMPORTS.read()
}

/// Stop the scan in the lane, whatever it covers. It keeps what it had
/// committed.
pub fn cancel_all() {
    if let Some(running) = RUNNING.lock().as_ref() {
        running.cancel.store(true, Ordering::Relaxed);
    }
}

/// Stop the scan in the lane if it covers `path` or anything under it.
pub fn cancel_under(path: &Path) {
    if let Some(running) = RUNNING.lock().as_ref()
        && running
            .roots
            .iter()
            .any(|root| root.starts_with(path) || path.starts_with(root))
    {
        running.cancel.store(true, Ordering::Relaxed);
    }
}
