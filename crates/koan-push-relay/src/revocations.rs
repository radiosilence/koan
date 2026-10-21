//! The relay's only state: for each device that has revoked its handles, the
//! time it did. A handle minted at or before that time no longer opens.
//!
//! Held whole in memory, so a push never touches the disk, and written
//! through to SQLite before a revocation is acknowledged. An entry is dropped
//! once every handle it could refuse has expired anyway.

use std::collections::HashMap;
use std::path::Path;

use parking_lot::{Mutex, RwLock};
use rusqlite::Connection;

use crate::handle::TokenHash;

#[derive(Debug, PartialEq, Eq)]
pub enum RevokeError {
    Full,
    Store(String),
}

pub struct Revocations {
    map: RwLock<HashMap<TokenHash, u64>>,
    db: Mutex<Connection>,
    max: usize,
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
             ) WITHOUT ROWID;",
        )
        .map_err(|e| e.to_string())?;
        let this = Self {
            map: RwLock::new(HashMap::new()),
            db: Mutex::new(db),
            max,
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
        self.revoked_at(device).is_some_and(|at| minted_ms <= at)
    }

    /// When this device last revoked its handles.
    pub fn revoked_at(&self, device: &TokenHash) -> Option<u64> {
        self.map.read().get(device).copied()
    }

    /// Revoke every handle for this device minted up to `now_ms`. Blocks on
    /// the disk.
    pub fn revoke(&self, device: TokenHash, now_ms: u64) -> Result<(), RevokeError> {
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
    pub fn prune(&self, now_ms: u64, lifetime_ms: u64) -> Result<(), String> {
        let cutoff = now_ms.saturating_sub(lifetime_ms);
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

    pub fn is_empty(&self) -> bool {
        self.map.read().is_empty()
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
        assert!(!r.revoked(&device, 11));
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
        let r = Revocations::open(&path, 10, 200, 60 * DAY).unwrap();
        assert!(r.revoked(&[1; 16], 100));
        assert!(!r.revoked(&[1; 16], 101));
    }

    #[test]
    fn old_revocations_are_pruned() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.db");
        let r = Revocations::open(&path, 10, 0, 60 * DAY).unwrap();
        r.revoke([1; 16], DAY).unwrap();
        r.revoke([2; 16], 30 * DAY).unwrap();
        r.prune(70 * DAY, 60 * DAY).unwrap();
        assert_eq!(r.len(), 1);
        drop(r);
        let r = Revocations::open(&path, 10, 70 * DAY, 60 * DAY).unwrap();
        assert_eq!(r.len(), 1);
        assert!(r.revoked(&[2; 16], 30 * DAY));
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
