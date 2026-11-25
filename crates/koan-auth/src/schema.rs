//! The tables, versioned apart from the rest of the database.
//!
//! A service versions its own tables, usually with `PRAGMA user_version`, so
//! these keep their version in a table of their own, `koan_auth_schema`.
//! [`migrate`] brings them up to date, and refuses a database whose tables
//! are newer than this build, rather than writing to tables it does not
//! understand.
//!
//! The first version is kōan's tables as they stood when they moved here,
//! columns and all, so a kōan database adopts it without a change: `device`
//! and `device_key` on `api_keys` are kōan's, binding a key to the device it
//! signed in. The second adds the second factor's tables.

use rusqlite::{Connection, OptionalExtension};

/// Each version's statements, in order: the first creates the tables.
const MIGRATIONS: &[&str] = &[
    "
    CREATE TABLE IF NOT EXISTS users (
        id            INTEGER PRIMARY KEY AUTOINCREMENT,
        username      TEXT NOT NULL UNIQUE,
        password_hash TEXT NOT NULL,
        role          TEXT NOT NULL DEFAULT 'user' CHECK (role IN ('admin', 'user', 'readonly')),
        created_at    TEXT DEFAULT (datetime('now'))
    );

    -- Sessions, under the SHA-256 of the secret the client holds. A grant's
    -- tokens are kept once spent, until they would have expired, so that one
    -- coming back is recognised.
    CREATE TABLE IF NOT EXISTS refresh_tokens (
        id          TEXT PRIMARY KEY,
        user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        expires_at  INTEGER NOT NULL,
        revoked     INTEGER NOT NULL DEFAULT 0,
        created_at  TEXT DEFAULT (datetime('now')),
        grant_id    TEXT,
        client_name TEXT,
        used_at     INTEGER
    );

    CREATE INDEX IF NOT EXISTS idx_refresh_tokens_user ON refresh_tokens(user_id);

    -- Only `sha256(key)` is kept: a key is 32 random bytes, so a fast hash is
    -- enough, and a read of the database yields nothing that signs in.
    CREATE TABLE IF NOT EXISTS api_keys (
        id           INTEGER PRIMARY KEY,
        user_id      INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        name         TEXT NOT NULL,
        key_hash     TEXT NOT NULL UNIQUE,
        created_at   INTEGER NOT NULL,
        last_used_at INTEGER,
        device       TEXT,
        device_key   TEXT
    );

    CREATE INDEX IF NOT EXISTS idx_api_keys_user ON api_keys(user_id);

    -- Sealed under a key derived from the server's signing key: see
    -- `app_passwords`.
    CREATE TABLE IF NOT EXISTS app_passwords (
        id           INTEGER PRIMARY KEY,
        user_id      INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        name         TEXT NOT NULL,
        sealed       BLOB NOT NULL,
        created_at   INTEGER NOT NULL,
        last_used_at INTEGER
    );

    CREATE INDEX IF NOT EXISTS idx_app_passwords_user ON app_passwords(user_id);
",
    "
    -- A second factor: see `second_factor`. An authenticator app's secret is
    -- sealed as an app password is, and unconfirmed until a code from it is
    -- given; `last_step` is the last step a code was accepted for.
    CREATE TABLE IF NOT EXISTS totp (
        user_id    INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
        sealed     BLOB NOT NULL,
        confirmed  INTEGER NOT NULL DEFAULT 0,
        last_step  INTEGER NOT NULL DEFAULT 0,
        created_at INTEGER NOT NULL
    );

    -- WebAuthn credentials: the COSE public key and the signature counter.
    CREATE TABLE IF NOT EXISTS security_keys (
        id            INTEGER PRIMARY KEY,
        user_id       INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        name          TEXT NOT NULL,
        credential_id BLOB NOT NULL UNIQUE,
        public_key    BLOB NOT NULL,
        sign_count    INTEGER NOT NULL DEFAULT 0,
        created_at    INTEGER NOT NULL,
        last_used_at  INTEGER
    );

    CREATE INDEX IF NOT EXISTS idx_security_keys_user ON security_keys(user_id);

    -- Only `sha256(code)`: a code is random, so a fast hash is enough.
    CREATE TABLE IF NOT EXISTS recovery_codes (
        id        INTEGER PRIMARY KEY,
        user_id   INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        code_hash TEXT NOT NULL,
        used_at   INTEGER
    );

    CREATE INDEX IF NOT EXISTS idx_recovery_codes_user ON recovery_codes(user_id);
",
];

/// The version this build's tables are at.
pub const VERSION: i64 = MIGRATIONS.len() as i64;

/// The version of the tables in `conn`'s database: 0 for none.
pub fn version(conn: &Connection) -> rusqlite::Result<i64> {
    let has_table: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'koan_auth_schema')",
        [],
        |r| r.get(0),
    )?;
    if !has_table {
        return Ok(0);
    }
    Ok(conn
        .query_row("SELECT version FROM koan_auth_schema", [], |r| r.get(0))
        .optional()?
        .unwrap_or(0))
}

/// Create the tables, or bring them up to [`VERSION`]. A database already
/// there is only read, so this is cheap to call on every open. Inside the
/// caller's transaction it is a savepoint in it; outside one, its own.
pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let found = version(conn)?;
    if found > VERSION {
        return Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_ERROR),
            Some(format!(
                "the database's account tables are at version {found}, newer than this \
                 build understands ({VERSION}): upgrade rather than downgrade"
            )),
        ));
    }
    if found == VERSION {
        return Ok(());
    }
    crate::sql::atomically(conn, || {
        // Again under the write lock: another process opening the database
        // at the same moment may have migrated it meanwhile.
        let found = version(conn)?;
        if found >= VERSION {
            return Ok(());
        }
        for sql in &MIGRATIONS[found as usize..] {
            conn.execute_batch(sql)?;
        }
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS koan_auth_schema (version INTEGER NOT NULL);
             DELETE FROM koan_auth_schema;",
        )?;
        conn.execute(
            "INSERT INTO koan_auth_schema (version) VALUES (?1)",
            [VERSION],
        )?;
        Ok(())
    })
}

/// A fresh in-memory database with the tables, enforcing foreign keys.
#[cfg(test)]
pub(crate) fn test_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.pragma_update(None, "foreign_keys", "ON").unwrap();
    migrate(&conn).unwrap();
    conn
}

/// A fresh database at `path`, for tests that need a second connection.
#[cfg(test)]
pub(crate) fn test_file_db(path: &std::path::Path) -> Connection {
    let conn = Connection::open(path).unwrap();
    conn.pragma_update(None, "journal_mode", "WAL").unwrap();
    conn.pragma_update(None, "foreign_keys", "ON").unwrap();
    migrate(&conn).unwrap();
    conn
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tables_are_made_once_and_then_only_read() {
        let conn = Connection::open_in_memory().unwrap();
        assert_eq!(version(&conn).unwrap(), 0);
        migrate(&conn).unwrap();
        assert_eq!(version(&conn).unwrap(), VERSION);
        let changes = conn.total_changes();
        migrate(&conn).unwrap();
        assert_eq!(conn.total_changes(), changes);
    }

    #[test]
    fn tables_from_a_newer_build_are_refused() {
        let conn = test_db();
        conn.execute("UPDATE koan_auth_schema SET version = ?1", [VERSION + 1])
            .unwrap();
        assert!(migrate(&conn).is_err());
    }

    #[test]
    fn tables_a_service_already_made_are_adopted() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(MIGRATIONS[0]).unwrap();
        conn.execute(
            "INSERT INTO users (username, password_hash) VALUES ('kept', 'x')",
            [],
        )
        .unwrap();
        migrate(&conn).unwrap();
        assert!(
            crate::users::get_user_by_username(&conn, "kept")
                .unwrap()
                .is_some()
        );
    }
}
