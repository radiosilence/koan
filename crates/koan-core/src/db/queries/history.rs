use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, params};

use crate::db::connection::DbError;

use super::TrackRow;
use super::auth::resolve_user;

/// Where a play came from. `local` is koan playing the track itself; `subsonic`
/// is another client scrobbling to koan's own Subsonic endpoint.
pub const SOURCE_LOCAL: &str = "local";
pub const SOURCE_SUBSONIC: &str = "subsonic";
/// A play another device made, adopted from the signed-in koan server's
/// history: see `remote::history`.
pub const SOURCE_SYNCED: &str = "synced";

/// How far apart, in seconds, two entries for a track can be and still be one
/// play. A device records a play when it starts and dates its scrobble to the
/// same moment, but the two clocks are read a moment apart, and the server
/// keeps whole seconds.
pub const SAME_PLAY_SECS: i64 = 5;

/// Record a play at an explicit time. Returns the new entry's id.
///
/// `listened_ms` is how long the track was actually listened to, not how long
/// the track is — an entry written the moment playback starts does not know it
/// yet, and fills it in later via [`set_listened_ms`].
pub fn record_play_at(
    conn: &Connection,
    user: i64,
    track_id: i64,
    played_at: i64,
    listened_ms: Option<i64>,
    source: &str,
) -> Result<i64, DbError> {
    conn.execute(
        "INSERT INTO play_history (user_id, track_id, played_at, duration_ms, source)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            resolve_user(conn, user)?,
            track_id,
            played_at,
            listened_ms,
            source
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Record several plays, `(track_id, played_at)`, as one transaction: all of
/// them or, if any names a track that does not exist, none. A play already
/// recorded for the track at the same second is not recorded again.
pub fn record_plays_at(
    conn: &Connection,
    user: i64,
    plays: &[(i64, i64)],
    source: &str,
) -> Result<(), DbError> {
    let user = resolve_user(conn, user)?;
    super::atomically(conn, || {
        // A client that lost the answer sends the batch again: a play already
        // recorded at that second is that play.
        let mut insert = conn.prepare_cached(
            "INSERT INTO play_history (user_id, track_id, played_at, source)
             SELECT ?1, ?2, ?3, ?4
             WHERE NOT EXISTS (
                 SELECT 1 FROM play_history
                 WHERE user_id = ?1 AND track_id = ?2 AND played_at = ?3
             )",
        )?;
        for &(track_id, played_at) in plays {
            insert.execute(params![user, track_id, played_at, source])?;
        }
        Ok(())
    })
}

/// How far back Recently played reaches, and how many of each it shows: the
/// same on every front end.
pub const RECENT_DAYS: i64 = 30;
pub const RECENT_LIMIT: u32 = 50;

/// What was played lately, each once and newest first by its latest play.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RecentlyPlayed {
    pub albums: Vec<i64>,
    /// By the record's artist, or the track's where it has no record.
    pub artists: Vec<i64>,
    pub tracks: Vec<i64>,
}

/// The records, artists and tracks `user` played since `since` (unix
/// seconds), each counted once however often it played, ordered by its latest
/// play, at most `limit` of each. Derived from the history, so plays another
/// device recorded here count as soon as they arrive.
pub fn recently_played(
    conn: &Connection,
    user: i64,
    since: i64,
    limit: u32,
) -> Result<RecentlyPlayed, DbError> {
    let user = resolve_user(conn, user)?;
    let ids = |key: &str| -> Result<Vec<i64>, DbError> {
        let sql = format!(
            "SELECT {key}
             FROM play_history h
             JOIN tracks t ON t.id = h.track_id
             LEFT JOIN albums al ON al.id = t.album_id
             WHERE h.user_id = ?1 AND h.played_at >= ?2 AND {key} IS NOT NULL
             GROUP BY {key}
             ORDER BY MAX(h.played_at) DESC, MAX(h.id) DESC
             LIMIT ?3"
        );
        let mut stmt = conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(params![user, since, limit], |row| row.get(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    };
    Ok(RecentlyPlayed {
        albums: ids("t.album_id")?,
        artists: ids("COALESCE(al.artist_id, t.artist_id)")?,
        tracks: ids("h.track_id")?,
    })
}

/// Record a play that started just now.
pub fn record_play(
    conn: &Connection,
    user: i64,
    track_id: i64,
    listened_ms: Option<i64>,
) -> Result<i64, DbError> {
    record_play_at(conn, user, track_id, now_secs(), listened_ms, SOURCE_LOCAL)
}

/// Fill in how long an entry was listened to, once that is known.
///
/// Guarded on the track so a dropped start event cannot make the previous
/// entry inherit this one's listening time.
pub fn set_listened_ms(
    conn: &Connection,
    id: i64,
    track_id: i64,
    listened_ms: i64,
) -> Result<(), DbError> {
    conn.execute(
        "UPDATE play_history SET duration_ms = ?1 WHERE id = ?2 AND track_id = ?3",
        params![listened_ms, id, track_id],
    )?;
    Ok(())
}

/// Forget specific plays of `user`'s.
pub fn delete_plays(conn: &Connection, user: i64, ids: &[i64]) -> Result<usize, DbError> {
    let user = resolve_user(conn, user)?;
    let tx = crate::db::queries::write_transaction(conn)?;
    let mut removed = 0;
    {
        let mut stmt = tx.prepare("DELETE FROM play_history WHERE id = ?1 AND user_id = ?2")?;
        for id in ids {
            removed += stmt.execute(params![id, user])?;
        }
    }
    tx.commit()?;
    Ok(removed)
}

/// Get the last play timestamp for a track, or None if never played.
pub fn last_played_at(conn: &Connection, user: i64, track_id: i64) -> Result<Option<i64>, DbError> {
    let result = conn.query_row(
        "SELECT MAX(played_at) FROM play_history WHERE track_id = ?1 AND user_id = ?2",
        params![track_id, resolve_user(conn, user)?],
        |row| row.get::<_, Option<i64>>(0),
    )?;
    Ok(result)
}

/// Get track IDs from recent play history (most recent first), up to `limit`.
///
/// Each track ordered by its latest play. `SELECT DISTINCT … ORDER BY
/// played_at` orders by whichever of a track's plays SQLite happens to keep.
pub fn recent_track_ids(conn: &Connection, user: i64, limit: usize) -> Result<Vec<i64>, DbError> {
    let mut stmt = conn.prepare_cached(
        "SELECT track_id FROM play_history
         WHERE user_id = ?2
         GROUP BY track_id
         ORDER BY MAX(played_at) DESC, MAX(id) DESC
         LIMIT ?1",
    )?;
    let rows = stmt
        .query_map(params![limit as i64, resolve_user(conn, user)?], |row| {
            row.get(0)
        })?
        .collect::<Result<Vec<i64>, _>>()?;
    Ok(rows)
}

/// Get play count for a track.
pub fn play_count(conn: &Connection, user: i64, track_id: i64) -> Result<i64, DbError> {
    let count = conn.query_row(
        "SELECT COUNT(*) FROM play_history WHERE track_id = ?1 AND user_id = ?2",
        params![track_id, resolve_user(conn, user)?],
        |row| row.get(0),
    )?;
    Ok(count)
}

/// A play history entry with full track info.
#[derive(Debug, Clone)]
pub struct PlayHistoryEntry {
    pub track_id: i64,
    pub played_at: i64,
    pub duration_ms: Option<i64>,
}

/// Get recent play history entries (most recent first).
pub fn get_play_history(
    conn: &Connection,
    user: i64,
    limit: u32,
    offset: u32,
) -> Result<Vec<PlayHistoryEntry>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT track_id, played_at, duration_ms FROM play_history
         WHERE user_id = ?3
         ORDER BY played_at DESC
         LIMIT ?1 OFFSET ?2",
    )?;
    let user = resolve_user(conn, user)?;
    let rows = stmt
        .query_map(params![limit as i64, offset as i64, user], |row| {
            Ok(PlayHistoryEntry {
                track_id: row.get(0)?,
                played_at: row.get(1)?,
                duration_ms: row.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// A play history entry joined to the track it played.
///
/// History is a list of events, not of tracks: the same track played three
/// times is three rows, so this cannot be deduplicated into a track list.
#[derive(Debug, Clone)]
pub struct PlayHistoryRow {
    pub id: i64,
    pub track: TrackRow,
    pub played_at: i64,
    pub listened_ms: Option<i64>,
    pub source: String,
}

/// Recent plays with their tracks, most recent first.
///
/// Joined rather than looked up per entry, and inner-joined so an entry whose
/// track has left the library simply does not appear.
///
/// Track columns come first so `row_to_track_row` reads them at the offsets it
/// always does; the history columns follow.
pub fn play_history_with_tracks(
    conn: &Connection,
    user: i64,
    search: Option<&str>,
    // `None` for every play ever recorded.
    limit: Option<u32>,
    offset: u32,
) -> Result<Vec<PlayHistoryRow>, DbError> {
    let mut sql = String::from(
        "SELECT t.id, t.album_id, t.artist_id, a.name, aa.name, al.title,
                t.disc, t.track_number, t.title, t.duration_ms, t.path,
                t.codec, t.sample_rate, t.bit_depth, t.channels, t.bitrate,
                t.genre, t.source, t.remote_id, t.cached_path,
                h.id, h.played_at, h.duration_ms, COALESCE(h.source, 'local')
         FROM play_history h
         JOIN tracks t ON t.id = h.track_id
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         LEFT JOIN artists aa ON al.artist_id = aa.id
         WHERE h.user_id = ?",
    );
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(resolve_user(conn, user)?)];
    if let Some(query) = search {
        let pattern = format!("%{}%", super::artists::escape_like(query));
        for _ in 0..3 {
            params.push(Box::new(pattern.clone()));
        }
        sql.push_str(
            " AND (t.title LIKE ? COLLATE NOCASE ESCAPE '\\'
                 OR a.name LIKE ? COLLATE NOCASE ESCAPE '\\'
                 OR al.title LIKE ? COLLATE NOCASE ESCAPE '\\')",
        );
    }
    sql.push_str(" ORDER BY h.played_at DESC, h.id DESC");
    if let Some(limit) = limit {
        params.push(Box::new(limit as i64));
        params.push(Box::new(offset as i64));
        sql.push_str(" LIMIT ? OFFSET ?");
    }

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(params.iter()), |row| {
            Ok(PlayHistoryRow {
                track: super::row_to_track_row(row)?,
                id: row.get(20)?,
                played_at: row.get(21)?,
                listened_ms: row.get(22)?,
                source: row.get(23)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Delete every one of `user`'s plays. Returns how many were removed.
pub fn clear_play_history(conn: &Connection, user: i64) -> Result<usize, DbError> {
    Ok(conn.execute(
        "DELETE FROM play_history WHERE user_id = ?1",
        [resolve_user(conn, user)?],
    )?)
}

// --- History shared through a server ---------------------------------------
//
// A koan server's history is the account's record. Its devices read it in
// pages after a cursor of two ids, one into `play_history` and one into
// `play_history_forgotten`; both tables are `AUTOINCREMENT`, so neither id is
// ever handed out twice. A play is named by its track's uid and when it
// started, which is what every device can agree on.

/// One page of a server's history: plays recorded and plays forgotten since
/// the cursor, oldest first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistoryPage {
    pub plays: Vec<SharedPlay>,
    pub forgotten: Vec<ForgottenPlay>,
    /// Where the next page starts: the last play and the last forgetting
    /// included.
    pub cursor: HistoryCursor,
    /// Whether either list stopped at the page size.
    pub more: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedPlay {
    /// Its id in the server's history: what a device that cannot place the
    /// track yet holds its cursor before.
    pub seq: i64,
    pub track_uid: String,
    /// Seconds since the epoch.
    pub played_at: i64,
    pub listened_ms: Option<i64>,
}

/// A play forgotten, or with no track every play up to `played_at`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgottenPlay {
    pub track_uid: Option<String>,
    pub played_at: i64,
}

/// How far a device has read a server's history.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HistoryCursor {
    pub play: i64,
    pub forgotten: i64,
}

impl HistoryCursor {
    /// `play.forgotten`, as it travels.
    pub fn parse(raw: &str) -> Option<Self> {
        let (play, forgotten) = raw.split_once('.')?;
        Some(Self {
            play: play.parse().ok()?,
            forgotten: forgotten.parse().ok()?,
        })
    }
}

impl std::fmt::Display for HistoryCursor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.play, self.forgotten)
    }
}

/// `user`'s plays and forgettings after `after`, at most `count` of each.
pub fn history_since(
    conn: &Connection,
    user: i64,
    after: HistoryCursor,
    count: u32,
) -> Result<HistoryPage, DbError> {
    let user = resolve_user(conn, user)?;
    let mut page = HistoryPage {
        cursor: after,
        ..Default::default()
    };
    let mut plays = conn.prepare_cached(
        "SELECT h.id, t.uid, h.played_at, h.duration_ms FROM play_history h
         JOIN tracks t ON t.id = h.track_id
         WHERE h.user_id = ?1 AND h.id > ?2
         ORDER BY h.id LIMIT ?3",
    )?;
    let mut rows = plays.query(params![user, after.play, count])?;
    let mut read = 0;
    while let Some(row) = rows.next()? {
        read += 1;
        page.cursor.play = row.get(0)?;
        if let Some(track_uid) = row.get::<_, Option<String>>(1)? {
            page.plays.push(SharedPlay {
                seq: page.cursor.play,
                track_uid,
                played_at: row.get(2)?,
                listened_ms: row.get(3)?,
            });
        }
    }
    let mut forgotten = conn.prepare_cached(
        "SELECT id, track_uid, played_at FROM play_history_forgotten
         WHERE user_id = ?1 AND id > ?2
         ORDER BY id LIMIT ?3",
    )?;
    let mut rows = forgotten.query(params![user, after.forgotten, count])?;
    let mut forgot = 0;
    while let Some(row) = rows.next()? {
        forgot += 1;
        page.cursor.forgotten = row.get(0)?;
        page.forgotten.push(ForgottenPlay {
            track_uid: row.get(1)?,
            played_at: row.get(2)?,
        });
    }
    page.more = read == count || forgot == count;
    Ok(page)
}

/// Delete `user`'s plays of `track_id` within [`SAME_PLAY_SECS`] of
/// `played_at`. Returns how many went.
pub fn forget_play_near(
    conn: &Connection,
    user: i64,
    track_id: i64,
    played_at: i64,
) -> Result<usize, DbError> {
    Ok(conn.execute(
        "DELETE FROM play_history
         WHERE user_id = ?1 AND track_id = ?2 AND played_at BETWEEN ?3 AND ?4",
        params![
            resolve_user(conn, user)?,
            track_id,
            played_at - SAME_PLAY_SECS,
            played_at + SAME_PLAY_SECS
        ],
    )?)
}

/// Delete every one of `user`'s plays up to and including `played_at`.
pub fn forget_plays_through(
    conn: &Connection,
    user: i64,
    played_at: i64,
) -> Result<usize, DbError> {
    Ok(conn.execute(
        "DELETE FROM play_history WHERE user_id = ?1 AND played_at <= ?2",
        params![resolve_user(conn, user)?, played_at],
    )?)
}

/// On a server: forget these plays, `(track_id, played_at)`, and record each
/// that removed anything, so the account's devices forget it too. One
/// transaction. Returns how many entries went.
pub fn forget_shared_plays(
    conn: &Connection,
    user: i64,
    plays: &[(i64, i64)],
) -> Result<usize, DbError> {
    let user = resolve_user(conn, user)?;
    super::atomically(conn, || {
        let mut removed = 0;
        for &(track_id, played_at) in plays {
            let n = forget_play_near(conn, user, track_id, played_at)?;
            if n > 0 {
                conn.execute(
                    "INSERT INTO play_history_forgotten (user_id, track_uid, played_at)
                     SELECT ?1, uid, ?3 FROM tracks WHERE id = ?2",
                    params![user, track_id, played_at],
                )?;
            }
            removed += n;
        }
        Ok(removed)
    })
}

/// On a server: forget these entries of `user`'s history by id, and record
/// each, so the account's devices forget it too. Ids that are not `user`'s
/// are left alone. One transaction. Returns how many entries went.
pub fn forget_shared_entries(conn: &Connection, user: i64, ids: &[i64]) -> Result<usize, DbError> {
    let user = resolve_user(conn, user)?;
    super::atomically(conn, || {
        let mut record = conn.prepare_cached(
            "INSERT INTO play_history_forgotten (user_id, track_uid, played_at)
             SELECT h.user_id, t.uid, h.played_at
               FROM play_history h JOIN tracks t ON t.id = h.track_id
              WHERE h.id = ?1 AND h.user_id = ?2",
        )?;
        let mut delete =
            conn.prepare_cached("DELETE FROM play_history WHERE id = ?1 AND user_id = ?2")?;
        let mut removed = 0;
        for &id in ids {
            record.execute(params![id, user])?;
            removed += delete.execute(params![id, user])?;
        }
        Ok(removed)
    })
}

/// On a server: forget every play up to `played_at`, and record it.
pub fn forget_shared_plays_through(
    conn: &Connection,
    user: i64,
    played_at: i64,
) -> Result<usize, DbError> {
    let user = resolve_user(conn, user)?;
    super::atomically(conn, || {
        let removed = forget_plays_through(conn, user, played_at)?;
        conn.execute(
            "INSERT INTO play_history_forgotten (user_id, track_uid, played_at)
             VALUES (?1, NULL, ?2)",
            params![user, played_at],
        )?;
        Ok(removed)
    })
}

/// Adopt plays from the server's history, `(track_id, played_at,
/// listened_ms)`, leaving out any this database already holds: a play this
/// device made comes back from the server it was scrobbled to. Returns how
/// many were new.
pub fn adopt_plays(
    conn: &Connection,
    user: i64,
    plays: &[(i64, i64, Option<i64>)],
) -> Result<usize, DbError> {
    let user = resolve_user(conn, user)?;
    super::atomically(conn, || {
        let mut insert = conn.prepare_cached(
            "INSERT INTO play_history (user_id, track_id, played_at, duration_ms, source)
             SELECT ?1, ?2, ?3, ?4, ?5
             WHERE NOT EXISTS (
                 SELECT 1 FROM play_history
                 WHERE user_id = ?1 AND track_id = ?2 AND played_at BETWEEN ?6 AND ?7
             )",
        )?;
        let mut adopted = 0;
        for &(track_id, played_at, listened_ms) in plays {
            adopted += insert.execute(params![
                user,
                track_id,
                played_at,
                listened_ms,
                SOURCE_SYNCED,
                played_at - SAME_PLAY_SECS,
                played_at + SAME_PLAY_SECS
            ])?;
        }
        Ok(adopted)
    })
}

/// The server's id for the track of each of these plays of `user`'s, and when
/// it started. Plays of tracks the server does not have are left out.
pub fn remote_ids_of_plays(
    conn: &Connection,
    user: i64,
    ids: &[i64],
) -> Result<Vec<(String, i64)>, DbError> {
    let user = resolve_user(conn, user)?;
    let mut stmt = conn.prepare_cached(
        "SELECT t.remote_id, h.played_at FROM play_history h
         JOIN tracks t ON t.id = h.track_id
         WHERE h.id = ?1 AND h.user_id = ?2 AND t.remote_id IS NOT NULL",
    )?;
    let mut out = Vec::new();
    for id in ids {
        if let Some(row) = stmt
            .query_row(params![id, user], |row| Ok((row.get(0)?, row.get(1)?)))
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                e => Err(e),
            })?
        {
            out.push(row);
        }
    }
    Ok(out)
}

/// What waits in the history outbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboxKind {
    Scrobble,
    Forget,
    Clear,
}

impl OutboxKind {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "scrobble" => Some(Self::Scrobble),
            "forget" => Some(Self::Forget),
            "clear" => Some(Self::Clear),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxEntry {
    pub id: i64,
    pub kind: OutboxKind,
    /// The track's id on the server; `None` for a clear.
    pub remote_id: Option<String>,
    /// When the play started, or for a clear the moment it covers up to.
    pub at_ms: i64,
}

/// Hold a scrobble until the server takes it.
pub fn queue_scrobble(conn: &Connection, remote_id: &str, at_ms: i64) -> Result<(), DbError> {
    conn.execute(
        "INSERT INTO history_outbox (kind, remote_id, at_ms) VALUES ('scrobble', ?1, ?2)",
        params![remote_id, at_ms],
    )?;
    Ok(())
}

/// Hold the forgetting of a play until the server takes it. A play whose
/// scrobble has not left yet is forgotten by not sending it.
pub fn queue_forget(conn: &Connection, remote_id: &str, at_ms: i64) -> Result<(), DbError> {
    super::atomically(conn, || {
        let slack = SAME_PLAY_SECS * 1000;
        let unsent = conn.execute(
            "DELETE FROM history_outbox
             WHERE kind = 'scrobble' AND remote_id = ?1 AND at_ms BETWEEN ?2 AND ?3",
            params![remote_id, at_ms - slack, at_ms + slack],
        )?;
        if unsent == 0 {
            conn.execute(
                "INSERT INTO history_outbox (kind, remote_id, at_ms) VALUES ('forget', ?1, ?2)",
                params![remote_id, at_ms],
            )?;
        }
        Ok(())
    })
}

/// Hold the forgetting of every play up to `at_ms`. What was waiting to be
/// sent from before then goes unsent.
pub fn queue_clear(conn: &Connection, at_ms: i64) -> Result<(), DbError> {
    super::atomically(conn, || {
        conn.execute(
            "DELETE FROM history_outbox WHERE kind = 'forget' OR at_ms <= ?1",
            params![at_ms],
        )?;
        conn.execute(
            "INSERT INTO history_outbox (kind, remote_id, at_ms) VALUES ('clear', NULL, ?1)",
            params![at_ms],
        )?;
        Ok(())
    })
}

/// The oldest `limit` entries waiting, in the order they were queued.
pub fn history_outbox(conn: &Connection, limit: u32) -> Result<Vec<OutboxEntry>, DbError> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, kind, remote_id, at_ms FROM history_outbox ORDER BY id LIMIT ?1",
    )?;
    let rows = stmt
        .query_map([limit], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, kind, remote_id, at_ms)| {
            Some(OutboxEntry {
                id,
                kind: OutboxKind::parse(&kind)?,
                remote_id,
                at_ms,
            })
        })
        .collect())
}

/// Take entries out of the outbox: sent, or refused for good.
pub fn drop_from_outbox(conn: &Connection, ids: &[i64]) -> Result<(), DbError> {
    super::atomically(conn, || {
        let mut stmt = conn.prepare_cached("DELETE FROM history_outbox WHERE id = ?1")?;
        for id in ids {
            stmt.execute([id])?;
        }
        Ok(())
    })
}

/// Where this device has read `url`'s history up to.
pub fn history_cursor(conn: &Connection, url: &str) -> Result<HistoryCursor, DbError> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT history_cursor FROM remote_servers WHERE url = ?1",
            [url],
            |row| row.get(0),
        )
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            e => Err(e),
        })?;
    Ok(raw
        .as_deref()
        .and_then(HistoryCursor::parse)
        .unwrap_or_default())
}

pub fn set_history_cursor(
    conn: &Connection,
    url: &str,
    username: &str,
    cursor: HistoryCursor,
) -> Result<(), DbError> {
    conn.execute(
        "INSERT INTO remote_servers (url, username, history_cursor)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(url) DO UPDATE SET history_cursor = ?3",
        params![url, username, cursor.to_string()],
    )?;
    Ok(())
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::Database;
    use crate::db::queries::{sample_meta, upsert_track};

    fn test_db() -> Database {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        Database { conn }
    }

    fn seed_track(db: &Database, title: &str) -> i64 {
        let mut meta = sample_meta(title, "Artist1", "Album1");
        meta.path = Some(format!("/music/{title}.flac"));
        upsert_track(&db.conn, &meta).unwrap();
        db.conn
            .query_row(
                "SELECT id FROM tracks WHERE title = ?1",
                params![title],
                |row| row.get(0),
            )
            .unwrap()
    }

    /// A record counts once however many of its tracks played, by its latest
    /// play; so do an artist and a track. Plays before `since` do not count,
    /// and each list stops at the limit.
    #[test]
    fn recently_played_is_each_once_by_its_latest_play() {
        let db = test_db();
        let track = |title: &str, artist: &str, album: &str| {
            let mut meta = sample_meta(title, artist, album);
            meta.path = Some(format!("/music/{album}/{title}.flac"));
            upsert_track(&db.conn, &meta).unwrap();
            db.conn
                .query_row(
                    "SELECT id FROM tracks WHERE title = ?1",
                    params![title],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap()
        };
        let (a1, a2, b1, old) = (
            track("A1", "Ann", "Alpha"),
            track("A2", "Ann", "Alpha"),
            track("B1", "Bob", "Beta"),
            track("O1", "Old", "Omega"),
        );
        let album = |t: i64| -> i64 {
            db.conn
                .query_row("SELECT album_id FROM tracks WHERE id = ?1", [t], |r| {
                    r.get(0)
                })
                .unwrap()
        };
        let artist = |t: i64| -> i64 {
            db.conn
                .query_row(
                    "SELECT al.artist_id FROM tracks t JOIN albums al ON al.id = t.album_id
                     WHERE t.id = ?1",
                    [t],
                    |r| r.get(0),
                )
                .unwrap()
        };
        record_plays_at(
            &db.conn,
            crate::db::queries::LOCAL_USER,
            &[(old, 10), (a1, 100), (b1, 200), (a2, 300), (a1, 400)],
            SOURCE_LOCAL,
        )
        .unwrap();

        let recent = recently_played(&db.conn, crate::db::queries::LOCAL_USER, 50, 10).unwrap();
        assert_eq!(recent.albums, vec![album(a1), album(b1)]);
        assert_eq!(recent.artists, vec![artist(a1), artist(b1)]);
        assert_eq!(recent.tracks, vec![a1, a2, b1]);

        let one = recently_played(&db.conn, crate::db::queries::LOCAL_USER, 0, 1).unwrap();
        assert_eq!(one.albums, vec![album(a1)]);
        assert_eq!(one.tracks, vec![a1]);
    }

    #[test]
    fn test_record_and_query_play_history() {
        let db = test_db();
        let track_id = seed_track(&db, "Track1");

        // No plays yet.
        assert_eq!(
            play_count(&db.conn, crate::db::queries::LOCAL_USER, track_id).unwrap(),
            0
        );
        assert!(
            last_played_at(&db.conn, crate::db::queries::LOCAL_USER, track_id)
                .unwrap()
                .is_none()
        );
        assert!(
            recent_track_ids(&db.conn, crate::db::queries::LOCAL_USER, 10)
                .unwrap()
                .is_empty()
        );

        // Record a play.
        record_play(
            &db.conn,
            crate::db::queries::LOCAL_USER,
            track_id,
            Some(240_000),
        )
        .unwrap();
        assert_eq!(
            play_count(&db.conn, crate::db::queries::LOCAL_USER, track_id).unwrap(),
            1
        );
        assert!(
            last_played_at(&db.conn, crate::db::queries::LOCAL_USER, track_id)
                .unwrap()
                .is_some()
        );

        let recent = recent_track_ids(&db.conn, crate::db::queries::LOCAL_USER, 10).unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0], track_id);

        // Record another play.
        record_play(
            &db.conn,
            crate::db::queries::LOCAL_USER,
            track_id,
            Some(240_000),
        )
        .unwrap();
        assert_eq!(
            play_count(&db.conn, crate::db::queries::LOCAL_USER, track_id).unwrap(),
            2
        );
        // Still only 1 distinct track.
        assert_eq!(
            recent_track_ids(&db.conn, crate::db::queries::LOCAL_USER, 10)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn recent_tracks_are_ordered_by_their_latest_play() {
        let db = test_db();
        let a = seed_track(&db, "A");
        let b = seed_track(&db, "B");
        let user = crate::db::queries::LOCAL_USER;
        for (track, at) in [(a, 100), (b, 200), (a, 300)] {
            record_play_at(&db.conn, user, track, at, None, SOURCE_LOCAL).unwrap();
        }
        assert_eq!(recent_track_ids(&db.conn, user, 10).unwrap(), [a, b]);
        assert_eq!(recent_track_ids(&db.conn, user, 1).unwrap(), [a]);
    }

    #[test]
    fn history_is_a_list_of_events_not_of_tracks() {
        let db = test_db();
        let a = seed_track(&db, "A");
        let b = seed_track(&db, "B");

        record_play_at(
            &db.conn,
            crate::db::queries::LOCAL_USER,
            a,
            100,
            Some(1000),
            SOURCE_LOCAL,
        )
        .unwrap();
        record_play_at(
            &db.conn,
            crate::db::queries::LOCAL_USER,
            b,
            200,
            None,
            SOURCE_SUBSONIC,
        )
        .unwrap();
        record_play_at(
            &db.conn,
            crate::db::queries::LOCAL_USER,
            a,
            300,
            Some(2000),
            SOURCE_LOCAL,
        )
        .unwrap();

        let rows =
            play_history_with_tracks(&db.conn, crate::db::queries::LOCAL_USER, None, Some(10), 0)
                .unwrap();
        assert_eq!(
            rows.iter()
                .map(|r| r.track.title.as_str())
                .collect::<Vec<_>>(),
            ["A", "B", "A"],
            "most recent first, and the same track appears once per play"
        );
        assert_eq!(rows[0].played_at, 300);
        assert_eq!(rows[0].listened_ms, Some(2000));
        assert_eq!(rows[1].source, SOURCE_SUBSONIC);
        assert_eq!(rows[1].listened_ms, None);
        assert_eq!(rows[0].track.artist_name, "Artist1");
    }

    #[test]
    fn history_narrows_on_the_track_it_played() {
        let db = test_db();
        let a = seed_track(&db, "Autumn");
        let b = seed_track(&db, "Winter");
        record_play_at(
            &db.conn,
            crate::db::queries::LOCAL_USER,
            a,
            100,
            None,
            SOURCE_LOCAL,
        )
        .unwrap();
        record_play_at(
            &db.conn,
            crate::db::queries::LOCAL_USER,
            b,
            200,
            None,
            SOURCE_LOCAL,
        )
        .unwrap();

        let titles = |q| {
            play_history_with_tracks(&db.conn, crate::db::queries::LOCAL_USER, Some(q), None, 0)
                .unwrap()
                .into_iter()
                .map(|r| r.track.title)
                .collect::<Vec<_>>()
        };
        assert_eq!(titles("autumn"), ["Autumn"]);
        assert_eq!(
            play_history_with_tracks(&db.conn, crate::db::queries::LOCAL_USER, None, None, 0)
                .unwrap()
                .len(),
            2,
            "no limit is every play ever recorded"
        );
        assert_eq!(titles("Artist1").len(), 2, "matched on the artist name");
        assert!(titles("nothing here").is_empty());
    }

    #[test]
    fn history_paginates() {
        let db = test_db();
        let id = seed_track(&db, "A");
        for at in 0..5 {
            record_play_at(
                &db.conn,
                crate::db::queries::LOCAL_USER,
                id,
                at,
                None,
                SOURCE_LOCAL,
            )
            .unwrap();
        }
        assert_eq!(
            play_history_with_tracks(&db.conn, crate::db::queries::LOCAL_USER, None, Some(2), 0)
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            play_history_with_tracks(&db.conn, crate::db::queries::LOCAL_USER, None, Some(2), 4)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            play_history_with_tracks(&db.conn, crate::db::queries::LOCAL_USER, None, Some(10), 5)
                .unwrap()
                .len(),
            0
        );
    }

    #[test]
    fn plays_within_the_same_second_keep_their_order() {
        let db = test_db();
        let a = seed_track(&db, "A");
        let b = seed_track(&db, "B");
        // played_at has one-second resolution, so a short track and its
        // successor can share a timestamp. Insertion order breaks the tie.
        record_play_at(
            &db.conn,
            crate::db::queries::LOCAL_USER,
            a,
            42,
            None,
            SOURCE_LOCAL,
        )
        .unwrap();
        record_play_at(
            &db.conn,
            crate::db::queries::LOCAL_USER,
            b,
            42,
            None,
            SOURCE_LOCAL,
        )
        .unwrap();

        let rows =
            play_history_with_tracks(&db.conn, crate::db::queries::LOCAL_USER, None, Some(10), 0)
                .unwrap();
        assert_eq!(
            rows.iter()
                .map(|r| r.track.title.as_str())
                .collect::<Vec<_>>(),
            ["B", "A"]
        );
    }

    #[test]
    fn deleting_a_track_takes_its_history_with_it() {
        let db = test_db();
        let id = seed_track(&db, "A");
        record_play(&db.conn, crate::db::queries::LOCAL_USER, id, None).unwrap();

        db.conn
            .execute("DELETE FROM tracks WHERE id = ?1", params![id])
            .expect("a track with play history must still be deletable");

        assert_eq!(
            play_count(&db.conn, crate::db::queries::LOCAL_USER, id).unwrap(),
            0
        );
        assert!(
            play_history_with_tracks(&db.conn, crate::db::queries::LOCAL_USER, None, Some(10), 0)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn listening_time_lands_on_the_entry_it_belongs_to() {
        let db = test_db();
        let a = seed_track(&db, "A");
        let b = seed_track(&db, "B");

        let first = record_play(&db.conn, crate::db::queries::LOCAL_USER, a, None).unwrap();
        let second = record_play(&db.conn, crate::db::queries::LOCAL_USER, b, None).unwrap();

        set_listened_ms(&db.conn, second, b, 4_200).unwrap();
        // A start event that never landed must not push its track's time onto
        // whatever entry happens to be open.
        set_listened_ms(&db.conn, first, b, 9_999).unwrap();

        let rows =
            play_history_with_tracks(&db.conn, crate::db::queries::LOCAL_USER, None, Some(10), 0)
                .unwrap();
        let by_id: Vec<_> = rows.iter().map(|r| (r.id, r.listened_ms)).collect();
        assert!(by_id.contains(&(second, Some(4_200))));
        assert!(
            by_id.contains(&(first, None)),
            "the mismatched update was refused"
        );
    }

    #[test]
    fn plays_can_be_forgotten_individually() {
        let db = test_db();
        let id = seed_track(&db, "A");
        let first = record_play(&db.conn, crate::db::queries::LOCAL_USER, id, None).unwrap();
        let second = record_play(&db.conn, crate::db::queries::LOCAL_USER, id, None).unwrap();
        let third = record_play(&db.conn, crate::db::queries::LOCAL_USER, id, None).unwrap();

        assert_eq!(
            delete_plays(&db.conn, crate::db::queries::LOCAL_USER, &[first, third]).unwrap(),
            2
        );

        let left =
            play_history_with_tracks(&db.conn, crate::db::queries::LOCAL_USER, None, Some(10), 0)
                .unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].id, second);
        assert_eq!(
            play_count(&db.conn, crate::db::queries::LOCAL_USER, id).unwrap(),
            1,
            "and the play count follows"
        );
    }

    #[test]
    fn forgetting_an_entry_that_is_already_gone_is_not_an_error() {
        let db = test_db();
        assert_eq!(
            delete_plays(&db.conn, crate::db::queries::LOCAL_USER, &[404]).unwrap(),
            0
        );
        assert_eq!(
            delete_plays(&db.conn, crate::db::queries::LOCAL_USER, &[]).unwrap(),
            0
        );
    }

    #[test]
    fn clearing_removes_everything() {
        let db = test_db();
        let id = seed_track(&db, "A");
        record_play(&db.conn, crate::db::queries::LOCAL_USER, id, None).unwrap();
        record_play(&db.conn, crate::db::queries::LOCAL_USER, id, None).unwrap();

        assert_eq!(
            clear_play_history(&db.conn, crate::db::queries::LOCAL_USER).unwrap(),
            2
        );
        assert_eq!(
            play_count(&db.conn, crate::db::queries::LOCAL_USER, id).unwrap(),
            0
        );
    }

    const USER: i64 = crate::db::queries::LOCAL_USER;

    fn plays_of(db: &Database, track: i64) -> Vec<(i64, String)> {
        let mut stmt = db
            .conn
            .prepare(
                "SELECT played_at, source FROM play_history WHERE track_id = ?1 ORDER BY played_at",
            )
            .unwrap();
        stmt.query_map([track], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    #[test]
    fn a_batch_sent_again_records_each_play_once() {
        let db = test_db();
        let a = seed_track(&db, "A");
        let batch = [(a, 100), (a, 200)];
        record_plays_at(&db.conn, USER, &batch, SOURCE_SUBSONIC).unwrap();
        record_plays_at(&db.conn, USER, &batch, SOURCE_SUBSONIC).unwrap();
        assert_eq!(play_count(&db.conn, USER, a).unwrap(), 2);
        record_plays_at(&db.conn, USER, &[(a, 300)], SOURCE_SUBSONIC).unwrap();
        assert_eq!(play_count(&db.conn, USER, a).unwrap(), 3);
    }

    #[test]
    fn history_pages_after_its_cursor() {
        let db = test_db();
        let a = seed_track(&db, "A");
        for at in [100, 200, 300] {
            record_play_at(&db.conn, USER, a, at, None, SOURCE_SUBSONIC).unwrap();
        }
        let first = history_since(&db.conn, USER, HistoryCursor::default(), 2).unwrap();
        assert_eq!(
            first.plays.iter().map(|p| p.played_at).collect::<Vec<_>>(),
            [100, 200]
        );
        assert!(first.more);
        let rest = history_since(&db.conn, USER, first.cursor, 2).unwrap();
        assert_eq!(
            rest.plays.iter().map(|p| p.played_at).collect::<Vec<_>>(),
            [300]
        );
        assert!(!rest.more);
        assert_eq!(
            HistoryCursor::parse(&rest.cursor.to_string()),
            Some(rest.cursor)
        );

        // Forgetting the newest play and recording another: the new one's id
        // is past the cursor, never the forgotten one's again.
        forget_shared_plays(&db.conn, USER, &[(a, 302)]).unwrap();
        record_play_at(&db.conn, USER, a, 400, None, SOURCE_SUBSONIC).unwrap();
        let next = history_since(&db.conn, USER, rest.cursor, 10).unwrap();
        assert_eq!(
            next.plays.iter().map(|p| p.played_at).collect::<Vec<_>>(),
            [400]
        );
        assert_eq!(next.forgotten.len(), 1);
        assert_eq!(next.forgotten[0].played_at, 302);
        assert!(next.forgotten[0].track_uid.is_some());
    }

    #[test]
    fn a_forgetting_is_recorded_only_when_it_removed_a_play() {
        let db = test_db();
        let a = seed_track(&db, "A");
        record_play_at(&db.conn, USER, a, 1000, None, SOURCE_SUBSONIC).unwrap();
        assert_eq!(
            forget_shared_plays(&db.conn, USER, &[(a, 5000)]).unwrap(),
            0
        );
        assert_eq!(
            forget_shared_plays(&db.conn, USER, &[(a, 1004)]).unwrap(),
            1
        );
        let page = history_since(&db.conn, USER, HistoryCursor::default(), 10).unwrap();
        assert_eq!(page.forgotten.len(), 1);

        record_play_at(&db.conn, USER, a, 2000, None, SOURCE_SUBSONIC).unwrap();
        assert_eq!(
            forget_shared_plays_through(&db.conn, USER, 3000).unwrap(),
            1
        );
        let page = history_since(&db.conn, USER, page.cursor, 10).unwrap();
        assert_eq!(
            page.forgotten,
            [ForgottenPlay {
                track_uid: None,
                played_at: 3000
            }]
        );
    }

    #[test]
    fn adopting_leaves_out_plays_already_here() {
        let db = test_db();
        let a = seed_track(&db, "A");
        // This device's own play, which comes back from the server a second
        // off.
        record_play_at(&db.conn, USER, a, 1000, None, SOURCE_LOCAL).unwrap();
        let adopted = adopt_plays(
            &db.conn,
            USER,
            &[(a, 1001, Some(200_000)), (a, 5000, None), (a, 5000, None)],
        )
        .unwrap();
        assert_eq!(adopted, 1, "the second copy of 5000 is the first one");
        assert_eq!(
            plays_of(&db, a),
            [
                (1000, SOURCE_LOCAL.to_owned()),
                (5000, SOURCE_SYNCED.to_owned())
            ]
        );
    }

    #[test]
    fn forgetting_a_play_not_yet_sent_unsends_it() {
        let db = test_db();
        queue_scrobble(&db.conn, "x", 10_000).unwrap();
        queue_scrobble(&db.conn, "y", 10_000).unwrap();
        queue_forget(&db.conn, "x", 11_000).unwrap();
        queue_forget(&db.conn, "z", 20_000).unwrap();
        let waiting = history_outbox(&db.conn, 10).unwrap();
        assert_eq!(
            waiting
                .iter()
                .map(|e| (e.kind, e.remote_id.as_deref()))
                .collect::<Vec<_>>(),
            [
                (OutboxKind::Scrobble, Some("y")),
                (OutboxKind::Forget, Some("z"))
            ]
        );

        // A clear supersedes everything from before it.
        queue_scrobble(&db.conn, "w", 40_000).unwrap();
        queue_clear(&db.conn, 30_000).unwrap();
        let waiting = history_outbox(&db.conn, 10).unwrap();
        assert_eq!(
            waiting.iter().map(|e| e.kind).collect::<Vec<_>>(),
            [OutboxKind::Scrobble, OutboxKind::Clear]
        );
        drop_from_outbox(&db.conn, &waiting.iter().map(|e| e.id).collect::<Vec<_>>()).unwrap();
        assert!(history_outbox(&db.conn, 10).unwrap().is_empty());
    }

    #[test]
    fn the_cursor_is_kept_per_server() {
        let db = test_db();
        let url = "https://music.example.com";
        assert_eq!(
            history_cursor(&db.conn, url).unwrap(),
            HistoryCursor::default()
        );
        let at = HistoryCursor {
            play: 7,
            forgotten: 2,
        };
        set_history_cursor(&db.conn, url, "me", at).unwrap();
        assert_eq!(history_cursor(&db.conn, url).unwrap(), at);
        assert_eq!(
            history_cursor(&db.conn, "https://other.example.com").unwrap(),
            HistoryCursor::default()
        );
    }
}
