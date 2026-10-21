//! The relay's only state: for each device that has revoked its handles, or
//! that Apple says is gone, the time it happened. A handle minted at or
//! before that time, or within `SLACK_MS` after it, no longer opens.
//!
//! Held whole in memory, so a push never touches the disk, and written
//! through to SQLite before a revocation is acknowledged. An entry is dropped
//! once every handle it could refuse has expired anyway.
//!
//! Beside it, the latest time any handle was minted at. A revocation is
//! recorded at that time if the clock now reads earlier, so a handle minted
//! while the clock ran fast cannot outrun a revocation made once it is
//! corrected.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::{Mutex, RwLock};
use rusqlite::Connection;

use crate::handle::TokenHash;

/// How far past a revocation's recorded time it reaches. A handle minted
/// before the revocation still falls inside it if the clock has since been
/// stepped back by up to this much; registration mints past it.
pub const SLACK_MS: u64 = 10 * 60 * 1000;

#[derive(Debug, PartialEq, Eq)]
pub enum RevokeError {
    Full,
    Store(String),
}

pub struct Revocations {
    map: RwLock<HashMap<TokenHash, u64>>,
    db: Mutex<Connection>,
    max: usize,
    /// The latest mint time, and the latest revocation time, in Unix ms.
    minted: AtomicU64,
    newest: AtomicU64,
}

impl Revocations {
    pub fn open(path: &Path, max: usize, now_ms: u64, lifetime_ms: u64) -> Result<Self, String> {
        let db = Connection::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        db.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = FULL;
             CREATE TABLE IF NOT EXISTS revocations (
                 token_hash BLOB PRIMARY KEY NOT NULL,
                 revoked_ms INTEGER NOT NULL
             ) WITHOUT ROWID;
             CREATE TABLE IF NOT EXISTS clock (
                 name TEXT PRIMARY KEY NOT NULL,
                 ms INTEGER NOT NULL
             ) WITHOUT ROWID;",
        )
        .map_err(|e| e.to_string())?;
        let minted: i64 = db
            .query_row("SELECT ms FROM clock WHERE name = 'minted'", [], |r| {
                r.get(0)
            })
            .unwrap_or(0);
        let newest: i64 = db
            .query_row(
                "SELECT coalesce(max(revoked_ms), 0) FROM revocations",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let this = Self {
            map: RwLock::new(HashMap::new()),
            db: Mutex::new(db),
            max,
            minted: AtomicU64::new(minted as u64),
            newest: AtomicU64::new(newest as u64),
        };
        this.prune(now_ms, lifetime_ms)?;
        let rows = {
            let db = this.db.lock();
            let mut stmt = db
                .prepare("SELECT token_hash, revoked_ms FROM revocations")
                .map_err(|e| e.to_string())?;
            stmt.query_map([], |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, i64>(1)?)))
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?
        };
        let mut map = this.map.write();
        for (hash, at) in rows {
            if let Ok(hash) = TokenHash::try_from(hash.as_slice()) {
                map.insert(hash, at as u64);
            }
        }
        drop(map);
        Ok(this)
    }

    /// Whether a handle for this device minted at `minted_ms` has been
    /// revoked.
    pub fn revoked(&self, device: &TokenHash, minted_ms: u64) -> bool {
        self.revoked_at(device)
            .is_some_and(|at| minted_ms <= at.saturating_add(SLACK_MS))
    }

    /// When this device last revoked its handles.
    pub fn revoked_at(&self, device: &TokenHash) -> Option<u64> {
        self.map.read().get(device).copied()
    }

    /// Note that a handle was minted at `minted_ms`.
    pub fn minted(&self, minted_ms: u64) {
        self.minted.fetch_max(minted_ms, Ordering::Relaxed);
    }

    /// Keep the latest mint time on disk, so a restart still knows it. Blocks
    /// on the disk.
    pub fn save_minted(&self) -> Result<(), String> {
        let minted = self.minted.load(Ordering::Relaxed);
        self.db
            .lock()
            .execute(
                "INSERT INTO clock (name, ms) VALUES ('minted', ?1)
                 ON CONFLICT (name) DO UPDATE SET ms = max(ms, ?1)",
                [minted as i64],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Revoke every handle for this device minted up to `now_ms`, or up to
    /// the latest mint time if the clock reads earlier, and always past the
    /// device's previous revocation, which a handle minted since reaches
    /// beyond. Blocks on the disk.
    pub fn revoke(&self, device: TokenHash, now_ms: u64) -> Result<(), RevokeError> {
        let after_last = self.revoked_at(&device).map_or(0, |at| at + 1);
        let now_ms = now_ms
            .max(self.minted.load(Ordering::Relaxed))
            .max(after_last);
        // One writer at a time keeps the size check and the insert together.
        let db = self.db.lock();
        {
            let map = self.map.read();
            if !map.contains_key(&device) && map.len() >= self.max {
                return Err(RevokeError::Full);
            }
        }
        db.execute(
            "INSERT INTO revocations (token_hash, revoked_ms) VALUES (?1, ?2)
             ON CONFLICT (token_hash) DO UPDATE SET revoked_ms = max(revoked_ms, ?2)",
            rusqlite::params![device.as_slice(), now_ms as i64],
        )
        .map_err(|e| RevokeError::Store(e.to_string()))?;
        let mut map = self.map.write();
        let at = map.entry(device).or_insert(now_ms);
        *at = (*at).max(now_ms);
        self.newest.fetch_max(now_ms, Ordering::Relaxed);
        Ok(())
    }

    /// Whether a revocation for this device would be recorded rather than
    /// refused for want of room.
    pub fn has_room(&self, device: &TokenHash) -> bool {
        let map = self.map.read();
        map.len() < self.max || map.contains_key(device)
    }

    /// Drop revocations older than any handle that could still open. Blocks
    /// on the disk.
    ///
    /// Measured from the newest revocation when the clock reads later than
    /// it: a clock jumped far ahead must not prune rows that live handles
    /// still need once it is corrected.
    pub fn prune(&self, now_ms: u64, lifetime_ms: u64) -> Result<(), String> {
        let reference = now_ms.min(self.newest.load(Ordering::Relaxed).max(1));
        let cutoff = reference.saturating_sub(lifetime_ms + SLACK_MS);
        let db = self.db.lock();
        db.execute(
            "DELETE FROM revocations WHERE revoked_ms < ?1",
            [cutoff as i64],
        )
        .map_err(|e| e.to_string())?;
        self.map.write().retain(|_, at| *at >= cutoff);
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.map.read().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u64 = 86_400_000;

    #[test]
    fn a_revocation_refuses_handles_minted_up_to_it() {
        let dir = tempfile::tempdir().unwrap();
        let r = Revocations::open(&dir.path().join("r.db"), 10, 0, 60 * DAY).unwrap();
        let device = [1; 16];
        assert!(!r.revoked(&device, 5));
        r.revoke(device, 10).unwrap();
        assert!(r.revoked(&device, 5));
        assert!(r.revoked(&device, 10));
        assert!(
            r.revoked(&device, 10 + SLACK_MS),
            "a clock stepped back is allowed for"
        );
        assert!(!r.revoked(&device, 11 + SLACK_MS));
        assert!(!r.revoked(&[2; 16], 5));
    }

    #[test]
    fn revocations_survive_a_restart_and_never_move_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.db");
        {
            let r = Revocations::open(&path, 10, 0, 60 * DAY).unwrap();
            r.revoke([1; 16], 100).unwrap();
            r.revoke([1; 16], 50).unwrap();
        }
        // The second, read as earlier, is recorded just past the first.
        let r = Revocations::open(&path, 10, 200, 60 * DAY).unwrap();
        assert!(r.revoked(&[1; 16], 101 + SLACK_MS));
        assert!(!r.revoked(&[1; 16], 102 + SLACK_MS));
    }

    #[test]
    fn old_revocations_are_pruned() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.db");
        let r = Revocations::open(&path, 10, 0, 60 * DAY).unwrap();
        r.revoke([1; 16], DAY).unwrap();
        r.revoke([2; 16], 30 * DAY).unwrap();
        r.revoke([3; 16], 80 * DAY).unwrap();
        r.prune(90 * DAY, 60 * DAY).unwrap();
        assert_eq!(r.len(), 2);
        drop(r);
        let r = Revocations::open(&path, 10, 90 * DAY, 60 * DAY).unwrap();
        assert_eq!(r.len(), 2);
        assert!(r.revoked(&[2; 16], 30 * DAY));
    }

    #[test]
    fn a_revocation_reaches_a_handle_minted_past_the_last_one() {
        let dir = tempfile::tempdir().unwrap();
        let r = Revocations::open(&dir.path().join("r.db"), 10, 0, 60 * DAY).unwrap();
        r.revoke([1; 16], 1000).unwrap();
        // Registration mints past the window; a quick second revocation must
        // still reach it.
        let minted = 1000 + SLACK_MS + 1;
        assert!(!r.revoked(&[1; 16], minted));
        r.revoke([1; 16], 1001).unwrap();
        assert!(r.revoked(&[1; 16], minted));
    }

    #[test]
    fn a_clock_jumped_ahead_prunes_nothing_still_needed() {
        let dir = tempfile::tempdir().unwrap();
        let r = Revocations::open(&dir.path().join("r.db"), 10, 0, 60 * DAY).unwrap();
        r.revoke([1; 16], 10 * DAY).unwrap();
        r.prune(1000 * DAY, 60 * DAY).unwrap();
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn a_revocation_reaches_handles_minted_while_the_clock_ran_fast() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.db");
        {
            let r = Revocations::open(&path, 10, 0, 60 * DAY).unwrap();
            r.minted(5 * DAY);
            r.save_minted().unwrap();
        }
        // The clock is corrected, and the relay restarted.
        let r = Revocations::open(&path, 10, DAY, 60 * DAY).unwrap();
        r.revoke([1; 16], DAY).unwrap();
        assert!(r.revoked(&[1; 16], 5 * DAY));
    }

    #[test]
    fn a_full_table_refuses_new_devices_but_not_known_ones() {
        let dir = tempfile::tempdir().unwrap();
        let r = Revocations::open(&dir.path().join("r.db"), 1, 0, 60 * DAY).unwrap();
        r.revoke([1; 16], 1).unwrap();
        assert_eq!(r.revoke([2; 16], 2), Err(RevokeError::Full));
        assert!(r.revoke([1; 16], 3).is_ok());
    }
}
