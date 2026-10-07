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
pub fn sync_remote(
    db: &Database,
    client: &SubsonicClient,
    walk: Walk,
    url: &str,
    username: &str,
    progress: &(dyn Fn(crate::remote::sync::SyncProgress) + Sync),
) -> Result<Synced, crate::remote::sync::SyncError> {
    use crate::remote::sync;
    // One at a time. An automatic sync still reconciling favourites when a
    // sync was asked for wrote under it, and each fought the other for the
    // write lock. The second waits for the first, and then has little left
    // to do.
    static SYNCING: parking_lot::Mutex<()> = parking_lot::Mutex::new(());
    let _one_at_a_time = SYNCING.lock();

    let walked = sync::library_version(db, url);
    let modified = client.library_modified(walked);
    crate::remote::refusal::observe(client.auth(), &modified);
    let version = modified
        .inspect_err(|e| log::debug!("library version unavailable: {e}"))
        .ok()
        .flatten();
    let library = if walk == Walk::IfChanged && version.is_some() && version == walked {
        log::info!("library unchanged on the server; not walked");
        sync::SyncResult::default()
    } else {
        let library = sync::sync_library(db, client, url, username, progress).inspect_err(|e| {
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
    Ok(Synced {
        library,
        favourites: reconcile_favourites(db, client),
        playlists: crate::playlists::reconcile_playlists(db, client, url, username),
        history: crate::remote::history::reconcile(db, client, url, username),
        dsp: crate::remote::dsp_sync::reconcile(db, client, url),
    })
}
