//! Share links this koan serves itself: an unguessable id, the tracks it
//! covers in order, and when it stops working.
//!
//! A share is the only public surface a server has. What it names is what an
//! anonymous visitor can play, so it is stored as an explicit list of tracks
//! rather than as a query that could grow to include more. What it is a slice
//! of (an album, an artist, or loose tracks) is recorded beside that list so
//! the page can show it the way the app would; it never widens the list.

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::connection::DbError;

/// What a share is a slice of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ShareKind {
    /// Loose tracks, in the order given.
    #[default]
    Tracks,
    /// An album, possibly cued to one of its tracks.
    Album,
    /// An artist's albums, in release order.
    Artist,
}

impl ShareKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tracks => "tracks",
            Self::Album => "album",
            Self::Artist => "artist",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "album" => Self::Album,
            "artist" => Self::Artist,
            _ => Self::Tracks,
        }
    }
}

/// The slice, fixed when the share is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Slice {
    pub kind: ShareKind,
    /// The album or artist id; `None` for loose tracks.
    pub subject_id: Option<i64>,
    /// The track playback is cued to.
    pub start_track_id: Option<i64>,
}

impl Slice {
    pub const TRACKS: Self = Self {
        kind: ShareKind::Tracks,
        subject_id: None,
        start_track_id: None,
    };
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShareRow {
    pub id: String,
    pub description: Option<String>,
    /// Unix seconds.
    pub created_at: i64,
    /// Unix seconds; `None` never expires.
    pub expires_at: Option<i64>,
    pub visits: i64,
    pub last_visited: Option<i64>,
    pub slice: Slice,
    /// In the order they were shared.
    pub track_ids: Vec<i64>,
}

impl ShareRow {
    pub fn is_live(&self, now: i64) -> bool {
        self.expires_at.is_none_or(|e| e > now)
    }
}

/// 128 random bits, hex: an id nobody can guess or walk.
fn new_id() -> Result<String, DbError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| {
        DbError::Io(std::io::Error::other(format!(
            "no randomness for a share id: {e}"
        )))
    })?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Make a share on `user`'s behalf.
pub fn create_share(
    conn: &Connection,
    user: i64,
    slice: Slice,
    track_ids: &[i64],
    description: Option<&str>,
    created_at: i64,
    expires_at: Option<i64>,
) -> Result<ShareRow, DbError> {
    let id = new_id()?;
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO shares (id, description, created_at, expires_at, kind, subject_id, start_track_id, user_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            id,
            description,
            created_at,
            expires_at,
            slice.kind.as_str(),
            slice.subject_id,
            slice.start_track_id,
            super::auth::resolve_user(&tx, user)?
        ],
    )?;
    {
        let mut insert = tx.prepare(
            "INSERT INTO share_tracks (share_id, position, track_id) VALUES (?1, ?2, ?3)",
        )?;
        for (position, track_id) in track_ids.iter().enumerate() {
            insert.execute(params![id, position as i64, track_id])?;
        }
    }
    tx.commit()?;
    Ok(ShareRow {
        id,
        description: description.map(str::to_string),
        created_at,
        expires_at,
        visits: 0,
        last_visited: None,
        slice,
        track_ids: track_ids.to_vec(),
    })
}

fn tracks_of(conn: &Connection, id: &str) -> Result<Vec<i64>, DbError> {
    let mut stmt =
        conn.prepare("SELECT track_id FROM share_tracks WHERE share_id = ?1 ORDER BY position")?;
    let rows = stmt.query_map([id], |r| r.get(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn row(r: &rusqlite::Row) -> rusqlite::Result<ShareRow> {
    Ok(ShareRow {
        id: r.get(0)?,
        description: r.get(1)?,
        created_at: r.get(2)?,
        expires_at: r.get(3)?,
        visits: r.get(4)?,
        last_visited: r.get(5)?,
        slice: Slice {
            kind: ShareKind::parse(&r.get::<_, String>(6)?),
            subject_id: r.get(7)?,
            start_track_id: r.get(8)?,
        },
        track_ids: Vec::new(),
    })
}

const COLUMNS: &str = "id, description, created_at, expires_at, visits, last_visited, kind, subject_id, start_track_id";

pub fn get_share(conn: &Connection, id: &str) -> Result<Option<ShareRow>, DbError> {
    let Some(mut share) = conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM shares WHERE id = ?1"),
            [id],
            row,
        )
        .optional()?
    else {
        return Ok(None);
    };
    share.track_ids = tracks_of(conn, id)?;
    Ok(Some(share))
}

/// Whose shares a listing or an edit reaches: `Some(user)` for that user's
/// own, `None` for everyone's (an admin).
fn owned_by(conn: &Connection, owner: Option<i64>) -> Result<Option<i64>, DbError> {
    Ok(match owner {
        Some(user) => Some(super::auth::resolve_user(conn, user)?),
        None => None,
    })
}

/// `owner`'s shares, or everyone's for `None`. Newest first.
pub fn list_shares(conn: &Connection, owner: Option<i64>) -> Result<Vec<ShareRow>, DbError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM shares WHERE ?1 IS NULL OR user_id = ?1
         ORDER BY created_at DESC, id"
    ))?;
    let mut shares: Vec<ShareRow> = stmt
        .query_map([owned_by(conn, owner)?], row)?
        .collect::<Result<_, _>>()?;
    for share in &mut shares {
        share.track_ids = tracks_of(conn, &share.id)?;
    }
    Ok(shares)
}

/// `true` when there was such a share of `owner`'s (anyone's for `None`).
pub fn delete_share(conn: &Connection, owner: Option<i64>, id: &str) -> Result<bool, DbError> {
    Ok(conn.execute(
        "DELETE FROM shares WHERE id = ?1 AND (?2 IS NULL OR user_id = ?2)",
        params![id, owned_by(conn, owner)?],
    )? > 0)
}

/// Change the description and expiry. `true` when there was such a share of
/// `owner`'s (anyone's for `None`).
pub fn update_share(
    conn: &Connection,
    owner: Option<i64>,
    id: &str,
    description: Option<&str>,
    expires_at: Option<i64>,
) -> Result<bool, DbError> {
    Ok(conn.execute(
        "UPDATE shares SET description = ?2, expires_at = ?3
         WHERE id = ?1 AND (?4 IS NULL OR user_id = ?4)",
        params![id, description, expires_at, owned_by(conn, owner)?],
    )? > 0)
}

pub fn record_visit(conn: &Connection, id: &str, now: i64) -> Result<(), DbError> {
    // A public page, so it does not wait for the write lock: a visit that
    // lands during a scan goes uncounted.
    crate::db::connection::without_waiting(conn, |conn| {
        conn.execute(
            "UPDATE shares SET visits = visits + 1, last_visited = ?2 WHERE id = ?1",
            params![id, now],
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::queries::{sample_meta, upsert_track};

    fn test_conn() -> (Connection, [i64; 3]) {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        let t = ["One", "Two", "Three"]
            .map(|title| upsert_track(&conn, &sample_meta(title, "Artist", "Album")).unwrap());
        (conn, t)
    }

    #[test]
    fn a_share_keeps_its_tracks_in_order_and_goes_when_deleted() {
        let (conn, [t1, t2, t3]) = test_conn();
        let a = create_share(
            &conn,
            crate::db::queries::LOCAL_USER,
            Slice::TRACKS,
            &[t3, t1, t2],
            Some("mix"),
            100,
            None,
        )
        .unwrap();
        let b = create_share(
            &conn,
            crate::db::queries::LOCAL_USER,
            Slice::TRACKS,
            &[t1],
            None,
            200,
            Some(300),
        )
        .unwrap();
        assert_eq!(a.id.len(), 32);
        assert_ne!(a.id, b.id);
        assert_eq!(
            get_share(&conn, &a.id).unwrap().unwrap().track_ids,
            [t3, t1, t2]
        );
        assert_eq!(
            list_shares(&conn, None)
                .unwrap()
                .iter()
                .map(|s| s.id.clone())
                .collect::<Vec<_>>(),
            [b.id.clone(), a.id.clone()]
        );
        assert!(delete_share(&conn, None, &a.id).unwrap());
        assert!(get_share(&conn, &a.id).unwrap().is_none());
        assert!(!delete_share(&conn, None, &a.id).unwrap());
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM share_tracks WHERE share_id = ?1",
                [&a.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0, "tracks go with the share");
    }

    #[test]
    fn the_slice_is_kept_with_the_share() {
        let (conn, [t1, t2, _]) = test_conn();
        let slice = Slice {
            kind: ShareKind::Album,
            subject_id: Some(7),
            start_track_id: Some(t2),
        };
        let s = create_share(
            &conn,
            crate::db::queries::LOCAL_USER,
            slice,
            &[t1, t2],
            None,
            100,
            None,
        )
        .unwrap();
        let back = get_share(&conn, &s.id).unwrap().unwrap();
        assert_eq!(back.slice, slice);
        assert_eq!(back.track_ids, [t1, t2]);
    }

    #[test]
    fn expiry_and_visits() {
        let (conn, [t1, ..]) = test_conn();
        let s = create_share(
            &conn,
            crate::db::queries::LOCAL_USER,
            Slice::TRACKS,
            &[t1],
            None,
            100,
            Some(200),
        )
        .unwrap();
        assert!(s.is_live(199) && !s.is_live(200));
        record_visit(&conn, &s.id, 150).unwrap();
        let s = get_share(&conn, &s.id).unwrap().unwrap();
        assert_eq!((s.visits, s.last_visited), (1, Some(150)));
        assert!(update_share(&conn, None, &s.id, Some("x"), None).unwrap());
        assert!(get_share(&conn, &s.id).unwrap().unwrap().is_live(i64::MAX));
    }

    #[test]
    fn a_share_is_listed_and_changed_by_its_owner_alone() {
        let (conn, [t1, ..]) = test_conn();
        let make = |user| create_share(&conn, user, Slice::TRACKS, &[t1], None, 0, None).unwrap();
        let (a, b) = (make(1), make(2));
        let ids = |owner| -> Vec<String> {
            list_shares(&conn, owner)
                .unwrap()
                .into_iter()
                .map(|s| s.id)
                .collect()
        };

        assert_eq!(ids(Some(1)), vec![a.id.clone()]);
        assert_eq!(ids(None).len(), 2, "unscoped is everyone's");
        assert!(!update_share(&conn, Some(1), &b.id, Some("mine now"), None).unwrap());
        assert!(!delete_share(&conn, Some(1), &b.id).unwrap());
        assert!(
            get_share(&conn, &b.id)
                .unwrap()
                .unwrap()
                .description
                .is_none()
        );
        assert!(delete_share(&conn, Some(2), &b.id).unwrap());
    }
}
