use std::path::Path;

use rusqlite::Connection;
use thiserror::Error;

use super::schema;
use crate::config;

#[derive(Debug, Error)]
pub enum DbError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Wrapper around a SQLite connection with koan's schema applied.
pub struct Database {
    pub conn: Connection,
}

impl Database {
    /// Open (or create) a database at the given path.
    pub fn open(path: &Path) -> Result<Self, DbError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(path)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }

        // WAL mode for concurrent reads + single writer.
        conn.pragma_update(None, "journal_mode", "wal")?;
        conn.pragma_update(None, "foreign_keys", "on")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        // Slightly faster at the cost of durability on power loss (acceptable for a media DB).
        conn.pragma_update(None, "synchronous", "normal")?;

        // Attempt a passive WAL checkpoint on open. This is non-blocking — it
        // moves WAL pages back to the main DB file only if no readers/writers
        // are active, preventing unbounded WAL growth across sessions.
        let _ = conn.execute_batch("PRAGMA wal_checkpoint(PASSIVE)");

        schema::create_tables(&conn)?;

        Ok(Self { conn })
    }

    /// Open the default database at the standard data directory.
    pub fn open_default() -> Result<Self, DbError> {
        Self::open(&config::db_path())
    }
}
