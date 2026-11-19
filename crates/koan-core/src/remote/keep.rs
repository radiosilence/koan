//! What the person asked to have on this device, kept in the database so the
//! ask outlives the process: a download asked for and not yet arrived is
//! still owed after the app is suspended, killed or relaunched.
//!
//! The ask is the track's `cache_pinned` mark, set before its file arrives.
//! Writes go through one thread in the order they were made, so a cancel
//! right after an ask is never overtaken by it, and the player's thread,
//! which takes both, never waits on the database.

use std::sync::OnceLock;

use crate::db::queries;

enum Change {
    Want(Vec<i64>),
    Unwant(Vec<i64>),
    /// Downloads were cleared: what is still owed goes to the queue, in
    /// order behind every ask made before the clear.
    Cleared(std::collections::HashSet<i64>),
}

fn lane() -> Option<&'static crossbeam_channel::Sender<Change>> {
    static LANE: OnceLock<Option<crossbeam_channel::Sender<Change>>> = OnceLock::new();
    LANE.get_or_init(|| {
        let (tx, rx) = crossbeam_channel::unbounded::<Change>();
        std::thread::Builder::new()
            .name("koan-keep".into())
            .spawn(move || {
                for change in rx {
                    write(&change);
                }
            })
            .map_err(|e| log::error!("could not start the downloads' record: {e}"))
            .ok()
            .map(|_| tx)
    })
    .as_ref()
}

fn write(change: &Change) {
    if let Change::Cleared(owed) = change {
        crate::remote::queue::still_owed(owed.clone());
        return;
    }
    let result = crate::db::pool::shared()
        .get()
        .map_err(|e| e.to_string())
        .and_then(|db| {
            match change {
                Change::Want(ids) => queries::want_kept(&db.conn, ids),
                Change::Unwant(ids) => queries::unwant(&db.conn, ids),
                Change::Cleared(_) => Ok(()),
            }
            .map_err(|e| e.to_string())
        });
    if let Err(e) = result {
        log::warn!("could not record which downloads are wanted: {e}");
    }
}

fn send(change: Change) {
    match lane() {
        Some(lane) => {
            if let Err(e) = lane.send(change) {
                write(&e.0);
            }
        }
        None => write(&change),
    }
}

/// The person asked for these tracks on this device.
pub fn want(track_ids: &[i64]) {
    if !track_ids.is_empty() {
        send(Change::Want(track_ids.to_vec()));
    }
}

/// The person no longer wants these downloads.
pub fn unwant(track_ids: &[i64]) {
    if !track_ids.is_empty() {
        send(Change::Unwant(track_ids.to_vec()));
    }
}

/// Downloads were cleared, all or some, and with them the asks for those
/// tracks: the queue drops what is no longer owed. Read from `conn`, the
/// connection that cleared them, so no other is opened for it.
pub fn cleared(conn: &rusqlite::Connection) {
    match queries::owed_downloads(conn) {
        Ok(owed) => send(Change::Cleared(owed.into_iter().collect())),
        Err(e) => log::warn!("could not read which downloads are still owed: {e}"),
    }
}

/// Downloads asked for that have not arrived, to fetch again. Read on the
/// caller's thread; not the player's.
pub fn owed() -> Vec<i64> {
    crate::db::pool::shared()
        .get()
        .map_err(|e| e.to_string())
        .and_then(|db| queries::owed_downloads(&db.conn).map_err(|e| e.to_string()))
        .unwrap_or_else(|e| {
            log::warn!("could not read which downloads are owed: {e}");
            Vec::new()
        })
}
