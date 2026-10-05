//! Play history shared through a koan server.
//!
//! A koan server keeps each account's plays: every device signed in scrobbles
//! what it heard there. On a server offering `koanHistory`, that record is
//! also what the account's devices show. Each reads it after a cursor and
//! adopts the plays other devices made, leaving out its own, which come back
//! from the server a few seconds either side of when this device recorded
//! them (`queries::SAME_PLAY_SECS`). A play forgotten on any device is
//! forgotten on the server, which keeps a record of it, so every device
//! forgets it too.
//!
//! A device still records every play it starts, skips included, and the
//! server only the ones heard long enough to count (`player::history`). So a
//! device's list holds its own skips and other devices' plays, never their
//! skips.
//!
//! What a device sends — scrobbles, forgettings — waits in the
//! `history_outbox` table until the server takes it, so plays made offline
//! reach the server, dated to when they started, once it answers again.

use parking_lot::Mutex;

use crate::db::connection::{Database, DbError};
use crate::db::queries::{self, HistoryCursor, OutboxEntry, OutboxKind};
use crate::remote::client::{SubsonicClient, SubsonicError};

/// Plays read per request.
const PAGE: u32 = 500;
/// Outbox entries sent per request.
const BATCH: u32 = 50;

/// One flush or pull at a time: two reading the same page would each adopt
/// its plays, and two flushing would each send the same scrobble.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// What a history sync did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct HistorySync {
    /// Outbox entries the server took.
    pub sent: usize,
    /// Plays from other devices added here.
    pub adopted: usize,
    /// Plays forgotten here because they were forgotten elsewhere.
    pub forgotten: usize,
    /// The history named tracks this library has not synced yet, so the
    /// library was synced too.
    pub library_synced: bool,
}

impl HistorySync {
    /// Whether anything a history page shows moved.
    pub fn changed(&self) -> bool {
        self.adopted > 0 || self.forgotten > 0 || self.library_synced
    }
}

/// Read what the server's history gained and lost, after sending what waits
/// here: what a device does when told the history moved. When the history
/// names a track this library lacks, the library is synced, which reads the
/// history again once the track is here.
pub fn sync(db: &Database) -> HistorySync {
    let cfg = crate::config::Config::load().unwrap_or_default();
    let Some(client) = crate::helpers::subsonic_client(&cfg) else {
        return HistorySync::default();
    };
    let out = {
        let _one = ONE_AT_A_TIME.lock();
        let mut out = HistorySync {
            sent: flush(db, &client, None),
            ..Default::default()
        };
        if let Err(e) = pull(
            db,
            &client,
            &cfg.remote.url,
            &cfg.remote.username,
            true,
            &mut out,
        ) {
            log::warn!("history: could not read the server's: {e}");
        }
        out
    };
    if out.library_synced {
        crate::remote::link::sync(db, crate::helpers::Walk::IfChanged);
    }
    out
}

/// Send what waits, then read the server's history: part of every sync, run
/// after the library, so the tracks a page names are here.
pub fn reconcile(db: &Database, client: &SubsonicClient, url: &str, username: &str) -> HistorySync {
    let _one = ONE_AT_A_TIME.lock();
    let mut out = HistorySync {
        sent: flush(db, client, None),
        ..Default::default()
    };
    if let Err(e) = pull(db, client, url, username, false, &mut out) {
        log::warn!("history: could not read the server's: {e}");
    }
    out
}

/// A heard play of `remote_id`, started at `at_ms`, for the server. Queued,
/// then sent with whatever else waits unless a sync is already sending.
pub fn scrobble(db: &Database, remote_id: &str, at_ms: i64) {
    if let Err(e) = queries::queue_scrobble(&db.conn, remote_id, at_ms) {
        log::warn!("history: could not queue a scrobble: {e}");
        return;
    }
    let cfg = crate::config::Config::load().unwrap_or_default();
    let Some(client) = crate::helpers::subsonic_client(&cfg) else {
        return;
    };
    // The player's history writer calls this; a sync holding the lock will
    // send it, or the next one will.
    if let Some(_one) = ONE_AT_A_TIME.try_lock() {
        flush(db, &client, Some(1));
    }
}

/// Forget these plays, by their ids here, and on the server for every device
/// on the account. Returns how many went.
pub fn forget(db: &Database, ids: &[i64]) -> Result<usize, DbError> {
    if signed_in() {
        for (remote_id, played_at) in
            queries::remote_ids_of_plays(&db.conn, queries::LOCAL_USER, ids)?
        {
            queries::queue_forget(&db.conn, &remote_id, played_at * 1000)?;
        }
    }
    let removed = queries::delete_plays(&db.conn, queries::LOCAL_USER, ids)?;
    flush_in_background();
    Ok(removed)
}

/// Forget every play, here and on the server for every device on the
/// account. Returns how many went here.
pub fn clear(db: &Database) -> Result<usize, DbError> {
    if signed_in() {
        queries::queue_clear(&db.conn, now_ms())?;
    }
    let removed = queries::clear_play_history(&db.conn, queries::LOCAL_USER)?;
    flush_in_background();
    Ok(removed)
}

fn signed_in() -> bool {
    let cfg = crate::config::Config::load().unwrap_or_default();
    crate::helpers::subsonic_auth(&cfg).is_some()
}

fn flush_in_background() {
    let spawned = std::thread::Builder::new()
        .name("koan-history-flush".into())
        .spawn(|| {
            let Ok(db) = crate::db::pool::shared().get() else {
                return;
            };
            let cfg = crate::config::Config::load().unwrap_or_default();
            if let Some(client) = crate::helpers::subsonic_client(&cfg) {
                let _one = ONE_AT_A_TIME.lock();
                flush(&db, &client, None);
            }
        });
    if let Err(e) = spawned {
        log::warn!("history: could not start sending: {e}");
    }
}

/// Whether the server keeps the account's history for its devices. `None`
/// while it cannot be asked.
fn shares_history(client: &SubsonicClient) -> Option<bool> {
    crate::remote::profile::for_auth(client.auth())
        .map(|p| p.offers(crate::remote::profile::HISTORY))
}

/// Send what waits, oldest first, up to `batches` requests. Stops at the
/// first request the server does not answer, keeping the rest for next time.
/// An entry the server answers with an error is dropped: it would refuse it
/// again. Returns how many entries it took.
fn flush(db: &Database, client: &SubsonicClient, batches: Option<usize>) -> usize {
    let mut sent = 0;
    let mut requests = 0;
    loop {
        if batches.is_some_and(|max| requests >= max) {
            break;
        }
        let waiting = match queries::history_outbox(&db.conn, BATCH) {
            Ok(w) => w,
            Err(e) => {
                log::warn!("history: could not read the outbox: {e}");
                break;
            }
        };
        let Some(kind) = waiting.first().map(|e| e.kind) else {
            break;
        };
        let run: Vec<&OutboxEntry> = waiting.iter().take_while(|e| e.kind == kind).collect();
        requests += 1;
        let Some(taken) = send(client, kind, &run) else {
            break;
        };
        let ids: Vec<i64> = run.iter().map(|e| e.id).collect();
        if let Err(e) = queries::drop_from_outbox(&db.conn, &ids) {
            log::warn!("history: could not empty the outbox: {e}");
            break;
        }
        sent += taken;
    }
    sent
}

/// Send one run of entries of one kind. `None` when the server could not be
/// reached, so the run stays; otherwise how many of it the server took.
fn send(client: &SubsonicClient, kind: OutboxKind, run: &[&OutboxEntry]) -> Option<usize> {
    let pairs: Vec<(&str, i64)> = run
        .iter()
        .filter_map(|e| Some((e.remote_id.as_deref()?, e.at_ms)))
        .collect();
    match kind {
        OutboxKind::Scrobble => match client.scrobble_many(&pairs) {
            Ok(()) => Some(pairs.len()),
            // One track the server no longer has fails the whole batch on
            // koan: each is sent alone to keep the rest.
            Err(SubsonicError::Api { .. }) if pairs.len() > 1 => {
                let mut taken = 0;
                for pair in &pairs {
                    match client.scrobble_many(std::slice::from_ref(pair)) {
                        Ok(()) => taken += 1,
                        Err(SubsonicError::Api { code, message }) => {
                            log::info!("history: server refused a scrobble ({code}: {message})");
                        }
                        Err(_) => return None,
                    }
                }
                Some(taken)
            }
            Err(e) => answered(e).map(|()| 0),
        },
        OutboxKind::Forget | OutboxKind::Clear => {
            // A server that keeps no shared history has nothing to forget.
            if !shares_history(client)? {
                return Some(0);
            }
            let result = match kind {
                OutboxKind::Forget => client.koan_forget_plays(&pairs),
                _ => run
                    .iter()
                    .try_for_each(|e| client.koan_forget_plays_through(e.at_ms)),
            };
            match result {
                Ok(()) => Some(run.len()),
                Err(e) => answered(e).map(|()| 0),
            }
        }
    }
}

/// `Some` when the server answered the request, even with a refusal.
fn answered(e: SubsonicError) -> Option<()> {
    match e {
        SubsonicError::Api { code, message } => {
            log::info!("history: server refused ({code}: {message})");
            Some(())
        }
        e => {
            log::info!("history: server not reached: {e}");
            None
        }
    }
}

/// Read the server's history after this device's cursor. With `strict`, a
/// page naming a track this library lacks is left unread and
/// `library_synced` set, for the caller to sync the library and read again;
/// without, such plays are passed over.
fn pull(
    db: &Database,
    client: &SubsonicClient,
    url: &str,
    username: &str,
    strict: bool,
    out: &mut HistorySync,
) -> Result<(), SubsonicError> {
    if shares_history(client) != Some(true) {
        return Ok(());
    }
    let mut cursor = queries::history_cursor(&db.conn, url).map_err(db_failed)?;
    loop {
        let page = client.koan_history(cursor, PAGE)?;
        let next = HistoryCursor::parse(&page.cursor).ok_or(SubsonicError::BadResponse)?;

        let played: Vec<String> = page.play.iter().map(|p| p.id.clone()).collect();
        let tracks = queries::track_ids_for_remote_ids(&db.conn, &played).map_err(db_failed)?;
        if strict && tracks.iter().any(Option::is_none) {
            out.library_synced = true;
            return Ok(());
        }

        // Forgettings first: a play this device made and another forgot is
        // here as this device's own, and must go.
        for f in &page.forgotten {
            let secs = f.played / 1000;
            let removed = match &f.id {
                None => queries::forget_plays_through(&db.conn, queries::LOCAL_USER, secs),
                Some(id) => {
                    let track =
                        queries::track_ids_for_remote_ids(&db.conn, std::slice::from_ref(id))
                            .map_err(db_failed)?;
                    match track.first().copied().flatten() {
                        Some(track) => {
                            queries::forget_play_near(&db.conn, queries::LOCAL_USER, track, secs)
                        }
                        None => Ok(0),
                    }
                }
            };
            out.forgotten += removed.map_err(db_failed)?;
        }

        let plays: Vec<(i64, i64, Option<i64>)> = page
            .play
            .iter()
            .zip(&tracks)
            .filter_map(|(p, track)| Some(((*track)?, p.played / 1000, p.listened_ms)))
            .collect();
        out.adopted +=
            queries::adopt_plays(&db.conn, queries::LOCAL_USER, &plays).map_err(db_failed)?;
        queries::set_history_cursor(&db.conn, url, username, next).map_err(db_failed)?;
        cursor = next;
        if !page.more {
            return Ok(());
        }
    }
}

fn db_failed(e: DbError) -> SubsonicError {
    SubsonicError::Io(std::io::Error::other(e.to_string()))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}
