//! Deciding whether to walk the remote server, and walking it.

use crate::db::connection::Database;
use crate::remote::client::SubsonicClient;

use super::*;

/// Everything a sync is.
#[derive(Debug, Default)]
pub struct Synced {
    pub library: crate::remote::sync::SyncResult,
    pub favourites: FavouriteSync,
    pub playlists: crate::playlists::PlaylistSync,
    pub history: crate::remote::history::HistorySync,
    pub dsp: crate::remote::dsp_sync::DspSync,
}

/// Whether a sync walks the server's library.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Walk {
    /// Somebody asked: walk it, whatever the server says.
    Always,
    /// On koan's own account — a server saying something changed, a timer, a
    /// favourite on another device: walk it only if the server's library has
    /// moved since the last complete walk.
    IfChanged,
}

/// Held by a sync for as long as it runs. One at a time: an automatic sync
/// still reconciling favourites when a sync was asked for wrote under it, and
/// each fought the other for the write lock. The second waits for the first,
/// and then has little left to do.
static SYNCING: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// Wait for the sync running, if any, to finish, and keep any other from
/// starting until the guard is dropped.
pub(crate) fn hold_syncs() -> parking_lot::MutexGuard<'static, ()> {
    SYNCING.lock()
}

/// Pull the library, then reconcile favourites and playlists.
///
/// One function because there are four callers — the app, the CLI, the GraphQL
/// job and koan's own auto-sync — and each must sync the same things.
///
/// The library comes first: favourites and playlists both name tracks by the
/// server's ids, and neither can find a track the library has not seen yet.
/// Walking it is most of a sync's cost — fifty thousand tracks written for
/// every walk — so `Walk::IfChanged` asks the server first. Its version is
/// read before the walk, so a change landing mid-walk leaves it newer than
/// what is recorded, and the next sync walks again. Favourites and playlists
/// are a request or two each, and are reconciled every time.
///
/// Signing out of the server, or in to another, stops it with
/// `SyncError::Cancelled`, whether it is running or still waiting for the
/// one before it: see `remote::sync::cancel`.
pub fn sync_remote(
    db: &Database,
    client: &SubsonicClient,
    walk: Walk,
    url: &str,
    username: &str,
    progress: &(dyn Fn(crate::remote::sync::SyncProgress) + Sync),
) -> Result<Synced, crate::remote::sync::SyncError> {
    use crate::remote::sync;
    let running = sync::Running::enter(url, username);
    let cancelled = || {
        if running.cancel.load(std::sync::atomic::Ordering::Relaxed) {
            Err(sync::SyncError::Cancelled)
        } else {
            Ok(())
        }
    };
    let _one_at_a_time = SYNCING.lock();
    cancelled()?;

    let walked = sync::library_version(db, url);
    let modified = client.library_modified(walked);
    crate::remote::refusal::observe(client.auth(), &modified);
    let version = modified
        .inspect_err(|e| log::debug!("library version unavailable: {e}"))
        .ok()
        .flatten();
    let library = if walk == Walk::IfChanged && version.is_some() && version == walked {
        log::info!("library unchanged on the server; not walked");
        let now = crate::clock::unix_secs();
        sync::update_last_sync(db, url, username, now)?;
        sync::SyncResult::default()
    } else {
        let library = sync::sync_library(db, client, url, username, progress, &running.cancel)
            .inspect_err(|e| {
                if let sync::SyncError::Subsonic(e) = e {
                    crate::remote::refusal::observe_error(client.auth(), e);
                }
            })?;
        if library.is_complete()
            && let Some(version) = version
        {
            sync::set_library_version(db, url, username, version)?;
        }
        library
    };
    cancelled()?;
    let favourites =
        reconcile_favourites(db, client).map_err(crate::db::connection::DbError::from)?;
    cancelled()?;
    let playlists = crate::playlists::reconcile_playlists(db, client, url, username);
    cancelled()?;
    let history = crate::remote::history::reconcile(db, client, url, username);
    cancelled()?;
    Ok(Synced {
        library,
        favourites,
        playlists,
        history,
        dsp: crate::remote::dsp_sync::reconcile(db, client, url),
    })
}
