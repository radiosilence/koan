//! EQ profiles kept everywhere: what a kōan server keeps of an account's
//! (`dsp_profiles`, `dsp_files`, `dsp_dismissed`), and what a device keeps of
//! its syncing (`dsp_sync_cursor`, `dsp_synced`, `dsp_local`). The syncing is
//! `crate::remote::dsp_sync`'s.

use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::connection::DbError;

/// A profile as the server keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredProfile {
    pub uid: String,
    pub rev: i64,
    pub edited_at: i64,
    /// The profile as JSON; `None` once deleted.
    pub doc: Option<String>,
}

/// What a save did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Saved {
    pub rev: i64,
    /// False when the server's copy was edited later and was kept.
    pub stored: bool,
}

fn next_rev(conn: &Connection, user: i64) -> Result<i64, DbError> {
    Ok(conn.query_row(
        "SELECT MAX(
             COALESCE((SELECT MAX(rev) FROM dsp_profiles WHERE user_id = ?1), 0),
             COALESCE((SELECT MAX(rev) FROM dsp_dismissed WHERE user_id = ?1), 0)
         ) + 1",
        [user],
        |r| r.get(0),
    )?)
}

/// The account's profiles changed after `since`, deletions included, oldest
/// first, and the revision to read from next.
pub fn changes(
    conn: &Connection,
    user: i64,
    since: i64,
) -> Result<(Vec<StoredProfile>, i64), DbError> {
    let mut stmt = conn.prepare(
        "SELECT uid, rev, edited_at, doc FROM dsp_profiles
          WHERE user_id = ?1 AND rev > ?2 ORDER BY rev",
    )?;
    let rows = stmt
        .query_map(params![user, since], |r| {
            Ok(StoredProfile {
                uid: r.get(0)?,
                rev: r.get(1)?,
                edited_at: r.get(2)?,
                doc: r.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let cursor = (next_rev(conn, user)? - 1).max(since);
    Ok((rows, cursor))
}

/// Keep `doc` (`None` to delete) as `uid`'s, unless the copy here was edited
/// after `edited_at`: the last edit wins, whichever device made it.
///
/// A deletion keeps the last document as `deleted_doc`, with when the
/// server recorded it by its own clock: devices are told it is gone, but the
/// server keeps a copy. A later save clears both.
pub fn save(
    conn: &Connection,
    user: i64,
    uid: &str,
    edited_at: i64,
    doc: Option<&str>,
) -> Result<Saved, DbError> {
    let existing: Option<(i64, i64)> = conn
        .query_row(
            "SELECT rev, edited_at FROM dsp_profiles WHERE user_id = ?1 AND uid = ?2",
            params![user, uid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((rev, at)) = existing
        && at > edited_at
    {
        return Ok(Saved { rev, stored: false });
    }
    let rev = next_rev(conn, user)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    conn.execute(
        "INSERT INTO dsp_profiles (user_id, uid, rev, edited_at, doc) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (user_id, uid) DO UPDATE
            SET rev = excluded.rev, edited_at = excluded.edited_at, doc = excluded.doc,
                deleted_doc = CASE WHEN excluded.doc IS NULL
                                   THEN COALESCE(dsp_profiles.doc, dsp_profiles.deleted_doc)
                              END,
                deleted_at = CASE WHEN excluded.doc IS NULL
                                  THEN COALESCE(dsp_profiles.deleted_at, ?6)
                             END",
        params![user, uid, rev, edited_at, doc, now],
    )?;
    Ok(Saved { rev, stored: true })
}

/// How long the server keeps a deleted profile's document and files.
pub const DELETED_KEPT_MS: i64 = 30 * 24 * 60 * 60 * 1000;

/// The account's profiles deleted since `since` (ms, the server's clock),
/// with the document each had: uid, when the server recorded the deletion,
/// and the document. Newest first.
pub fn deleted_docs(
    conn: &Connection,
    user: i64,
    since: i64,
) -> Result<Vec<(String, i64, String)>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT uid, deleted_at, deleted_doc FROM dsp_profiles
          WHERE user_id = ?1 AND doc IS NULL AND deleted_doc IS NOT NULL
            AND COALESCE(deleted_at, ?2) >= ?2
          ORDER BY deleted_at DESC",
    )?;
    Ok(stmt
        .query_map(params![user, since], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?
        .collect::<Result<Vec<_>, _>>()?)
}

/// The document profile `uid` had when it was deleted, if it was deleted
/// since `since` (ms, the server's clock), with the deletion's `edited_at`.
pub fn deleted_doc(
    conn: &Connection,
    user: i64,
    uid: &str,
    since: i64,
) -> Result<Option<(i64, String)>, DbError> {
    Ok(conn
        .query_row(
            "SELECT edited_at, deleted_doc FROM dsp_profiles
              WHERE user_id = ?1 AND uid = ?2 AND doc IS NULL AND deleted_doc IS NOT NULL
                AND COALESCE(deleted_at, ?3) >= ?3",
            params![user, uid, since],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
}

/// Forget the documents of profiles deleted before `before` (ms, the
/// server's clock).
pub fn expire_deleted(conn: &Connection, user: i64, before: i64) -> Result<usize, DbError> {
    Ok(conn.execute(
        "UPDATE dsp_profiles SET deleted_doc = NULL, deleted_at = NULL
          WHERE user_id = ?1 AND doc IS NULL AND deleted_doc IS NOT NULL AND deleted_at < ?2",
        params![user, before],
    )?)
}

/// The documents of the account's profiles that are not deleted.
pub fn live_docs(conn: &Connection, user: i64) -> Result<Vec<(String, String)>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT uid, doc FROM dsp_profiles WHERE user_id = ?1 AND doc IS NOT NULL ORDER BY uid",
    )?;
    Ok(stmt
        .query_map([user], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?)
}

/// The files the server holds for the account, by hash, with their sizes.
pub fn files(conn: &Connection, user: i64) -> Result<HashMap<String, i64>, DbError> {
    let mut stmt = conn.prepare("SELECT sha256, length(data) FROM dsp_files WHERE user_id = ?1")?;
    Ok(stmt
        .query_map([user], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<HashMap<_, _>, _>>()?)
}

pub fn file(conn: &Connection, user: i64, sha256: &str) -> Result<Option<Vec<u8>>, DbError> {
    Ok(conn
        .query_row(
            "SELECT data FROM dsp_files WHERE user_id = ?1 AND sha256 = ?2",
            params![user, sha256],
            |r| r.get(0),
        )
        .optional()?)
}

pub fn store_file(conn: &Connection, user: i64, sha256: &str, data: &[u8]) -> Result<(), DbError> {
    conn.execute(
        "INSERT OR IGNORE INTO dsp_files (user_id, sha256, data) VALUES (?1, ?2, ?3)",
        params![user, sha256, data],
    )?;
    Ok(())
}

/// Drop the account's files not in `keep`: those no profile names now.
pub fn keep_files(conn: &Connection, user: i64, keep: &HashSet<String>) -> Result<usize, DbError> {
    let held = files(conn, user)?;
    let mut dropped = 0;
    for sha in held.keys().filter(|s| !keep.contains(*s)) {
        dropped += conn.execute(
            "DELETE FROM dsp_files WHERE user_id = ?1 AND sha256 = ?2",
            params![user, sha],
        )?;
    }
    Ok(dropped)
}

/// Every output the account turned AutoEQ down for.
pub fn dismissed(conn: &Connection, user: i64) -> Result<Vec<String>, DbError> {
    let mut stmt =
        conn.prepare("SELECT output FROM dsp_dismissed WHERE user_id = ?1 ORDER BY output")?;
    Ok(stmt
        .query_map([user], |r| r.get(0))?
        .collect::<Result<Vec<_>, _>>()?)
}

/// How many profiles the server keeps for the account, deleted ones
/// included, and whether `uid` is one of them.
pub fn count(conn: &Connection, user: i64, uid: &str) -> Result<(i64, bool), DbError> {
    Ok(conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(uid = ?2), 0) > 0 FROM dsp_profiles WHERE user_id = ?1",
        params![user, uid],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?)
}

/// How many outputs the account turned AutoEQ down for.
pub fn dismissed_count(conn: &Connection, user: i64) -> Result<i64, DbError> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM dsp_dismissed WHERE user_id = ?1",
        [user],
        |r| r.get(0),
    )?)
}

/// Turn AutoEQ down for `output`. Whether it was new.
pub fn dismiss(conn: &Connection, user: i64, output: &str) -> Result<bool, DbError> {
    let rev = next_rev(conn, user)?;
    Ok(conn.execute(
        "INSERT OR IGNORE INTO dsp_dismissed (user_id, output, rev) VALUES (?1, ?2, ?3)",
        params![user, output, rev],
    )? > 0)
}

// -- A device's side --

pub fn sync_cursor(conn: &Connection, url: &str) -> Result<i64, DbError> {
    Ok(conn
        .query_row(
            "SELECT cursor FROM dsp_sync_cursor WHERE url = ?1",
            [url],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(0))
}

pub fn set_sync_cursor(conn: &Connection, url: &str, cursor: i64) -> Result<(), DbError> {
    conn.execute(
        "INSERT INTO dsp_sync_cursor (url, cursor) VALUES (?1, ?2)
         ON CONFLICT (url) DO UPDATE SET cursor = excluded.cursor",
        params![url, cursor],
    )?;
    Ok(())
}

/// Each profile as last synced with the server at `url`: its revision and
/// content hash, by uid.
pub fn synced(conn: &Connection, url: &str) -> Result<HashMap<String, (i64, String)>, DbError> {
    let mut stmt = conn.prepare("SELECT uid, rev, hash FROM dsp_synced WHERE url = ?1")?;
    Ok(stmt
        .query_map([url], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?
        .collect::<Result<HashMap<_, _>, _>>()?)
}

pub fn set_synced(
    conn: &Connection,
    url: &str,
    uid: &str,
    rev: i64,
    hash: &str,
) -> Result<(), DbError> {
    conn.execute(
        "INSERT INTO dsp_synced (url, uid, rev, hash) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (url, uid) DO UPDATE SET rev = excluded.rev, hash = excluded.hash",
        params![url, uid, rev, hash],
    )?;
    Ok(())
}

pub fn forget_synced(conn: &Connection, url: &str, uid: &str) -> Result<(), DbError> {
    conn.execute(
        "DELETE FROM dsp_synced WHERE url = ?1 AND uid = ?2",
        params![url, uid],
    )?;
    Ok(())
}

/// Everything this device synced with the server at `url`, for forgetting it.
pub fn forget_server(conn: &Connection, url: &str) -> Result<(), DbError> {
    conn.execute("DELETE FROM dsp_synced WHERE url = ?1", [url])?;
    conn.execute("DELETE FROM dsp_sync_cursor WHERE url = ?1", [url])?;
    Ok(())
}

/// A profile as last seen here: its content hash, when that content was
/// first seen, and why the server refused it, if it did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalEdit {
    pub hash: String,
    pub edited_at: i64,
    pub refused: Option<String>,
    /// What syncing did to it that its page says: a rename, and why.
    pub note: Option<String>,
}

pub fn local_edits(conn: &Connection) -> Result<HashMap<String, LocalEdit>, DbError> {
    let mut stmt = conn.prepare("SELECT uid, hash, edited_at, refused, note FROM dsp_local")?;
    Ok(stmt
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                LocalEdit {
                    hash: r.get(1)?,
                    edited_at: r.get(2)?,
                    refused: r.get(3)?,
                    note: r.get(4)?,
                },
            ))
        })?
        .collect::<Result<HashMap<_, _>, _>>()?)
}

/// Record `uid`'s content as `hash`, changed at `edited_at`, clearing any
/// refusal of the content it had.
pub fn set_local_edit(
    conn: &Connection,
    uid: &str,
    hash: &str,
    edited_at: i64,
) -> Result<(), DbError> {
    conn.execute(
        "INSERT INTO dsp_local (uid, hash, edited_at, refused) VALUES (?1, ?2, ?3, NULL)
         ON CONFLICT (uid) DO UPDATE
            SET hash = excluded.hash, edited_at = excluded.edited_at, refused = NULL",
        params![uid, hash, edited_at],
    )?;
    Ok(())
}

pub fn set_refused(conn: &Connection, uid: &str, refused: Option<&str>) -> Result<(), DbError> {
    conn.execute(
        "UPDATE dsp_local SET refused = ?2 WHERE uid = ?1",
        params![uid, refused],
    )?;
    Ok(())
}

pub fn set_note(conn: &Connection, uid: &str, note: &str) -> Result<(), DbError> {
    conn.execute(
        "INSERT INTO dsp_local (uid, hash, edited_at, note) VALUES (?1, '', 0, ?2)
         ON CONFLICT (uid) DO UPDATE SET note = excluded.note",
        params![uid, note],
    )?;
    Ok(())
}

pub fn forget_local(conn: &Connection, uid: &str) -> Result<(), DbError> {
    conn.execute("DELETE FROM dsp_local WHERE uid = ?1", [uid])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> (rusqlite::Connection, i64) {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        conn.execute(
            "INSERT INTO users (id, username, password_hash, role) VALUES (1, 'mate', 'x', 'user')",
            [],
        )
        .unwrap();
        (conn, 1)
    }

    fn now_ms() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
    }

    /// A deletion tells devices the profile is gone and keeps its last
    /// document, deleted twice included; saving it again clears it, and an
    /// old one is forgotten.
    #[test]
    fn a_deleted_profile_keeps_its_last_document() {
        let (conn, user) = db();
        let c = &conn;
        let before = now_ms();
        save(c, user, "a", 100, Some("v1")).unwrap();
        save(c, user, "a", 200, None).unwrap();
        save(c, user, "a", 300, None).unwrap();
        let (rows, _) = changes(c, user, 0).unwrap();
        assert_eq!(rows[0].doc, None, "devices see it deleted");
        let kept = deleted_docs(c, user, before).unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!((kept[0].0.as_str(), kept[0].2.as_str()), ("a", "v1"));

        save(c, user, "a", 400, Some("v2")).unwrap();
        assert!(deleted_docs(c, user, 0).unwrap().is_empty());

        save(c, user, "a", 500, None).unwrap();
        assert_eq!(expire_deleted(c, user, before).unwrap(), 0, "not old yet");
        assert_eq!(expire_deleted(c, user, now_ms() + 1).unwrap(), 1);
        assert!(deleted_docs(c, user, 0).unwrap().is_empty());
    }

    /// A deleted profile's document is found by its uid while it is kept,
    /// and only for its own account.
    #[test]
    fn a_deleted_document_is_found_by_uid() {
        let (conn, user) = db();
        let c = &conn;
        c.execute(
            "INSERT INTO users (id, username, password_hash, role) VALUES (2, 'other', 'x', 'user')",
            [],
        )
        .unwrap();
        save(c, user, "a", 100, Some("v1")).unwrap();
        assert_eq!(deleted_doc(c, user, "a", 0).unwrap(), None, "not deleted");
        save(c, user, "a", 200, None).unwrap();
        assert_eq!(
            deleted_doc(c, user, "a", 0).unwrap(),
            Some((200, "v1".into()))
        );
        assert_eq!(
            deleted_doc(c, 2, "a", 0).unwrap(),
            None,
            "another account's"
        );
        assert_eq!(
            deleted_doc(c, user, "a", now_ms() + 1).unwrap(),
            None,
            "too old"
        );
    }

    /// The window runs from when the server recorded the deletion, whatever
    /// time the deleting device's clock gave it.
    #[test]
    fn a_deletion_is_kept_by_the_servers_clock() {
        let (conn, user) = db();
        let c = &conn;
        let window = now_ms() - DELETED_KEPT_MS;
        save(c, user, "past", 1, Some("p")).unwrap();
        save(c, user, "past", 2, None).unwrap();
        let future = now_ms() + 10 * DELETED_KEPT_MS;
        save(c, user, "future", future, Some("f")).unwrap();
        save(c, user, "future", future + 1, None).unwrap();

        assert_eq!(expire_deleted(c, user, window).unwrap(), 0);
        assert_eq!(deleted_docs(c, user, window).unwrap().len(), 2);
        assert_eq!(
            expire_deleted(c, user, now_ms() + 1).unwrap(),
            2,
            "a clock ahead keeps nothing forever"
        );
    }

    /// The later edit is kept whichever arrives first, deletions included,
    /// and each change moves the account's cursor.
    #[test]
    fn the_last_edit_wins() {
        let (conn, user) = db();
        let c = &conn;
        assert_eq!(
            save(c, user, "a", 100, Some("v1")).unwrap(),
            Saved {
                rev: 1,
                stored: true
            }
        );
        assert_eq!(
            save(c, user, "a", 50, Some("old")).unwrap(),
            Saved {
                rev: 1,
                stored: false
            }
        );
        assert_eq!(
            save(c, user, "a", 200, None).unwrap(),
            Saved {
                rev: 2,
                stored: true
            }
        );
        assert!(dismiss(c, user, "AirPods").unwrap());
        assert!(!dismiss(c, user, "AirPods").unwrap());
        let (rows, cursor) = changes(c, user, 0).unwrap();
        assert_eq!(cursor, 3);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].doc, None);
        assert_eq!(changes(c, user, 3).unwrap(), (vec![], 3));
        assert!(live_docs(c, user).unwrap().is_empty());
    }

    #[test]
    fn files_no_profile_names_are_dropped() {
        let (conn, user) = db();
        let c = &conn;
        store_file(c, user, "aa", b"one").unwrap();
        store_file(c, user, "bb", b"two").unwrap();
        assert_eq!(files(c, user).unwrap().get("aa"), Some(&3));
        keep_files(c, user, &HashSet::from(["bb".to_string()])).unwrap();
        assert_eq!(file(c, user, "aa").unwrap(), None);
        assert_eq!(file(c, user, "bb").unwrap().as_deref(), Some(&b"two"[..]));
    }
}
