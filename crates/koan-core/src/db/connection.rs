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
        // Long enough to outlast a scan chunk: a writer that gives up mid-scan
        // silently loses favourites, queue state and play counts.
        conn.pragma_update(None, "busy_timeout", 30000)?;
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

/// A collation for names the way a person reads them.
///
/// SQLite's default is a byte comparison, which sorts every capital before
/// every lowercase and every accented letter after the whole ASCII range — so
/// an artist list ran `Zebra`, then `aphex twin`, and put `Âme` at the end
/// where nobody would look for it.
///
/// Case is folded, accents are folded onto their base letter (`Âme` sorts with
/// `Ame`), and runs of digits compare by value so `Track 2` precedes
/// `Track 10`. Ties fall back to the raw bytes, so two names that differ only
/// in case or accent still have a stable order rather than being treated as
/// equal.
/// Registered by `create_tables`, so every connection has it — a query using
/// `COLLATE LIBRARY` on a connection that skipped this fails outright rather
/// than quietly sorting some other way.
pub(crate) fn register_library_collation(conn: &Connection) -> rusqlite::Result<()> {
    conn.create_collation("LIBRARY", |a, b| {
        sort_key(a).cmp(&sort_key(b)).then(a.cmp(b))
    })
}

/// One comparable chunk of a name: either a run of digits, as a number, or a
/// run of folded characters.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum Chunk {
    Number(u128),
    Text(String),
}

fn sort_key(s: &str) -> Vec<Chunk> {
    use unicode_normalization::UnicodeNormalization;

    // NFD splits an accented letter into its base plus a combining mark; dropping
    // the marks leaves the base letter to sort on.
    let folded: String = s
        .nfd()
        .filter(|c| !matches!(*c as u32, 0x0300..=0x036F))
        .flat_map(char::to_lowercase)
        .collect();

    let mut chunks = Vec::new();
    let mut rest = folded.as_str();
    while !rest.is_empty() {
        let digits = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        if digits > 0 && rest.starts_with(|c: char| c.is_ascii_digit()) {
            // Absurdly long digit runs are not numbers anyone sorts by.
            match rest[..digits].parse::<u128>() {
                Ok(n) => chunks.push(Chunk::Number(n)),
                Err(_) => chunks.push(Chunk::Text(rest[..digits].to_string())),
            }
            rest = &rest[digits..];
            continue;
        }
        let text = rest
            .find(|c: char| c.is_ascii_digit())
            .unwrap_or(rest.len())
            .max(1);
        chunks.push(Chunk::Text(rest[..text].to_string()));
        rest = &rest[text..];
    }
    chunks
}

#[cfg(test)]
mod collation_tests {
    use super::*;

    fn sorted(names: &[&str]) -> Vec<String> {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        conn.execute_batch("CREATE TABLE t (name TEXT)").unwrap();
        for n in names {
            conn.execute("INSERT INTO t VALUES (?1)", [n]).unwrap();
        }
        let mut stmt = conn
            .prepare("SELECT name FROM t ORDER BY name COLLATE LIBRARY")
            .unwrap();
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).unwrap();
        rows.map(Result::unwrap).collect()
    }

    #[test]
    fn lowercase_does_not_sort_after_everything() {
        assert_eq!(
            sorted(&["Zebra", "aphex twin", "Boards of Canada"]),
            ["aphex twin", "Boards of Canada", "Zebra"]
        );
    }

    #[test]
    fn accents_sort_with_their_base_letter() {
        // Byte order puts every non-ASCII name after `z`, which is where nobody
        // looks for Âme.
        assert_eq!(
            sorted(&["Zomby", "Âme", "Alva Noto"]),
            ["Alva Noto", "Âme", "Zomby"]
        );
    }

    #[test]
    fn digit_runs_compare_as_numbers() {
        assert_eq!(
            sorted(&["Track 10", "Track 2", "Track 1"]),
            ["Track 1", "Track 2", "Track 10"]
        );
    }

    #[test]
    fn names_differing_only_in_case_keep_a_stable_order() {
        // Folding must not make them equal, or the order flips between runs.
        assert_eq!(
            sorted(&["kraftwerk", "Kraftwerk"]),
            ["Kraftwerk", "kraftwerk"]
        );
    }
}
