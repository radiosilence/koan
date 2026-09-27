use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

use rusqlite::Connection;
use rusqlite::functions::FunctionFlags;
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

        // The planner picks between indexes by guessing how many rows each one
        // will yield, and with no statistics it guesses the same number for all
        // of them. That is how a partial index on the column a query filters by
        // loses to an index that merely happens to supply the ORDER BY. Cheap
        // after the first run, and a no-op when nothing has moved.
        let _ = conn.execute_batch("PRAGMA optimize");

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

    /// Refresh the planner's statistics.
    ///
    /// Worth calling wherever the library changes size in bulk — a scan, a
    /// remote sync — because the statistics gathered when the process started
    /// describe a library that no longer exists, and the planner will keep
    /// choosing for it. A no-op when nothing has moved far enough to matter.
    pub fn optimize(&self) {
        if let Err(e) = self.conn.execute_batch("PRAGMA optimize") {
            log::debug!("PRAGMA optimize failed: {e}");
        }
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
    // Map the whole library. A library this size fits well inside this, so
    // reads become dereferences into a mapped region rather than syscalls —
    // which is the useful sense in which a database can be "in memory". The
    // page cache was already holding it; this stops copying it out per read.
    conn.pragma_update(None, "mmap_size", 268_435_456i64)?;
    // 32 MiB of pages, per connection. Negative means KiB rather than pages,
    // so the figure does not change meaning with the page size.
    conn.pragma_update(None, "cache_size", -32_000i64)?;
    // Sorts and intermediate tables in memory. FTS and the ORDER BYs behind
    // every library listing make temporary tables constantly.
    conn.pragma_update(None, "temp_store", "memory")?;
    // How many rows `PRAGMA optimize` samples per index. Bounded, so gathering
    // statistics stays a fraction of a second on a library of any size; the
    // planner needs the shape of the distribution, not an exact count.
    conn.pragma_update(None, "analysis_limit", 400i64)?;
    // Here rather than with the schema: `open_existing` skips the DDL, and a
    // connection without this collation fails every ORDER BY that uses it.
    register_library_collation(conn)?;
    register_shuffle_function(conn)?;
    Ok(())
}

/// `koan_shuffle(id, seed)` — a stable pseudo-random ordering key.
///
/// A shuffled listing that is read a page at a time cannot shuffle in the
/// client: page two would be drawn from a different shuffle than page one, and
/// records would repeat or vanish as you scrolled. Ordering by a hash of the
/// row id and a seed gives one order that every page of the same seed agrees
/// on, and a new seed gives a different one.
///
/// Registered next to the collation, and for the same reason: a connection
/// without it fails the query outright rather than sorting some other way.
pub(crate) fn register_shuffle_function(conn: &Connection) -> rusqlite::Result<()> {
    conn.create_scalar_function(
        "koan_shuffle",
        2,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| {
            let id = ctx.get::<i64>(0)? as u64;
            let seed = ctx.get::<i64>(1)? as u64;
            Ok(splitmix64(id ^ splitmix64(seed)) as i64)
        },
    )
}

/// SplitMix64. Cheap, and it scatters consecutive ids — which matters, because
/// a library's ids are consecutive in the order it was scanned.
fn splitmix64(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
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
        cached_sort_key(a).cmp(&cached_sort_key(b)).then(a.cmp(b))
    })
}

thread_local! {
    /// Sort keys, kept for the life of the thread.
    ///
    /// A collation sees the same name once per level of the sort — around two
    /// dozen times in a five-thousand-row list — and building a key means an
    /// NFD pass and a `Vec` of freshly allocated `String`s. Cached, each name is
    /// folded once per thread instead of once per comparison.
    static SORT_KEYS: RefCell<HashMap<Box<str>, Rc<[Chunk]>>> = RefCell::new(HashMap::new());
}

fn cached_sort_key(s: &str) -> Rc<[Chunk]> {
    SORT_KEYS.with_borrow_mut(|cache| {
        if let Some(key) = cache.get(s) {
            return Rc::clone(key);
        }
        // A library's worth of names is tens of thousands of entries. Anything
        // beyond that is a query sorting something other than names, and it
        // should not grow this without bound.
        if cache.len() >= 50_000 {
            cache.clear();
        }
        let key: Rc<[Chunk]> = sort_key(s).into();
        cache.insert(s.into(), Rc::clone(&key));
        key
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
