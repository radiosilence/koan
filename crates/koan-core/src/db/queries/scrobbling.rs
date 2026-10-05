//! Listening services an account forwards its plays to, and the plays waiting
//! to reach them. The sending is `crate::scrobbling`'s.
//!
//! A play reaches the outbox two ways: the `scrobble_reported_play` trigger
//! queues each play a client reports as it is recorded, and [`connect`] queues
//! the account's whole history when a service is connected.

use rusqlite::{Connection, OptionalExtension, params};

use crate::auth::now_unix;

pub const LISTENBRAINZ: &str = "listenbrainz";

/// A connected service, as the account's settings page shows it.
#[derive(Debug, Clone)]
pub struct ScrobbleService {
    pub service: String,
    pub account_name: String,
    pub connected_at: i64,
    pub error: Option<String>,
    /// Plays not yet accepted by the service.
    pub pending: i64,
}

/// What the sender needs to submit to one account's service.
#[derive(Debug, Clone)]
pub struct ScrobbleTarget {
    pub user_id: i64,
    pub service: String,
    pub token: String,
}

/// A queued play with the track metadata a service is sent.
#[derive(Debug, Clone)]
pub struct QueuedListen {
    pub outbox_id: i64,
    pub listen: Listen,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Listen {
    pub played_at: i64,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_ms: Option<i64>,
    pub track_number: Option<i32>,
    pub recording_mbid: Option<String>,
    pub release_mbid: Option<String>,
}

const LISTEN_COLUMNS: &str = "t.title, COALESCE(ar.name, ''), al.title, t.duration_ms,
     t.track_number, t.mbid, al.mbid";

fn listen_from(row: &rusqlite::Row<'_>, played_at: i64, at: usize) -> rusqlite::Result<Listen> {
    let blank = |s: Option<String>| s.filter(|s| !s.trim().is_empty());
    Ok(Listen {
        played_at,
        title: row.get(at)?,
        artist: row.get(at + 1)?,
        album: blank(row.get(at + 2)?),
        duration_ms: row.get(at + 3)?,
        track_number: row.get(at + 4)?,
        recording_mbid: blank(row.get(at + 5)?),
        release_mbid: blank(row.get(at + 6)?),
    })
}

/// Connect `service` for `user`, replacing any earlier connection, and queue
/// every play in the account's history that counts as heard. Returns how many
/// plays were queued.
///
/// Plays clients reported count as they are. koan's own playback records how
/// long each track was listened to, and those count by Last.fm's rule: half
/// the track or four minutes, never a track under thirty seconds.
pub fn connect(
    conn: &Connection,
    user: i64,
    service: &str,
    token: &str,
    account_name: &str,
) -> rusqlite::Result<usize> {
    super::atomically(conn, || {
        // Replacing the row clears its outbox through the foreign key, so a
        // reconnect queues the history once rather than on top of itself.
        conn.execute(
            "DELETE FROM scrobble_services WHERE user_id = ?1 AND service = ?2",
            params![user, service],
        )?;
        conn.execute(
            "INSERT INTO scrobble_services (user_id, service, token, account_name, connected_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![user, service, token, account_name, now_unix() as i64],
        )?;
        conn.execute(
            "INSERT INTO scrobble_outbox (user_id, service, history_id)
             SELECT ?1, ?2, h.id FROM play_history h JOIN tracks t ON t.id = h.track_id
              WHERE h.user_id = ?1
                AND (h.source = 'subsonic'
                     OR CASE WHEN t.duration_ms > 0
                             THEN t.duration_ms >= 30000
                                  AND h.duration_ms >= MIN(t.duration_ms / 2, 240000)
                             ELSE h.duration_ms >= 240000 END)
              ORDER BY h.played_at",
            params![user, service],
        )
    })
}

/// Stop forwarding to `service`; what was still queued for it is dropped.
pub fn disconnect(conn: &Connection, user: i64, service: &str) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM scrobble_services WHERE user_id = ?1 AND service = ?2",
        params![user, service],
    )?;
    Ok(())
}

/// The services `user` forwards to.
pub fn services(conn: &Connection, user: i64) -> rusqlite::Result<Vec<ScrobbleService>> {
    let mut stmt = conn.prepare(
        "SELECT s.service, s.account_name, s.connected_at, s.error,
                (SELECT COUNT(*) FROM scrobble_outbox o
                  WHERE o.user_id = s.user_id AND o.service = s.service)
           FROM scrobble_services s WHERE s.user_id = ?1 ORDER BY s.service",
    )?;
    stmt.query_map([user], |r| {
        Ok(ScrobbleService {
            service: r.get(0)?,
            account_name: r.get(1)?,
            connected_at: r.get(2)?,
            error: r.get(3)?,
            pending: r.get(4)?,
        })
    })?
    .collect()
}

/// The credential to submit to `service` for `user`, while it is accepted.
pub fn target(conn: &Connection, user: i64, service: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT token FROM scrobble_services
          WHERE user_id = ?1 AND service = ?2 AND error IS NULL",
        params![user, service],
        |r| r.get(0),
    )
    .optional()
}

/// An account and service with plays waiting, whose credential has not been
/// refused. Accounts are taken in turn: the one whose oldest waiting play is
/// oldest goes first.
pub fn next_target(conn: &Connection) -> rusqlite::Result<Option<ScrobbleTarget>> {
    conn.query_row(
        "SELECT s.user_id, s.service, s.token
           FROM scrobble_services s
           JOIN (SELECT user_id, service, MIN(id) AS first FROM scrobble_outbox
                  GROUP BY user_id, service) o
             ON o.user_id = s.user_id AND o.service = s.service
          WHERE s.error IS NULL
          ORDER BY o.first LIMIT 1",
        [],
        |r| {
            Ok(ScrobbleTarget {
                user_id: r.get(0)?,
                service: r.get(1)?,
                token: r.get(2)?,
            })
        },
    )
    .optional()
}

/// Up to `limit` of the plays waiting for one account's service, oldest first.
pub fn queued(
    conn: &Connection,
    target: &ScrobbleTarget,
    limit: usize,
) -> rusqlite::Result<Vec<QueuedListen>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT o.id, h.played_at, {LISTEN_COLUMNS}
           FROM scrobble_outbox o
           JOIN play_history h ON h.id = o.history_id
           JOIN tracks t ON t.id = h.track_id
           LEFT JOIN artists ar ON ar.id = t.artist_id
           LEFT JOIN albums al ON al.id = t.album_id
          WHERE o.user_id = ?1 AND o.service = ?2
          ORDER BY o.id LIMIT ?3"
    ))?;
    stmt.query_map(params![target.user_id, target.service, limit as i64], |r| {
        Ok(QueuedListen {
            outbox_id: r.get(0)?,
            listen: listen_from(r, r.get(1)?, 2)?,
        })
    })?
    .collect()
}

/// A track as a now-playing notice describes it.
pub fn now_playing(conn: &Connection, track_id: i64) -> rusqlite::Result<Option<Listen>> {
    conn.query_row(
        &format!(
            "SELECT {LISTEN_COLUMNS} FROM tracks t
               LEFT JOIN artists ar ON ar.id = t.artist_id
               LEFT JOIN albums al ON al.id = t.album_id
              WHERE t.id = ?1"
        ),
        [track_id],
        |r| listen_from(r, now_unix() as i64, 0),
    )
    .optional()
}

/// Forget queued plays: sent, or refused by the service as they stand.
pub fn dequeue(conn: &Connection, outbox_ids: &[i64]) -> rusqlite::Result<()> {
    super::atomically(conn, || {
        let mut stmt = conn.prepare_cached("DELETE FROM scrobble_outbox WHERE id = ?1")?;
        for id in outbox_ids {
            stmt.execute([id])?;
        }
        Ok(())
    })
}

/// The service refused the account's credential. Nothing more is sent for it
/// until it is connected again; the plays stay queued until then.
pub fn refuse(conn: &Connection, user: i64, service: &str, error: &str) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE scrobble_services SET error = ?3 WHERE user_id = ?1 AND service = ?2",
        params![user, service, error],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::Database;
    use crate::db::queries::{SOURCE_LOCAL, SOURCE_SUBSONIC, record_play_at, record_plays_at};

    fn setup() -> (Database, i64, i64) {
        let db = Database::open_memory().unwrap();
        db.conn
            .execute_batch(
                "INSERT INTO users (id, username, password_hash, role) VALUES (1, 'mate', 'x', 'user');
                 INSERT INTO artists (id, name) VALUES (1, 'Burial');
                 INSERT INTO albums (id, title, artist_id) VALUES (1, 'Untrue', 1);
                 INSERT INTO tracks (id, album_id, artist_id, title, duration_ms, track_number, mbid)
                     VALUES (1, 1, 1, 'Archangel', 238000, 2, 'rec-1');",
            )
            .unwrap();
        (db, 1, 1)
    }

    #[test]
    fn connecting_queues_heard_history_only() {
        let (db, user, track) = setup();
        record_plays_at(&db.conn, user, &[(track, 100)], SOURCE_SUBSONIC).unwrap();
        // Local plays count by how long they were heard.
        record_play_at(&db.conn, user, track, 200, Some(130_000), SOURCE_LOCAL).unwrap();
        record_play_at(&db.conn, user, track, 300, Some(5_000), SOURCE_LOCAL).unwrap();
        record_play_at(&db.conn, user, track, 400, None, SOURCE_LOCAL).unwrap();

        let queued_count = connect(&db.conn, user, LISTENBRAINZ, "tok", "mate").unwrap();
        assert_eq!(queued_count, 2);

        let target = next_target(&db.conn).unwrap().unwrap();
        assert_eq!(target.token, "tok");
        let listens = queued(&db.conn, &target, 10).unwrap();
        let times: Vec<i64> = listens.iter().map(|q| q.listen.played_at).collect();
        assert_eq!(times, vec![100, 200]);
        assert_eq!(listens[0].listen.artist, "Burial");
        assert_eq!(listens[0].listen.album.as_deref(), Some("Untrue"));
        assert_eq!(listens[0].listen.recording_mbid.as_deref(), Some("rec-1"));
    }

    #[test]
    fn reported_plays_are_queued_as_they_are_recorded() {
        let (db, user, track) = setup();
        record_plays_at(&db.conn, user, &[(track, 100)], SOURCE_SUBSONIC).unwrap();
        assert!(
            next_target(&db.conn).unwrap().is_none(),
            "nothing connected"
        );

        connect(&db.conn, user, LISTENBRAINZ, "tok", "mate").unwrap();
        record_plays_at(&db.conn, user, &[(track, 500)], SOURCE_SUBSONIC).unwrap();
        // Written when a track starts, before it is known to be heard.
        record_play_at(&db.conn, user, track, 600, None, SOURCE_LOCAL).unwrap();

        let target = next_target(&db.conn).unwrap().unwrap();
        let listens = queued(&db.conn, &target, 10).unwrap();
        let times: Vec<i64> = listens.iter().map(|q| q.listen.played_at).collect();
        assert_eq!(times, vec![100, 500]);

        dequeue(&db.conn, &[listens[0].outbox_id]).unwrap();
        assert_eq!(services(&db.conn, user).unwrap()[0].pending, 1);
    }

    #[test]
    fn a_refused_credential_holds_the_queue_and_reconnecting_requeues_once() {
        let (db, user, track) = setup();
        record_plays_at(
            &db.conn,
            user,
            &[(track, 100), (track, 200)],
            SOURCE_SUBSONIC,
        )
        .unwrap();
        connect(&db.conn, user, LISTENBRAINZ, "tok", "mate").unwrap();

        refuse(&db.conn, user, LISTENBRAINZ, "Invalid token").unwrap();
        assert!(next_target(&db.conn).unwrap().is_none());
        assert!(target(&db.conn, user, LISTENBRAINZ).unwrap().is_none());
        assert_eq!(services(&db.conn, user).unwrap()[0].pending, 2);

        connect(&db.conn, user, LISTENBRAINZ, "tok2", "mate").unwrap();
        let svc = &services(&db.conn, user).unwrap()[0];
        assert_eq!((svc.pending, svc.error.as_deref()), (2, None));

        disconnect(&db.conn, user, LISTENBRAINZ).unwrap();
        assert!(services(&db.conn, user).unwrap().is_empty());
        let left: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM scrobble_outbox", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0);
    }

    #[test]
    fn forgetting_a_play_drops_it_from_the_queue() {
        let (db, user, track) = setup();
        connect(&db.conn, user, LISTENBRAINZ, "tok", "mate").unwrap();
        record_plays_at(&db.conn, user, &[(track, 100)], SOURCE_SUBSONIC).unwrap();
        db.conn.execute("DELETE FROM play_history", []).unwrap();
        assert!(next_target(&db.conn).unwrap().is_none());
    }
}
