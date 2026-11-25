//! How this crate's writes share a connection with the service's own.

use std::time::Duration;

use rusqlite::Connection;

/// Run `f` as one unit: its writes land together or not at all, and the write
/// lock is taken once rather than per statement.
///
/// Inside a caller's transaction it is a savepoint, so a service can make an
/// account's change and its own in one transaction. Outside one it begins
/// `IMMEDIATE`: a deferred transaction that reads before it writes fails
/// outright, without waiting, when another connection wrote in between.
pub(crate) fn atomically<T, E: From<rusqlite::Error>>(
    conn: &Connection,
    f: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    let (begin, commit, rollback) = if conn.is_autocommit() {
        ("BEGIN IMMEDIATE", "COMMIT", "ROLLBACK")
    } else {
        (
            "SAVEPOINT koan_auth",
            "RELEASE koan_auth",
            "ROLLBACK TO koan_auth; RELEASE koan_auth",
        )
    };
    conn.execute_batch(begin)?;
    let result = f();
    let undo = || {
        if let Err(e) = conn.execute_batch(rollback) {
            log::warn!("rollback after a failed transaction: {e}");
        }
    };
    match &result {
        // A COMMIT that fails, busy or out of disk, leaves the transaction
        // open. Rolled back here, or the connection would go on holding the
        // write lock and every later write on it would be lost with it.
        Ok(_) => {
            if let Err(e) = conn.execute_batch(commit) {
                undo();
                return Err(e.into());
            }
        }
        // The work's own error is what the caller needs. A rollback that
        // fails, as one does after something inside ended the transaction
        // already, is logged rather than put in its place.
        Err(_) => undo(),
    }
    result
}

/// Run `write` without waiting for the write lock, which fails it with
/// `SQLITE_BUSY` while another connection holds the lock.
///
/// For bookkeeping a request should not wait on, such as a key's last use:
/// skipping one costs a stale time, waiting costs the request.
pub(crate) fn without_waiting<T, E: From<rusqlite::Error>>(
    conn: &Connection,
    write: impl FnOnce(&Connection) -> Result<T, E>,
) -> Result<T, E> {
    let waits: i64 = conn.pragma_query_value(None, "busy_timeout", |r| r.get(0))?;
    conn.busy_timeout(Duration::ZERO)?;
    let result = write(conn);
    // The write's own outcome is what the caller needs.
    if let Err(e) = conn.busy_timeout(Duration::from_millis(waits.max(0) as u64)) {
        log::warn!("restoring the busy timeout: {e}");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_busy_timeout_is_put_back_as_it_was() {
        let conn = Connection::open_in_memory().unwrap();
        conn.busy_timeout(Duration::from_millis(1234)).unwrap();
        without_waiting::<_, rusqlite::Error>(&conn, |_| Ok(())).unwrap();
        let waits: i64 = conn
            .pragma_query_value(None, "busy_timeout", |r| r.get(0))
            .unwrap();
        assert_eq!(waits, 1234);
    }

    #[test]
    fn a_unit_inside_a_transaction_is_a_savepoint_in_it() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t (n INTEGER); BEGIN;")
            .unwrap();
        let failed: Result<(), rusqlite::Error> = atomically(&conn, || {
            conn.execute("INSERT INTO t VALUES (1)", [])?;
            Err(rusqlite::Error::InvalidQuery)
        });
        assert!(failed.is_err());
        // The caller's transaction is still open, and the unit left nothing.
        assert!(!conn.is_autocommit());
        atomically::<_, rusqlite::Error>(&conn, || {
            conn.execute("INSERT INTO t VALUES (2)", [])?;
            Ok(())
        })
        .unwrap();
        conn.execute_batch("COMMIT").unwrap();
        let rows: Vec<i64> = conn
            .prepare("SELECT n FROM t")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(rows, [2]);
    }
}
