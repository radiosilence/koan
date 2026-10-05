//! Smart playlists in the database: rules compiled to SQL, and a playlist's
//! entries kept in step with what its rules select.
//!
//! A smart playlist is an ordinary `playlists` row with `rules` set. Its
//! entries are what the rules selected when they were last evaluated, so
//! everything that reads playlists reads it unchanged. Evaluation happens when
//! the playlist is read and its last evaluation is old enough (see
//! [`refresh_due`]); entries are rewritten only when the selection changed, so
//! a read that finds nothing new writes nothing.

use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, params, params_from_iter};

use super::auth::resolve_user;
use crate::db::connection::DbError;
use crate::smart::{Condition, Field, Kind, Match, Op, Operand, Rule, Rules};

/// How long an evaluation stands before a read evaluates again. Short, so a
/// play or a scan shows up the next time the playlist is looked at; long
/// enough that the reads a client makes back to back (the list, then each
/// playlist) evaluate once.
pub const REFRESH_SECS: i64 = 60;

/// How long a random order stands. Drawing it again on every read would make
/// it a different playlist each time a client synced.
pub const RANDOM_REFRESH_SECS: i64 = 24 * 60 * 60;

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// A track column or derived value, as SQL over the joins in [`FROM`].
fn column(field: Field) -> &'static str {
    match field {
        Field::Title => "t.title",
        Field::Artist => "a.name",
        Field::AlbumArtist => "aa.name",
        Field::Album => "al.title",
        Field::Genre => "t.genre",
        Field::Format => "t.codec",
        Field::Path => "t.path",
        Field::Year => "CAST(substr(al.date, 1, 4) AS INTEGER)",
        Field::Duration => "t.duration_ms",
        Field::BitDepth => "t.bit_depth",
        Field::SampleRate => "t.sample_rate",
        Field::TrackNumber => "t.track_number",
        Field::DiscNumber => "t.disc",
        Field::PlayCount => "COALESCE(ph.plays, 0)",
        Field::LastPlayed => "ph.last_played",
        Field::DateAdded => "CAST(strftime('%s', al.added_at) AS INTEGER)",
        Field::Favourite => "(fav.track_id IS NOT NULL)",
    }
}

/// What a rule's number is multiplied by to compare with the column: duration
/// is written in seconds and stored in milliseconds.
fn scale(field: Field) -> f64 {
    match field {
        Field::Duration => 1000.0,
        _ => 1.0,
    }
}

/// `?1` is the owner, whose plays and favourites the per-account fields read.
const FROM: &str = "FROM tracks t
     LEFT JOIN artists a ON a.id = t.artist_id
     LEFT JOIN albums al ON al.id = t.album_id
     LEFT JOIN artists aa ON aa.id = al.artist_id
     LEFT JOIN (SELECT track_id, COUNT(*) AS plays, MAX(played_at) AS last_played
                FROM play_history WHERE user_id = ?1 GROUP BY track_id) ph
            ON ph.track_id = t.id
     LEFT JOIN favourites fav ON fav.track_id = t.id AND fav.user_id = ?1";

/// Album order: what ties fall back to, and the order when none is asked for.
const ALBUM_ORDER: &str =
    "aa.name COLLATE LIBRARY, al.title COLLATE LIBRARY, t.disc, t.track_number, t.id";

/// Rules as one query selecting track ids for `user` (resolved), with its
/// parameters. Every value a rule carries is a parameter; the SQL text comes
/// only from the tables above.
pub fn compile(rules: &Rules, user: i64, now: i64) -> Result<(String, Vec<SqlValue>), String> {
    rules.check()?;
    let mut params = vec![SqlValue::Integer(user)];
    let filter = group(rules.matching, &rules.rules, &mut params, now)?;
    let mut order: Vec<String> = rules
        .sort
        .iter()
        .map(|s| {
            let dir = if s.desc { "DESC" } else { "ASC" };
            match s.field {
                None => "RANDOM()".to_owned(),
                Some(f) if f.kind() == Kind::Text => {
                    format!("{} COLLATE LIBRARY {dir}", column(f))
                }
                Some(f) => format!("{} {dir}", column(f)),
            }
        })
        .collect();
    order.push(ALBUM_ORDER.to_owned());
    let mut sql = format!(
        "SELECT t.id {FROM} WHERE {filter} ORDER BY {}",
        order.join(", ")
    );
    if let Some(limit) = rules.limit {
        params.push(SqlValue::Integer(limit.into()));
        sql.push_str(&format!(" LIMIT ?{}", params.len()));
    }
    Ok((sql, params))
}

fn group(
    matching: Match,
    rules: &[Condition],
    params: &mut Vec<SqlValue>,
    now: i64,
) -> Result<String, String> {
    if rules.is_empty() {
        // Nothing to satisfy: all of nothing holds, any of nothing does not.
        return Ok(match matching {
            Match::All => "1".into(),
            Match::Any => "0".into(),
        });
    }
    let parts = rules
        .iter()
        .map(|c| match c {
            Condition::Group { matching, rules } => group(*matching, rules, params, now),
            Condition::Rule(rule) => condition(rule, params, now),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let joiner = match matching {
        Match::All => " AND ",
        Match::Any => " OR ",
    };
    Ok(format!("({})", parts.join(joiner)))
}

fn condition(rule: &Rule, params: &mut Vec<SqlValue>, now: i64) -> Result<String, String> {
    let col = column(rule.field);
    let mut bind = |v: SqlValue| {
        params.push(v);
        format!("?{}", params.len())
    };
    let scale = scale(rule.field);
    Ok(match rule.operand()? {
        Operand::Text(text) => {
            // Compared as names are elsewhere: case and spacing folded.
            let folded = super::sources::fold(&text);
            let col = format!("COALESCE(koan_fold({col}), '')");
            let p = bind(SqlValue::Text(folded));
            match rule.op {
                Op::Is => format!("{col} = {p}"),
                Op::IsNot => format!("{col} != {p}"),
                Op::Contains => format!("instr({col}, {p}) > 0"),
                Op::NotContains => format!("instr({col}, {p}) = 0"),
                Op::StartsWith => format!("substr({col}, 1, length({p})) = {p}"),
                Op::EndsWith => {
                    format!("length({col}) >= length({p}) AND substr({col}, -length({p})) = {p}")
                }
                _ => unreachable!("checked by operand"),
            }
        }
        Operand::Number(n) => {
            let p = bind(SqlValue::Real(n * scale));
            match rule.op {
                Op::Is => format!("{col} = {p}"),
                Op::IsNot => format!("{col} IS NOT {p}"),
                Op::Gt => format!("{col} > {p}"),
                Op::Lt => format!("{col} < {p}"),
                _ => unreachable!("checked by operand"),
            }
        }
        Operand::NumberRange(lo, hi) => {
            let (lo, hi) = (
                bind(SqlValue::Real(lo * scale)),
                bind(SqlValue::Real(hi * scale)),
            );
            format!("{col} BETWEEN {lo} AND {hi}")
        }
        Operand::Bool(b) => {
            let wanted = b == (rule.op == Op::Is);
            if wanted {
                col.to_owned()
            } else {
                format!("NOT {col}")
            }
        }
        Operand::Days(days) => {
            let cutoff = bind(SqlValue::Integer(now - (days * 86_400.0) as i64));
            match rule.op {
                Op::InTheLast => format!("{col} >= {cutoff}"),
                // Never is not within the last anything.
                Op::NotInTheLast => format!("({col} IS NULL OR {col} < {cutoff})"),
                _ => unreachable!("checked by operand"),
            }
        }
        Operand::Instant(at) => match rule.op {
            Op::Before => format!("{col} < {}", bind(SqlValue::Integer(at))),
            // After the day named, not after its first second.
            Op::After => format!("{col} > {}", bind(SqlValue::Integer(at + 86_399))),
            _ => unreachable!("checked by operand"),
        },
        Operand::InstantRange(lo, hi) => {
            let (lo, hi) = (bind(SqlValue::Integer(lo)), bind(SqlValue::Integer(hi)));
            format!("{col} BETWEEN {lo} AND {hi}")
        }
    })
}

/// The track ids the rules select for `user`, in order.
pub fn evaluate(conn: &Connection, rules: &Rules, user: i64) -> Result<Vec<i64>, DbError> {
    let user = resolve_user(conn, user)?;
    let (sql, params) = compile(rules, user, now_secs()).map_err(DbError::InvalidRules)?;
    let mut stmt = conn.prepare(&sql)?;
    let ids = stmt
        .query_map(params_from_iter(params), |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    Ok(ids)
}

/// A playlist's rules, if it is a smart one. Rules that no longer parse (a
/// build that knows fewer fields than the one that wrote them) are `None`, and
/// the playlist keeps its last contents.
pub fn playlist_rules(conn: &Connection, id: i64) -> Result<Option<Rules>, DbError> {
    let json: Option<String> = conn
        .query_row(
            "SELECT rules FROM playlists WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            e => Err(e),
        })?;
    Ok(json.and_then(|json| match Rules::parse(&json) {
        Ok(rules) => Some(rules),
        Err(e) => {
            log::warn!("playlist {id}: its rules do not parse ({e}); keeping its contents");
            None
        }
    }))
}

/// Make `id` a smart playlist with these rules, or an ordinary one again with
/// `None`, keeping what it holds. Rules take effect at once.
pub fn set_rules(conn: &Connection, id: i64, rules: Option<&Rules>) -> Result<(), DbError> {
    conn.execute(
        "UPDATE playlists SET rules = ?2, refreshed_at = NULL,
                changed_at = datetime('now'), revision = revision + 1
         WHERE id = ?1",
        params![id, rules.map(Rules::to_json)],
    )?;
    if rules.is_some() {
        refresh(conn, id)?;
    }
    Ok(())
}

/// A new smart playlist of `user`'s, evaluated. Returns its id.
pub fn create_smart_playlist(
    conn: &Connection,
    user: i64,
    name: &str,
    comment: Option<&str>,
    rules: &Rules,
) -> Result<i64, DbError> {
    super::atomically(conn, || {
        let id = super::create_playlist(conn, user, name, comment)?;
        set_rules(conn, id, Some(rules))?;
        Ok(id)
    })
}

/// Evaluate a smart playlist as its owner and store the result. Whether its
/// contents changed. An ordinary playlist is left alone.
pub fn refresh(conn: &Connection, id: i64) -> Result<bool, DbError> {
    let Some(tracks) = select_for_owner(conn, id)? else {
        return Ok(false);
    };
    store(conn, id, &tracks)
}

/// [`refresh`] for a read: while a scan or sync holds the write lock it
/// stores nothing, and the read serves the last contents rather than waiting.
///
/// A refused store is remembered for [`REFRESH_SECS`], in memory, so a scan
/// long enough to span many reads does not have each of them evaluate the
/// rules again only to be refused.
fn refresh_for_read(conn: &Connection, id: i64) -> Result<bool, DbError> {
    let key = (conn.path().unwrap_or_default().to_owned(), id);
    let now = now_secs();
    if refused()
        .lock()
        .get(&key)
        .is_some_and(|&at| at > now - REFRESH_SECS)
    {
        return Ok(false);
    }
    let Some(tracks) = select_for_owner(conn, id)? else {
        return Ok(false);
    };
    match crate::db::connection::without_waiting(conn, |c| store(c, id, &tracks)) {
        Err(DbError::Sqlite(rusqlite::Error::SqliteFailure(e, _)))
            if e.code == rusqlite::ErrorCode::DatabaseBusy =>
        {
            refused().lock().insert(key, now);
            Ok(false)
        }
        other => {
            refused().lock().remove(&key);
            other
        }
    }
}

/// When a store was last refused for want of the write lock, by database
/// file and playlist.
fn refused() -> &'static parking_lot::Mutex<std::collections::HashMap<(String, i64), i64>> {
    static REFUSED: std::sync::OnceLock<
        parking_lot::Mutex<std::collections::HashMap<(String, i64), i64>>,
    > = std::sync::OnceLock::new();
    REFUSED.get_or_init(Default::default)
}

/// After `user` (unresolved) played or favourited something: evaluate at
/// once their smart playlists whose rules read any of `fields`, whatever
/// their last evaluation, so devices told to pull see the change. A random
/// order is left to its daily draw rather than reshuffled on every play.
/// Returns the ids whose contents changed.
pub fn refresh_after_activity(
    conn: &Connection,
    user: i64,
    fields: &[Field],
) -> Result<Vec<i64>, DbError> {
    let user = resolve_user(conn, user)?;
    let owned: Vec<(i64, String)> = conn
        .prepare_cached("SELECT id, rules FROM playlists WHERE rules IS NOT NULL AND user_id = ?1")?
        .query_map(params![user], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    let mut changed = Vec::new();
    for (id, json) in owned {
        let Ok(rules) = Rules::parse(&json) else {
            continue;
        };
        if rules.is_random() || !rules.uses(fields) {
            continue;
        }
        if refresh_for_read(conn, id)? {
            changed.push(id);
        }
    }
    Ok(changed)
}

fn select_for_owner(conn: &Connection, id: i64) -> Result<Option<Vec<i64>>, DbError> {
    let Some(rules) = playlist_rules(conn, id)? else {
        return Ok(None);
    };
    let owner: i64 = conn.query_row(
        "SELECT user_id FROM playlists WHERE id = ?1",
        params![id],
        |r| r.get(0),
    )?;
    Ok(Some(evaluate(conn, &rules, owner)?))
}

fn store(conn: &Connection, id: i64, tracks: &[i64]) -> Result<bool, DbError> {
    super::atomically(conn, || {
        conn.execute(
            "UPDATE playlists SET refreshed_at = ?2 WHERE id = ?1",
            params![id, now_secs()],
        )?;
        super::playlists::replace_entries(conn, id, tracks)
    })
}

/// Evaluate the smart playlists `user` (unresolved) can see whose last
/// evaluation is older than [`REFRESH_SECS`] (a day for a random order).
/// Returns the ids whose contents changed.
pub fn refresh_due(conn: &Connection, user: i64) -> Result<Vec<i64>, DbError> {
    let user = resolve_user(conn, user)?;
    let now = now_secs();
    let due: Vec<(i64, String)> = conn
        .prepare_cached(
            "SELECT id, rules FROM playlists
             WHERE rules IS NOT NULL AND (user_id = ?1 OR public = 1)
               AND (refreshed_at IS NULL OR refreshed_at <= ?2)",
        )?
        .query_map(params![user, now - REFRESH_SECS], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?
        .collect::<Result<_, _>>()?;
    let mut changed = Vec::new();
    for (id, json) in due {
        let random = Rules::parse(&json).is_ok_and(|r| r.is_random());
        if random {
            let refreshed: Option<i64> = conn.query_row(
                "SELECT refreshed_at FROM playlists WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )?;
            if refreshed.is_some_and(|at| at > now - RANDOM_REFRESH_SECS) {
                continue;
            }
        }
        if refresh_for_read(conn, id)? {
            changed.push(id);
        }
    }
    Ok(changed)
}

/// Evaluate one playlist if it is smart and due, as [`refresh_due`] does for
/// a list. Whether its contents changed.
pub fn refresh_if_due(conn: &Connection, id: i64) -> Result<bool, DbError> {
    let row: Option<(Option<String>, Option<i64>)> = conn
        .query_row(
            "SELECT rules, refreshed_at FROM playlists WHERE id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    let Some((Some(json), refreshed)) = row else {
        return Ok(false);
    };
    let stands = if Rules::parse(&json).is_ok_and(|r| r.is_random()) {
        RANDOM_REFRESH_SECS
    } else {
        REFRESH_SECS
    };
    if refreshed.is_some_and(|at| at > now_secs() - stands) {
        return Ok(false);
    }
    refresh_for_read(conn, id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::queries::{LOCAL_USER, sample_meta, upsert_track};

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        conn
    }

    fn track(conn: &Connection, title: &str, artist: &str, album: &str) -> i64 {
        upsert_track(conn, &sample_meta(title, artist, album)).unwrap()
    }

    fn rules(json: &str) -> Rules {
        Rules::parse(json).unwrap()
    }

    fn select(conn: &Connection, json: &str) -> Vec<i64> {
        evaluate(conn, &rules(json), LOCAL_USER).unwrap()
    }

    #[test]
    fn values_are_parameters_not_sql() {
        let (sql, params) = compile(
            &rules(
                r#"{"rules":[{"field":"title","op":"is","value":"x'); DROP TABLE tracks; --"}]}"#,
            ),
            1,
            0,
        )
        .unwrap();
        assert!(!sql.contains("DROP"), "{sql}");
        assert_eq!(params.len(), 2);
    }

    #[test]
    fn text_matches_fold_case_and_spacing() {
        let conn = test_conn();
        let a = track(&conn, "Teardrop", "Massive Attack", "Mezzanine");
        let b = track(&conn, "Angel", "Massive Attack", "Mezzanine");
        let c = track(&conn, "Roads", "Portishead", "Dummy");

        assert_eq!(
            select(
                &conn,
                r#"{"rules":[{"field":"artist","op":"is","value":"massive  ATTACK"}]}"#
            ),
            vec![a, b],
            "album order: disc, number, then id; sample tracks share number 1"
        );
        assert_eq!(
            select(
                &conn,
                r#"{"rules":[{"field":"title","op":"contains","value":"EAR"}]}"#
            ),
            vec![a]
        );
        assert_eq!(
            select(
                &conn,
                r#"{"rules":[{"field":"album","op":"startsWith","value":"dum"}]}"#
            ),
            vec![c]
        );
        assert_eq!(
            select(
                &conn,
                r#"{"rules":[{"field":"title","op":"endsWith","value":"drop"}]}"#
            ),
            vec![a]
        );
        let not_massive = select(
            &conn,
            r#"{"rules":[{"field":"artist","op":"isNot","value":"Massive Attack"}]}"#,
        );
        assert_eq!(not_massive, vec![c]);
    }

    #[test]
    fn any_all_and_nesting() {
        let conn = test_conn();
        let a = track(&conn, "A", "One", "X");
        let b = track(&conn, "B", "Two", "Y");
        let _c = track(&conn, "C", "Three", "Z");

        let mut got = select(
            &conn,
            r#"{"match":"any","rules":[
                {"field":"artist","op":"is","value":"One"},
                {"field":"artist","op":"is","value":"Two"}]}"#,
        );
        got.sort();
        assert_eq!(got, vec![a, b]);
        assert_eq!(
            select(
                &conn,
                r#"{"rules":[
                    {"field":"title","op":"isNot","value":"A"},
                    {"match":"any","rules":[
                        {"field":"artist","op":"is","value":"One"},
                        {"field":"artist","op":"is","value":"Two"}]}]}"#,
            ),
            vec![b]
        );
        assert_eq!(
            select(&conn, r#"{"match":"any","rules":[]}"#),
            Vec::<i64>::new()
        );
        assert_eq!(select(&conn, r#"{"rules":[]}"#).len(), 3);
    }

    #[test]
    fn plays_favourites_sort_and_limit() {
        let conn = test_conn();
        let a = track(&conn, "A", "Artist", "X");
        let b = track(&conn, "B", "Artist", "Y");
        let c = track(&conn, "C", "Artist", "Z");
        let now = now_secs();
        let plays = [
            (a, now - 100),
            (a, now - 50),
            (a, now - 10),
            (b, now - 40 * 86_400),
        ];
        super::super::record_plays_at(&conn, LOCAL_USER, &plays, "local").unwrap();
        super::super::add_favourite(&conn, LOCAL_USER, c).unwrap();

        assert_eq!(
            select(
                &conn,
                r#"{"rules":[{"field":"playCount","op":"gt","value":2}]}"#
            ),
            vec![a]
        );
        assert_eq!(
            select(
                &conn,
                r#"{"rules":[{"field":"lastPlayed","op":"inTheLast","value":30}]}"#
            ),
            vec![a]
        );
        let mut stale = select(
            &conn,
            r#"{"rules":[{"field":"lastPlayed","op":"notInTheLast","value":30}]}"#,
        );
        stale.sort();
        assert_eq!(stale, vec![b, c], "never played counts as not recently");
        assert_eq!(
            select(
                &conn,
                r#"{"rules":[{"field":"favourite","op":"is","value":true}]}"#
            ),
            vec![c]
        );
        assert_eq!(
            select(
                &conn,
                r#"{"rules":[{"field":"favourite","op":"isNot","value":true}]}"#
            )
            .len(),
            2
        );
        assert_eq!(
            select(
                &conn,
                r#"{"sort":[{"field":"playCount","desc":true}],"limit":2}"#
            ),
            vec![a, b]
        );
    }

    #[test]
    fn duration_is_written_in_seconds() {
        let conn = test_conn();
        let a = track(&conn, "A", "Artist", "X");
        // sample_meta tracks run 240 s.
        assert_eq!(
            select(
                &conn,
                r#"{"rules":[{"field":"duration","op":"gt","value":200}]}"#
            ),
            vec![a]
        );
        assert!(
            select(
                &conn,
                r#"{"rules":[{"field":"duration","op":"gt","value":300}]}"#
            )
            .is_empty()
        );
    }

    #[test]
    fn plays_are_the_owners() {
        let conn = test_conn();
        let a = track(&conn, "A", "Artist", "X");
        conn.execute(
            "INSERT INTO users (id, username, password_hash, role) VALUES (7, 'other', 'x', 'user')",
            [],
        )
        .unwrap();
        super::super::record_plays_at(&conn, 7, &[(a, now_secs())], "local").unwrap();
        let played = rules(r#"{"rules":[{"field":"playCount","op":"gt","value":0}]}"#);
        assert_eq!(evaluate(&conn, &played, 7).unwrap(), vec![a]);
        assert!(evaluate(&conn, &played, LOCAL_USER).unwrap().is_empty());
    }

    #[test]
    fn a_refresh_writes_only_what_changed_and_keeps_entry_ids() {
        let conn = test_conn();
        let a = track(&conn, "A", "Artist", "X");
        let id = create_smart_playlist(
            &conn,
            LOCAL_USER,
            "Played",
            None,
            &rules(r#"{"rules":[{"field":"playCount","op":"gt","value":0}]}"#),
        )
        .unwrap();
        assert!(
            super::super::playlist_track_ids(&conn, id)
                .unwrap()
                .is_empty()
        );

        super::super::record_plays_at(&conn, LOCAL_USER, &[(a, now_secs())], "local").unwrap();
        assert!(refresh(&conn, id).unwrap());
        let entries = super::super::playlist_entry_ids(&conn, id).unwrap();
        assert_eq!(
            super::super::playlist_track_ids(&conn, id).unwrap(),
            vec![a]
        );

        let revision = super::super::get_playlist(&conn, id)
            .unwrap()
            .unwrap()
            .revision;
        assert!(!refresh(&conn, id).unwrap(), "nothing new");
        assert_eq!(
            super::super::get_playlist(&conn, id)
                .unwrap()
                .unwrap()
                .revision,
            revision
        );

        let b = track(&conn, "B", "Artist", "X");
        super::super::record_plays_at(&conn, LOCAL_USER, &[(b, now_secs())], "local").unwrap();
        assert!(refresh(&conn, id).unwrap());
        let after = super::super::playlist_entry_ids(&conn, id).unwrap();
        assert_eq!(after.len(), 2);
        assert!(
            after.contains(&entries[0]),
            "the surviving entry kept its id"
        );
    }

    #[test]
    fn refresh_due_skips_what_was_just_evaluated() {
        let conn = test_conn();
        track(&conn, "A", "Artist", "X");
        let id = create_smart_playlist(&conn, LOCAL_USER, "All", None, &rules(r#"{"rules":[]}"#))
            .unwrap();
        assert!(
            refresh_due(&conn, LOCAL_USER).unwrap().is_empty(),
            "evaluated on creation"
        );
        track(&conn, "B", "Artist", "X");
        conn.execute("UPDATE playlists SET refreshed_at = 0 WHERE id = ?1", [id])
            .unwrap();
        assert_eq!(refresh_due(&conn, LOCAL_USER).unwrap(), vec![id]);
        assert_eq!(
            super::super::playlist_track_ids(&conn, id).unwrap().len(),
            2
        );
    }

    #[test]
    fn activity_refreshes_only_playlists_that_read_it() {
        let conn = test_conn();
        let a = track(&conn, "A", "Artist", "X");
        let played = create_smart_playlist(
            &conn,
            LOCAL_USER,
            "Played",
            None,
            &rules(r#"{"rules":[{"field":"playCount","op":"gt","value":0}]}"#),
        )
        .unwrap();
        let loved = create_smart_playlist(
            &conn,
            LOCAL_USER,
            "Loved",
            None,
            &rules(r#"{"rules":[{"field":"favourite","op":"is","value":true}]}"#),
        )
        .unwrap();
        super::super::record_plays_at(&conn, LOCAL_USER, &[(a, now_secs())], "local").unwrap();
        let plays = [Field::PlayCount, Field::LastPlayed];
        assert_eq!(
            refresh_after_activity(&conn, LOCAL_USER, &plays).unwrap(),
            vec![played],
            "inside the minute, and only the one reading plays"
        );
        assert!(
            super::super::playlist_track_ids(&conn, loved)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn smart_playlists_read_as_readonly() {
        let conn = test_conn();
        let id = create_smart_playlist(&conn, LOCAL_USER, "All", None, &rules(r#"{"rules":[]}"#))
            .unwrap();
        let row = super::super::get_playlist(&conn, id).unwrap().unwrap();
        assert!(row.readonly);
        assert!(row.rules.is_some());
        set_rules(&conn, id, None).unwrap();
        assert!(
            !super::super::get_playlist(&conn, id)
                .unwrap()
                .unwrap()
                .readonly
        );
    }
}
