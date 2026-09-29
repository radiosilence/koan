//! The `uid` each artist, album, track and playlist carries: the id every
//! surface publishes, so one item has one id on every device. Row ids number
//! each table separately and differ between databases; they stay internal.
//!
//! A server mints uids. A client syncing from a koan server adopts the
//! server's, so the same track is the same id on the server and on each
//! device, and ids pass between them untranslated. Rows only this device has,
//! and rows synced from servers that are not koan, keep one minted here.

use std::collections::HashMap;

use rusqlite::{Connection, OptionalExtension, params};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UidKind {
    Artist,
    Album,
    Track,
    Playlist,
}

impl UidKind {
    fn table(self) -> &'static str {
        match self {
            Self::Artist => "artists",
            Self::Album => "albums",
            Self::Track => "tracks",
            Self::Playlist => "playlists",
        }
    }
}

/// Whether `s` is a uid as koan writes them: a UUIDv7, hyphenated. Other
/// servers' ids are not taken for one, even those shaped like a UUID of
/// another version or spelling.
pub fn is_uid(s: &str) -> bool {
    s.len() == 36 && uuid::Uuid::try_parse(s).is_ok_and(|u| u.get_version_num() == 7)
}

/// The row of `kind` an id names: a uid, or a bare row id, which clients from
/// before uids still send. A row id is not checked for existence.
pub fn resolve_id(conn: &Connection, kind: UidKind, raw: &str) -> rusqlite::Result<Option<i64>> {
    if let Ok(id) = raw.parse() {
        return Ok(Some(id));
    }
    if !is_uid(raw) {
        return Ok(None);
    }
    id_for_uid(conn, kind, raw)
}

/// The uids of these rows, in the order given. A row without one is given as
/// its row id, which `resolve_id` still reads.
pub fn uids_in_order(
    conn: &Connection,
    kind: UidKind,
    ids: &[i64],
) -> rusqlite::Result<Vec<String>> {
    let uids = uids_for(conn, kind, ids.iter().copied())?;
    Ok(ids
        .iter()
        .map(|id| uids.get(id).cloned().unwrap_or_else(|| id.to_string()))
        .collect())
}

/// Take the server's id as this row's uid when the server is koan, whose ids
/// are uids. Another row already holding it keeps it and this row keeps its
/// own: that row is a duplicate of this one, and the unique index is not
/// broken for it.
pub fn adopt_uid(
    conn: &Connection,
    kind: UidKind,
    id: i64,
    remote_id: &str,
) -> rusqlite::Result<()> {
    if !is_uid(remote_id) {
        return Ok(());
    }
    let table = kind.table();
    let adopted = conn
        .prepare_cached(&format!(
            "UPDATE {table} SET uid = ?1
             WHERE id = ?2 AND uid IS NOT ?1
               AND NOT EXISTS (SELECT 1 FROM {table} WHERE uid = ?1)"
        ))?
        .execute(params![remote_id, id])?;
    if adopted == 0
        && let Some(other) = id_for_uid(conn, kind, remote_id)?.filter(|other| *other != id)
    {
        log::warn!(
            "{table} {id}: server id {remote_id} is already row {other}'s; kept its own uid"
        );
    }
    Ok(())
}

/// The uids of these rows, by row id. One query however many are asked for.
pub fn uids_for(
    conn: &Connection,
    kind: UidKind,
    ids: impl IntoIterator<Item = i64>,
) -> rusqlite::Result<HashMap<i64, String>> {
    let ids = serde_json::to_string(&ids.into_iter().collect::<Vec<_>>()).unwrap_or_default();
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT id, uid FROM {} WHERE id IN (SELECT value FROM json_each(?1)) AND uid IS NOT NULL",
        kind.table()
    ))?;
    stmt.query_map([ids], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect()
}

/// The row of `kind` holding `uid`.
pub fn id_for_uid(conn: &Connection, kind: UidKind, uid: &str) -> rusqlite::Result<Option<i64>> {
    conn.prepare_cached(&format!("SELECT id FROM {} WHERE uid = ?1", kind.table()))?
        .query_row(params![uid], |r| r.get(0))
        .optional()
}

/// The row holding `uid`, of whichever kind it is.
pub fn find_uid(conn: &Connection, uid: &str) -> rusqlite::Result<Option<(UidKind, i64)>> {
    for kind in [UidKind::Track, UidKind::Album, UidKind::Artist] {
        if let Some(id) = id_for_uid(conn, kind, uid)? {
            return Ok(Some((kind, id)));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_uid_names_one_row_of_one_kind() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO artists (id, name) VALUES (5, 'Burial');
             INSERT INTO albums (id, title, artist_id) VALUES (5, 'Untrue', 5);
             INSERT INTO tracks (id, title, album_id, artist_id) VALUES (5, 'Archangel', 5, 5);",
        )
        .unwrap();
        let album = uids_for(&conn, UidKind::Album, [5]).unwrap()[&5].clone();
        let track = uids_for(&conn, UidKind::Track, [5]).unwrap()[&5].clone();
        assert_ne!(album, track);
        assert_eq!(find_uid(&conn, &album).unwrap(), Some((UidKind::Album, 5)));
        assert_eq!(find_uid(&conn, &track).unwrap(), Some((UidKind::Track, 5)));
        assert_eq!(id_for_uid(&conn, UidKind::Artist, &album).unwrap(), None);
        assert_eq!(resolve_id(&conn, UidKind::Album, &album).unwrap(), Some(5));
        assert_eq!(resolve_id(&conn, UidKind::Album, "5").unwrap(), Some(5));
        assert_eq!(resolve_id(&conn, UidKind::Album, &track).unwrap(), None);
        assert_eq!(resolve_id(&conn, UidKind::Album, "al-5").unwrap(), None);
    }

    #[test]
    fn a_server_uid_another_row_holds_stays_with_it() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO artists (id, name) VALUES (1, 'Burial');
             INSERT INTO albums (id, title, artist_id) VALUES (1, 'Untrue', 1);
             INSERT INTO tracks (id, title, album_id, artist_id) VALUES
                 (1, 'Archangel', 1, 1), (2, 'Archangel', 1, 1);",
        )
        .unwrap();
        let server = uuid::Uuid::now_v7().to_string();
        adopt_uid(&conn, UidKind::Track, 1, &server).unwrap();
        let own = uids_for(&conn, UidKind::Track, [2]).unwrap()[&2].clone();

        adopt_uid(&conn, UidKind::Track, 2, &server).unwrap();

        let uids = uids_for(&conn, UidKind::Track, [1, 2]).unwrap();
        assert_eq!(uids[&1], server);
        assert_eq!(uids[&2], own);
    }

    #[test]
    fn only_a_hyphenated_uuid_v7_is_a_uid() {
        assert!(is_uid(&uuid::Uuid::now_v7().to_string()));
        assert!(!is_uid(&uuid::Uuid::now_v7().simple().to_string()));
        assert!(!is_uid("0f8fad5b-d9cb-469f-a165-70867728950e"));
        assert!(!is_uid("mf-5"));
        assert!(!is_uid("5"));
    }
}
