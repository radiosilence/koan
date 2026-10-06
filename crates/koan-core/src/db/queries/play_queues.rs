use rusqlite::{Connection, OptionalExtension, params};

// A play queue a Subsonic client saved for its account, to resume on another
// device or after a restart: one per account. kōan's own apps keep theirs on
// the device and move it between devices over the link; they neither read
// nor write this.
//
// Its entries are rows that name tracks, so a track merge moves them and a
// track removed from the library takes its entries with it. The current entry
// is held by its place in the order as saved, so entries removed before it do
// not move it onto another track; when it is removed itself, the next one that
// remains is current, or the first.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayQueue {
    /// The queue's tracks, in order. A track may appear more than once.
    pub track_ids: Vec<i64>,
    /// Index into `track_ids` of the current entry.
    pub current: usize,
    pub position_ms: i64,
    /// Seconds since the epoch.
    pub changed_at: i64,
    /// The client that saved it, as it named itself.
    pub changed_by: String,
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// Replace `user`'s play queue. `current` indexes `track_ids`; an empty list
/// clears the queue.
pub fn save_play_queue(
    conn: &Connection,
    user: i64,
    track_ids: &[i64],
    current: Option<usize>,
    position_ms: i64,
    changed_by: &str,
) -> rusqlite::Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.prepare_cached("DELETE FROM play_queue_entries WHERE user_id = ?1")?
        .execute([user])?;
    if track_ids.is_empty() {
        tx.prepare_cached("DELETE FROM play_queues WHERE user_id = ?1")?
            .execute([user])?;
        return tx.commit();
    }
    {
        let mut insert = tx.prepare_cached(
            "INSERT INTO play_queue_entries (user_id, position, track_id) VALUES (?1, ?2, ?3)",
        )?;
        for (position, track) in track_ids.iter().enumerate() {
            insert.execute(params![user, position as i64, track])?;
        }
    }
    tx.prepare_cached(
        "INSERT INTO play_queues (user_id, current_position, position_ms, changed_at, changed_by)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (user_id) DO UPDATE SET
             current_position = excluded.current_position,
             position_ms = excluded.position_ms,
             changed_at = excluded.changed_at,
             changed_by = excluded.changed_by",
    )?
    .execute(params![
        user,
        current.map(|c| c as i64),
        position_ms.max(0),
        now(),
        changed_by
    ])?;
    tx.commit()
}

/// `user`'s play queue, as far as its tracks are still in the library.
/// `None` when none was saved, or every track has gone.
pub fn play_queue(conn: &Connection, user: i64) -> rusqlite::Result<Option<PlayQueue>> {
    let Some((current_position, position_ms, changed_at, changed_by)) = conn
        .prepare_cached(
            "SELECT current_position, position_ms, changed_at, changed_by
             FROM play_queues WHERE user_id = ?1",
        )?
        .query_row([user], |r| {
            Ok((
                r.get::<_, Option<i64>>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
            ))
        })
        .optional()?
    else {
        return Ok(None);
    };
    let entries: Vec<(i64, i64)> = conn
        .prepare_cached(
            "SELECT position, track_id FROM play_queue_entries
             WHERE user_id = ?1 ORDER BY position",
        )?
        .query_map([user], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    if entries.is_empty() {
        return Ok(None);
    }
    Ok(Some(PlayQueue {
        current: current_position
            .and_then(|at| entries.iter().position(|(position, _)| *position >= at))
            .unwrap_or(0),
        track_ids: entries.into_iter().map(|(_, track)| track).collect(),
        position_ms,
        changed_at,
        changed_by,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALICE: i64 = 7;

    /// A library of three tracks, ids 1 to 3.
    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        conn.execute_batch("INSERT INTO tracks (id, title) VALUES (1, 'a'), (2, 'b'), (3, 'c');")
            .unwrap();
        conn
    }

    #[test]
    fn a_queue_saves_and_reads_back_with_its_repeats() {
        let conn = &conn();
        save_play_queue(conn, ALICE, &[1, 2, 1], Some(2), 1500, "Feishin").unwrap();
        let q = play_queue(conn, ALICE).unwrap().unwrap();
        assert_eq!(q.track_ids, [1, 2, 1]);
        assert_eq!(q.current, 2, "the second 1, not the first");
        assert_eq!((q.position_ms, q.changed_by.as_str()), (1500, "Feishin"));
        assert_eq!(
            play_queue(conn, ALICE + 1).unwrap(),
            None,
            "one per account"
        );

        save_play_queue(conn, ALICE, &[], None, 0, "Feishin").unwrap();
        assert_eq!(play_queue(conn, ALICE).unwrap(), None);
    }

    #[test]
    fn a_track_gone_from_the_library_drops_out_and_the_current_one_holds() {
        let conn = &conn();
        save_play_queue(conn, ALICE, &[1, 2, 3], Some(2), 0, "x").unwrap();
        conn.execute("DELETE FROM tracks WHERE id = 2", []).unwrap();
        let q = play_queue(conn, ALICE).unwrap().unwrap();
        assert_eq!(q.track_ids, [1, 3]);
        assert_eq!(q.current, 1, "still on 3");

        // The current one gone: the next that remains, else the first.
        save_play_queue(conn, ALICE, &[1, 3, 1], Some(1), 0, "x").unwrap();
        conn.execute("DELETE FROM tracks WHERE id = 3", []).unwrap();
        let q = play_queue(conn, ALICE).unwrap().unwrap();
        assert_eq!((q.track_ids.as_slice(), q.current), ([1, 1].as_slice(), 1));
        save_play_queue(conn, ALICE, &[1, 2], None, 0, "x").unwrap();
        assert_eq!(play_queue(conn, ALICE).unwrap().unwrap().current, 0);
    }
}
