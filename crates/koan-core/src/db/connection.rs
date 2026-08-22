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
    /// A bulk delete looked like a mount failure rather than an intentional
    /// deletion, so it was refused. The library is untouched.
    #[error("refused unsafe bulk delete: {0}")]
    UnsafeBulkDelete(String),
}

/// Wrapper around a SQLite connection with koan's schema applied.
pub struct Database {
    pub conn: Connection,
}

impl Database {
    /// Open (or create) a database at the given path, applying the schema and
    /// pending migrations.
    ///
    /// This is the once-per-process path: it creates the parent directory,
    /// tightens file permissions, checkpoints the WAL and runs the ~30-statement
    /// DDL batch. Anything opening a connection per request wants
    /// [`Database::open_existing`] instead.
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

        configure(&conn)?;

        // Attempt a passive WAL checkpoint on open. This is non-blocking — it
        // moves WAL pages back to the main DB file only if no readers/writers
        // are active, preventing unbounded WAL growth across sessions.
        let _ = conn.execute_batch("PRAGMA wal_checkpoint(PASSIVE)");

        schema::create_tables(&conn)?;

        Ok(Self { conn })
    }

    /// Open an additional connection to a database whose schema is already
    /// applied — pragmas only, no DDL, no checkpoint, no permission syscall.
    ///
    /// Callers are responsible for having run [`Database::open`] at least once
    /// against the same path first.
    pub fn open_existing(path: &Path) -> Result<Self, DbError> {
        let conn = Connection::open(path)?;
        configure(&conn)?;
        Ok(Self { conn })
    }

    /// Open the default database at the standard data directory.
    pub fn open_default() -> Result<Self, DbError> {
        Self::open(&config::db_path())
    }
}

/// Connection-scoped pragmas. Every connection needs these; none of them touch
/// the file on disk, so they are cheap enough to repeat per connection.
fn configure(conn: &Connection) -> Result<(), DbError> {
    // WAL mode for concurrent reads + single writer.
    conn.pragma_update(None, "journal_mode", "wal")?;
    conn.pragma_update(None, "foreign_keys", "on")?;
    // Long enough to outlast a scan chunk: a writer that gives up mid-scan
    // silently loses favourites, queue state and play counts.
    conn.pragma_update(None, "busy_timeout", 30000)?;
    // Slightly faster at the cost of durability on power loss (acceptable for a media DB).
    conn.pragma_update(None, "synchronous", "normal")?;
    Ok(())
}
