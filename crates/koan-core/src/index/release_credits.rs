//! Each artist on a release whose files carry its MusicBrainz id but credit
//! its artists only as one string, as files tagged before taggers wrote
//! `ALBUMARTISTS` do.
//!
//! After a scan, the releases worth asking about are looked up one a second,
//! as MusicBrainz asks, on a thread of their own. Each answer is kept, a miss
//! included, so a release is fetched once; the tracks tagged with it are then
//! derived again and listed under each artist the release credits.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::db::connection::Database;
use crate::db::queries::{Credit, sources};
use crate::remote::musicbrainz::{self, MusicBrainzError};

static RUNNING: AtomicBool = AtomicBool::new(false);

/// Off until a front end turns it on, so tests that scan a database on disk
/// never reach MusicBrainz.
static ENABLED: AtomicBool = AtomicBool::new(false);

/// Let scans look releases up. Called once by the apps and the CLI.
pub fn enable() {
    ENABLED.store(true, Ordering::Release);
}

/// Where the database `db` lives, for [`after_scan`]; none in memory.
pub(crate) fn database_path(db: &Database) -> Option<PathBuf> {
    db.conn.path().filter(|p| !p.is_empty()).map(PathBuf::from)
}

/// Look up the releases still to credit, in the background. A pass already
/// running picks up what this scan added when it reads the list again.
pub(crate) fn after_scan(path: PathBuf) {
    if !ENABLED.load(Ordering::Acquire) || RUNNING.swap(true, Ordering::AcqRel) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("koan-credits".into())
        .spawn(move || {
            let db = match Database::open_existing(&path) {
                Ok(db) => db,
                Err(e) => {
                    log::warn!("release credits: {e}");
                    RUNNING.store(false, Ordering::Release);
                    return;
                }
            };
            // A scan that ended while this pass was finishing found it
            // running and left its releases to it: look once more after
            // standing down, and carry on if there are any.
            loop {
                run(&db);
                RUNNING.store(false, Ordering::Release);
                let more = sources::releases_to_credit(&db.conn).is_ok_and(|p| !p.is_empty());
                if !more || RUNNING.swap(true, Ordering::AcqRel) {
                    return;
                }
            }
        });
    if spawned.is_err() {
        RUNNING.store(false, Ordering::Release);
    }
}

fn run(db: &Database) {
    let http = musicbrainz::default_client();
    loop {
        let pending = match sources::releases_to_credit(&db.conn) {
            Ok(p) => p,
            Err(e) => {
                log::warn!("release credits: {e}");
                return;
            }
        };
        if pending.is_empty() {
            return;
        }
        for release in pending {
            crate::quiet::wait_until_awake();
            crate::artist_info::throttle();
            let credits = match musicbrainz::release_artists(&http, &release) {
                Ok(found) => Some(
                    found
                        .into_iter()
                        .map(|(name, mbid)| Credit {
                            name,
                            mbid: Some(mbid),
                            remote_id: None,
                        })
                        .collect::<Vec<_>>(),
                ),
                Err(MusicBrainzError::NotFound) => None,
                // A request MusicBrainz will never answer, such as a
                // malformed id, is a miss like a 404; otherwise every pass
                // would stop at it.
                Err(MusicBrainzError::Http(e))
                    if e.is_decode()
                        || e.status().is_some_and(|s| {
                            s.is_client_error() && s != reqwest::StatusCode::TOO_MANY_REQUESTS
                        }) =>
                {
                    log::info!("release credits: {release}: {e}");
                    None
                }
                // Unreachable or refusing: try again after the next scan.
                Err(e) => {
                    log::info!("release credits: {release}: {e}");
                    return;
                }
            };
            let adopted = crate::db::queries::atomically(&db.conn, || {
                sources::adopt_release_credits(
                    &db.conn,
                    &release,
                    credits.as_deref(),
                    crate::artist_info::now(),
                )
            });
            if let Err(e) = adopted {
                log::warn!("release credits: {release}: {e}");
                return;
            }
        }
    }
}
