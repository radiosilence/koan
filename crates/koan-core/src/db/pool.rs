//! Database connections, opened once and kept.
//!
//! Every read used to open its own: a connection, a permissions syscall, the
//! whole schema DDL and a WAL checkpoint, before a single row came back.
//! Clicking an album paid all of it, and while downloads were writing, the
//! checkpoint contended with them and it took seconds.
//!
//! A pool rather than one shared connection, because rusqlite's `Connection` is
//! `Send` but not `Sync`: sharing one means a mutex, and a mutex means every
//! read waits for every other. SQLite in WAL mode reads concurrently across
//! connections, so the way to keep that is to have several and hand them out.
//!
//! A connection is opened only when every existing one is busy, so the pool
//! grows to whatever concurrency actually happens. What it *keeps* is capped:
//! queueing a thousand tracks runs as many transfers at once as
//! `download_workers` allows and no more, but a burst wider than the cap should
//! not leave a thousand connections parked for the rest of the session, each
//! holding its own page cache. Past the cap a returned connection is closed
//! rather than kept.

use std::ops::Deref;
use std::path::{Path, PathBuf};

use super::connection::{Database, DbError};

pub struct Pool {
    path: PathBuf,
    idle: parking_lot::Mutex<Vec<Database>>,
    keep: usize,
}

/// How many idle connections to hold on to.
///
/// Comfortably more than the concurrency anything here actually reaches — a
/// handful of front-end reads alongside the configured download workers — so
/// the steady state never opens one, while a burst still cannot park hundreds.
const KEEP_IDLE: usize = 32;

/// The process's pool for the default library.
///
/// One database, opened once, shared by everything that reads it: the front
/// ends, the downloader, the background tasks. Callers with their own path —
/// tests, mostly — build their own.
pub fn shared() -> &'static Pool {
    static POOL: std::sync::OnceLock<Pool> = std::sync::OnceLock::new();
    POOL.get_or_init(|| Pool::new(crate::config::db_path()))
}

impl Pool {
    /// The schema must already be applied — see [`Pool::get`].
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            idle: parking_lot::Mutex::new(Vec::new()),
            keep: KEEP_IDLE,
        }
    }

    /// Borrow a connection, opening one only if none are free.
    ///
    /// `open_existing`, so this never re-runs the DDL or checkpoints: the
    /// schema is applied once at startup, before any pool exists.
    pub fn get(&self) -> Result<Handle<'_>, DbError> {
        let pooled = self.idle.lock().pop();
        let db = match pooled {
            Some(db) => db,
            None => Database::open_existing(&self.path)?,
        };
        Ok(Handle {
            db: Some(db),
            pool: self,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn put_back(&self, db: Database) {
        let mut idle = self.idle.lock();
        if idle.len() < self.keep {
            idle.push(db);
        }
        // Otherwise it closes here, which is the point of the cap.
    }
}

/// A borrowed connection, returned to the pool when it goes out of scope.
///
/// Derefs to `Database` so callers reach `.conn` exactly as they did when this
/// was an owned connection they had opened themselves.
pub struct Handle<'a> {
    db: Option<Database>,
    pool: &'a Pool,
}

impl Deref for Handle<'_> {
    type Target = Database;

    fn deref(&self) -> &Database {
        self.db.as_ref().expect("a handle holds its connection")
    }
}

impl Drop for Handle<'_> {
    fn drop(&mut self) {
        if let Some(db) = self.db.take() {
            self.pool.put_back(db);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool() -> (tempfile::TempDir, Pool) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("koan.db");
        // What startup does, once.
        Database::open(&path).unwrap();
        (dir, Pool::new(path))
    }

    #[test]
    fn a_connection_comes_back_and_is_reused() {
        let (_dir, pool) = pool();
        {
            let db = pool.get().unwrap();
            db.conn.execute_batch("SELECT 1").unwrap();
        }
        assert_eq!(pool.idle.lock().len(), 1, "returned on drop");
        {
            let _db = pool.get().unwrap();
            assert_eq!(pool.idle.lock().len(), 0, "handed back out");
        }
        assert_eq!(pool.idle.lock().len(), 1);
    }

    #[test]
    fn concurrent_borrowers_get_their_own() {
        let (_dir, pool) = pool();
        let first = pool.get().unwrap();
        let second = pool.get().unwrap();
        first.conn.execute_batch("SELECT 1").unwrap();
        second.conn.execute_batch("SELECT 1").unwrap();
        drop(first);
        drop(second);
        assert_eq!(pool.idle.lock().len(), 2, "both kept for next time");
    }

    #[test]
    fn idle_connections_are_capped() {
        // A burst wider than the cap must not park connections for the rest of
        // the session.
        let (_dir, mut pool) = pool();
        pool.keep = 2;
        let handles: Vec<_> = (0..6).map(|_| pool.get().unwrap()).collect();
        assert_eq!(pool.idle.lock().len(), 0, "all of them are out");
        drop(handles);
        assert_eq!(pool.idle.lock().len(), 2, "the rest closed on return");
    }

    #[test]
    fn a_pooled_connection_sees_what_another_wrote() {
        // WAL readers are per-connection snapshots; a stale one would serve a
        // library that is missing whatever just landed.
        let (_dir, pool) = pool();
        {
            let db = pool.get().unwrap();
            db.conn
                .execute_batch("CREATE TABLE probe (v INTEGER); INSERT INTO probe VALUES (7)")
                .unwrap();
        }
        let db = pool.get().unwrap();
        let v: i64 = db
            .conn
            .query_row("SELECT v FROM probe", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 7);
    }
}
