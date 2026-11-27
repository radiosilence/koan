//! What a koan server keeps about the devices that link to it, so it outlives
//! a restart: each device an account has linked, where it last linked from,
//! the commands waiting for it, its push handle, the Apple TVs paired to be
//! woken, devices shared between accounts, and standing orders to queue an
//! album once it arrives.

use std::collections::HashMap;
use std::net::IpAddr;

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::connection::DbError;
use crate::remote::acks::Envelope;
use crate::remote::companion::Credential;
use crate::remote::link::LinkCommand;

/// Standing orders, as the bodies they were saved with, oldest first.
pub fn orders(conn: &Connection) -> Result<Vec<String>, DbError> {
    let mut stmt = conn.prepare_cached("SELECT body FROM link_orders ORDER BY created_at")?;
    let rows = stmt.query_map([], |r| r.get(0))?;
    rows.collect::<Result<_, _>>().map_err(Into::into)
}

/// Keep a standing order, replacing one saved under the same id.
pub fn save_order(conn: &Connection, id: &str, body: &str, created_at: i64) -> Result<(), DbError> {
    conn.execute(
        "INSERT OR REPLACE INTO link_orders (id, body, created_at) VALUES (?1, ?2, ?3)",
        params![id, body, created_at],
    )?;
    Ok(())
}

pub fn drop_order(conn: &Connection, id: &str) -> Result<(), DbError> {
    conn.execute("DELETE FROM link_orders WHERE id = ?1", [id])?;
    Ok(())
}

/// Record that `username`'s `device` linked now, under this name and platform.
pub fn record_device(
    conn: &Connection,
    device: &str,
    username: &str,
    name: &str,
    platform: &str,
    now: i64,
) -> Result<(), DbError> {
    conn.execute(
        "INSERT INTO link_devices (device, username, name, platform, last_seen)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (device, username) DO UPDATE SET name = ?3, platform = ?4, last_seen = ?5",
        params![device, username, name, platform, now],
    )?;
    Ok(())
}

/// The commands waiting for `username`'s `device`, oldest first, taken: they
/// are no longer kept. One that no longer parses is dropped.
pub fn take_waiting(
    conn: &Connection,
    device: &str,
    username: &str,
) -> Result<Vec<Envelope>, DbError> {
    super::atomically(conn, || {
        let waiting: Vec<String> = conn
            .prepare_cached(
                "SELECT command FROM link_outbox WHERE device = ?1 AND username = ?2 ORDER BY id",
            )?
            .query_map([device, username], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        conn.execute(
            "DELETE FROM link_outbox WHERE device = ?1 AND username = ?2",
            [device, username],
        )?;
        Ok::<_, DbError>(
            waiting
                .iter()
                .filter_map(|c| serde_json::from_str(c).ok())
                .collect(),
        )
    })
}

/// Drop undelivered commands, devices and push handles older than
/// `keep_secs`, after marking the devices in `live` seen: a link held open
/// that long is seen, not stale. A paired Apple TV is kept however long it
/// sleeps, since the pairing was a deliberate act and waking it is what links
/// it again. Whether any device was dropped.
pub fn forget_stale(
    conn: &Connection,
    live: &[(String, String)],
    now: i64,
    keep_secs: i64,
) -> Result<bool, DbError> {
    // Pruning goes ahead without it: a live device then risks being dropped
    // as stale, and links again when it next connects.
    if let Err(e) = touch(conn, live, now) {
        log::warn!("could not mark linked devices seen: {e}");
    }
    let before = now - keep_secs;
    conn.execute("DELETE FROM link_outbox WHERE created_at < ?1", [before])?;
    conn.execute(
        "DELETE FROM link_handles WHERE (device, username) IN
           (SELECT device, username FROM link_devices WHERE last_seen < ?1)
           AND (device, username) NOT IN (SELECT device, username FROM link_tv)",
        [before],
    )?;
    let dropped = conn.execute(
        "DELETE FROM link_devices WHERE last_seen < ?1
           AND (device, username) NOT IN (SELECT device, username FROM link_tv)",
        [before],
    )?;
    Ok(dropped > 0)
}

/// Mark `(device, username)` pairs seen at `now`.
pub fn touch(conn: &Connection, devices: &[(String, String)], now: i64) -> Result<(), DbError> {
    let mut stmt = conn.prepare_cached(
        "UPDATE link_devices SET last_seen = ?1 WHERE device = ?2 AND username = ?3",
    )?;
    for (device, username) in devices {
        stmt.execute(params![now, device, username])?;
    }
    Ok(())
}

/// A device that has linked before, and was not linked when a command was
/// queued for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Absent {
    pub device: String,
    pub username: String,
    pub name: String,
}

impl Absent {
    /// `(device, username)`, as devices are keyed while linked.
    pub fn key(&self) -> (String, String) {
        (self.device.clone(), self.username.clone())
    }
}

/// Queue `cmd` for each device in scope (`username`'s, or everyone's for
/// `None`) that has linked before and is not in `live`, each device in a
/// transaction of its own, so one that fails costs only its own command. A
/// sync replaces one already waiting, since a full one covers an incremental.
/// The devices it was queued for.
pub fn queue_for_absent(
    conn: &Connection,
    username: Option<&str>,
    live: &[(String, String)],
    cmd: &LinkCommand,
    now: i64,
) -> Result<Vec<Absent>, DbError> {
    let known: Vec<Absent> = conn
        .prepare_cached("SELECT device, username, name FROM link_devices")?
        .query_map([], |r| {
            Ok(Absent {
                device: r.get(0)?,
                username: r.get(1)?,
                name: r.get(2)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    let absent: Vec<Absent> = known
        .into_iter()
        .filter(|a| {
            username.is_none_or(|u| u == a.username)
                && !live.iter().any(|(d, u)| *d == a.device && *u == a.username)
        })
        .collect();
    if absent.is_empty() {
        return Ok(Vec::new());
    }
    let text = serde_json::to_string(cmd)?;
    let is_sync = matches!(cmd, LinkCommand::Sync { .. });
    Ok(absent
        .into_iter()
        .filter(|a| {
            let queued = super::atomically(conn, || {
                if is_sync {
                    conn.execute(
                        "DELETE FROM link_outbox WHERE device = ?1 AND username = ?2
                           AND command LIKE '{\"type\":\"sync\"%'",
                        [&a.device, &a.username],
                    )?;
                }
                conn.execute(
                    "INSERT INTO link_outbox (device, username, command, created_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![a.device, a.username, text, now],
                )?;
                Ok::<_, DbError>(())
            });
            if let Err(e) = &queued {
                log::warn!("could not queue a command for {}: {e}", a.device);
            }
            queued.is_ok()
        })
        .collect())
}

/// Keep `envelope` for `username`'s `device` until it next links, under the id
/// it was sent with, if any, so a device that also got it over its link acts
/// on it once.
pub fn queue_for(
    conn: &Connection,
    device: &str,
    username: &str,
    envelope: &Envelope,
    now: i64,
) -> Result<(), DbError> {
    let text = serde_json::to_string(envelope)?;
    conn.execute(
        "INSERT INTO link_outbox (device, username, command, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![device, username, text, now],
    )?;
    Ok(())
}

/// Where each device last linked from, by device id, with the account it
/// linked as: the most recent of its rows, should it have linked as more than
/// one account.
pub fn addresses(conn: &Connection) -> Result<HashMap<String, (String, IpAddr)>, DbError> {
    let rows: Vec<(String, String, String)> = conn
        .prepare_cached(
            "SELECT device, username, addr FROM link_devices
             WHERE addr IS NOT NULL ORDER BY last_seen",
        )?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<Result<_, _>>()?;
    Ok(rows
        .into_iter()
        .filter_map(|(device, username, addr)| Some((device, (username, addr.parse().ok()?))))
        .collect())
}

pub fn save_address(
    conn: &Connection,
    device: &str,
    username: &str,
    addr: IpAddr,
) -> Result<(), DbError> {
    conn.execute(
        "UPDATE link_devices SET addr = ?1 WHERE device = ?2 AND username = ?3",
        params![addr.to_string(), device, username],
    )?;
    Ok(())
}

/// The owner of `device` lets the account `grantee` control it: playback and
/// the queue, as a device on the same network may, and nothing of the
/// owner's account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    pub device: String,
    pub owner: String,
    pub grantee: String,
}

/// Every grant, oldest first.
pub fn grants(conn: &Connection) -> Result<Vec<Grant>, DbError> {
    let mut stmt =
        conn.prepare_cached("SELECT device, owner, grantee FROM link_grants ORDER BY created_at")?;
    let rows = stmt.query_map([], |r| {
        Ok(Grant {
            device: r.get(0)?,
            owner: r.get(1)?,
            grantee: r.get(2)?,
        })
    })?;
    rows.collect::<Result<_, _>>().map_err(Into::into)
}

pub fn save_grant(conn: &Connection, grant: &Grant, now: i64) -> Result<(), DbError> {
    conn.execute(
        "INSERT OR IGNORE INTO link_grants (device, owner, grantee, created_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![grant.device, grant.owner, grant.grantee, now],
    )?;
    Ok(())
}

pub fn drop_grant(conn: &Connection, grant: &Grant) -> Result<(), DbError> {
    conn.execute(
        "DELETE FROM link_grants WHERE device = ?1 AND owner = ?2 AND grantee = ?3",
        [&grant.device, &grant.owner, &grant.grantee],
    )?;
    Ok(())
}

/// Keep the push relay's handle for `username`'s `device`.
pub fn save_push(
    conn: &Connection,
    username: &str,
    device: &str,
    handle: &str,
    now: i64,
) -> Result<(), DbError> {
    conn.execute(
        "INSERT INTO link_handles (device, username, handle, updated_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (device, username) DO UPDATE SET handle = ?3, updated_at = ?4",
        params![device, username, handle, now],
    )?;
    Ok(())
}

pub fn forget_push(conn: &Connection, username: &str, device: &str) -> Result<(), DbError> {
    conn.execute(
        "DELETE FROM link_handles WHERE device = ?1 AND username = ?2",
        [device, username],
    )?;
    Ok(())
}

/// A device the push relay can reach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushTarget {
    pub device: String,
    pub username: String,
    pub name: String,
    pub platform: String,
    /// The push relay's handle for it.
    pub handle: String,
    /// Unix seconds.
    pub last_seen: i64,
}

/// Devices in scope (`username`'s, or everyone's for `None`) with a push
/// handle, most recently seen first.
pub fn push_targets(conn: &Connection, username: Option<&str>) -> Result<Vec<PushTarget>, DbError> {
    let mut stmt = conn.prepare_cached(
        "SELECT p.device, p.username, d.name, d.platform, p.handle, d.last_seen
           FROM link_handles p
           JOIN link_devices d ON d.device = p.device AND d.username = p.username
          WHERE ?1 IS NULL OR p.username = ?1
          ORDER BY d.last_seen DESC",
    )?;
    let rows = stmt.query_map([username], |r| {
        Ok(PushTarget {
            device: r.get(0)?,
            username: r.get(1)?,
            name: r.get(2)?,
            platform: r.get(3)?,
            handle: r.get(4)?,
            last_seen: r.get(5)?,
        })
    })?;
    rows.collect::<Result<_, _>>().map_err(Into::into)
}

/// Whether `device` is an Apple TV that has linked as `username`'s.
pub fn is_tv(conn: &Connection, username: &str, device: &str) -> Result<bool, DbError> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM link_devices WHERE device = ?1 AND username = ?2 AND platform = 'tvos'",
            [device, username],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Keep the pairing that wakes `username`'s Apple TV `device`, or forget it
/// for `None`.
pub fn save_tv_pairing(
    conn: &Connection,
    username: &str,
    device: &str,
    pairing: Option<&Credential>,
    now: i64,
) -> Result<(), DbError> {
    match pairing {
        Some(pairing) => {
            let json = serde_json::to_string(pairing)?;
            conn.execute(
                "INSERT INTO link_tv (device, username, pairing, updated_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (device, username) DO UPDATE SET pairing = ?3, updated_at = ?4",
                params![device, username, json, now],
            )?
        }
        None => conn.execute(
            "DELETE FROM link_tv WHERE device = ?1 AND username = ?2",
            [device, username],
        )?,
    };
    Ok(())
}

/// An Apple TV of an account's, paired to be woken.
#[derive(Debug, Clone)]
pub struct TvPairing {
    pub device: String,
    pub name: String,
    pub platform: String,
    /// Unix seconds.
    pub last_seen: i64,
    pub pairing: Credential,
}

/// `username`'s paired Apple TVs. One whose pairing no longer parses is left
/// out.
pub fn tv_pairings(conn: &Connection, username: &str) -> Result<Vec<TvPairing>, DbError> {
    let rows: Vec<(String, String, String, i64, String)> = conn
        .prepare_cached(
            "SELECT t.device, d.name, d.platform, d.last_seen, t.pairing
               FROM link_tv t
               JOIN link_devices d ON d.device = t.device AND d.username = t.username
              WHERE t.username = ?1",
        )?
        .query_map([username], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?
        .collect::<Result<_, _>>()?;
    Ok(rows
        .into_iter()
        .filter_map(|(device, name, platform, last_seen, pairing)| {
            Some(TvPairing {
                device,
                name,
                platform,
                last_seen,
                pairing: serde_json::from_str(&pairing).ok()?,
            })
        })
        .collect())
}

/// Forget `username`'s `device`: its record, its push handle, its pairing and
/// what waits for it. It is recorded afresh if it links again.
pub fn forget_device(conn: &Connection, device: &str, username: &str) -> Result<(), DbError> {
    super::atomically(conn, || {
        for table in ["link_handles", "link_tv", "link_outbox", "link_devices"] {
            conn.execute(
                &format!("DELETE FROM {table} WHERE device = ?1 AND username = ?2"),
                [device, username],
            )?;
        }
        Ok::<_, DbError>(())
    })
}

/// What a playback command would play, for a notification to say: "Golden
/// Standard — Tony Petersen", or a track and how many follow. `None` for a
/// command that plays nothing, or names no track the library holds. Two
/// queries however many tracks.
pub fn describe(conn: &Connection, cmd: &LinkCommand) -> Result<Option<String>, DbError> {
    let raw: Vec<&str> = match cmd {
        LinkCommand::Play { track_ids, .. }
        | LinkCommand::Enqueue { track_ids, .. }
        | LinkCommand::PlayNext { track_ids } => track_ids.iter().map(String::as_str).collect(),
        LinkCommand::JumpTo { track_id } => vec![track_id],
        _ => return Ok(None),
    };
    // Bare row ids, which clients from before uids send, as they are.
    let uids = super::ids_for_uids(
        conn,
        super::UidKind::Track,
        raw.iter().copied().filter(|t| super::is_uid(t)),
    )?;
    let ids: Vec<i64> = raw
        .iter()
        .filter_map(|t| t.parse().ok().or_else(|| uids.get(*t).copied()))
        .collect();
    let Some(&first) = ids.first() else {
        return Ok(None);
    };
    let rows: HashMap<i64, (String, String, String, Option<i64>)> = conn
        .prepare_cached(
            "SELECT t.id, t.title, COALESCE(a.name, ''), COALESCE(al.title, ''), t.album_id
               FROM tracks t LEFT JOIN artists a ON a.id = t.artist_id
               LEFT JOIN albums al ON al.id = t.album_id
              WHERE t.id IN (SELECT value FROM json_each(?1))",
        )?
        .query_map([super::json_list(&ids)], |r| {
            Ok((r.get(0)?, (r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
        })?
        .collect::<Result<_, _>>()?;
    let Some((title, artist, album, album_id)) = rows.get(&first) else {
        return Ok(None);
    };
    let one_album = ids.len() > 1
        && album_id.is_some()
        && ids
            .iter()
            .all(|id| rows.get(id).is_some_and(|r| r.3 == *album_id));
    Ok(Some(match (one_album, ids.len()) {
        (true, _) => format!("{album} — {artist}"),
        (false, 1) => format!("{title} — {artist}"),
        (false, n) => format!("{title} — {artist}, and {} more", n - 1),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::queries::{sample_meta, uids_for, upsert_track};

    const DAY: i64 = 24 * 60 * 60;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        conn
    }

    fn devices(conn: &Connection, table: &str) -> Vec<String> {
        conn.prepare(&format!("SELECT device FROM {table} ORDER BY device"))
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    #[test]
    fn orders_are_kept_oldest_first_and_dropped_by_id() {
        let conn = db();
        save_order(&conn, "b", "{\"n\":2}", 20).unwrap();
        save_order(&conn, "a", "{\"n\":1}", 10).unwrap();
        save_order(&conn, "a", "{\"n\":3}", 10).unwrap();
        assert_eq!(orders(&conn).unwrap(), ["{\"n\":3}", "{\"n\":2}"]);
        drop_order(&conn, "a").unwrap();
        assert_eq!(orders(&conn).unwrap(), ["{\"n\":2}"]);
    }

    /// A command waits for each device that is away, a sync only once, and
    /// is taken when the device links.
    #[test]
    fn commands_wait_for_absent_devices_until_taken() {
        let conn = db();
        for (device, user) in [("phone", "j"), ("mac", "j"), ("laptop", "k")] {
            record_device(&conn, device, user, device, "ios", 1).unwrap();
        }
        let live = [("mac".to_string(), "j".to_string())];
        let sync = LinkCommand::Sync { full: false };
        let queued = queue_for_absent(&conn, Some("j"), &live, &sync, 2).unwrap();
        assert_eq!(
            queued.iter().map(|a| a.device.as_str()).collect::<Vec<_>>(),
            ["phone"],
            "not the linked mac, not another account's laptop"
        );
        queue_for_absent(&conn, Some("j"), &live, &sync, 3).unwrap();
        queue_for_absent(&conn, Some("j"), &live, &LinkCommand::Pause, 4).unwrap();
        let waiting = take_waiting(&conn, "phone", "j").unwrap();
        assert_eq!(
            waiting.iter().map(|e| &e.command).collect::<Vec<_>>(),
            [&sync, &LinkCommand::Pause],
            "one sync, then the pause"
        );
        assert!(take_waiting(&conn, "phone", "j").unwrap().is_empty());
        let everyone = queue_for_absent(&conn, None, &live, &LinkCommand::Pause, 5).unwrap();
        assert_eq!(everyone.len(), 2);
    }

    #[test]
    fn an_envelope_keeps_its_ack() {
        let conn = db();
        let envelope = Envelope {
            command: LinkCommand::Pause,
            ack: Some(7),
        };
        queue_for(&conn, "phone", "j", &envelope, 1).unwrap();
        assert_eq!(take_waiting(&conn, "phone", "j").unwrap(), [envelope]);
    }

    /// The server restarts with every release; where a device last linked
    /// from is read back, with the account it linked as.
    #[test]
    fn addresses_are_kept_per_device() {
        let conn = db();
        let home: IpAddr = "203.0.113.7".parse().unwrap();
        record_device(&conn, "ipad", "sarita", "iPad", "ios", 1).unwrap();
        record_device(&conn, "phone", "admin", "Phone", "ios", 2).unwrap();
        save_address(&conn, "ipad", "sarita", home).unwrap();
        let addresses = addresses(&conn).unwrap();
        assert_eq!(addresses["ipad"], ("sarita".to_string(), home));
        assert!(!addresses.contains_key("phone"), "no address, no entry");
    }

    #[test]
    fn grants_are_kept_once_and_dropped() {
        let conn = db();
        let grant = Grant {
            device: "phone".into(),
            owner: "jo".into(),
            grantee: "kim".into(),
        };
        save_grant(&conn, &grant, 1).unwrap();
        save_grant(&conn, &grant, 2).unwrap();
        assert_eq!(grants(&conn).unwrap(), std::slice::from_ref(&grant));
        drop_grant(&conn, &grant).unwrap();
        assert!(grants(&conn).unwrap().is_empty());
    }

    /// The server forgets a device's push handle with it, so nothing is
    /// pushed to it again; and only for the account it is forgotten as.
    #[test]
    fn forgetting_a_device_stops_pushes_to_it_and_only_for_its_account() {
        let conn = db();
        for user in ["j", "k"] {
            record_device(&conn, "phone", user, "phone", "ios", 1).unwrap();
            save_push(&conn, user, "phone", "h", 1).unwrap();
        }
        forget_device(&conn, "phone", "k").unwrap();
        assert_eq!(
            push_targets(&conn, Some("j")).unwrap().len(),
            1,
            "k forgetting its own leaves j's alone"
        );
        forget_device(&conn, "phone", "j").unwrap();
        assert!(push_targets(&conn, None).unwrap().is_empty());
    }

    #[test]
    fn push_targets_are_the_most_recently_seen_first() {
        let conn = db();
        record_device(&conn, "old", "j", "Old", "ios", 1).unwrap();
        record_device(&conn, "new", "j", "New", "ios", 2).unwrap();
        for device in ["old", "new"] {
            save_push(&conn, "j", device, "h", 1).unwrap();
        }
        let targets = push_targets(&conn, Some("j")).unwrap();
        assert_eq!(
            targets
                .iter()
                .map(|t| t.device.as_str())
                .collect::<Vec<_>>(),
            ["new", "old"]
        );
        forget_push(&conn, "j", "new").unwrap();
        assert_eq!(push_targets(&conn, Some("j")).unwrap().len(), 1);
    }

    /// A device whose command cannot be written costs only its own: the
    /// others are queued and returned, to be woken.
    #[test]
    fn a_failed_write_for_one_device_queues_the_rest() {
        let conn = db();
        for device in ["phone", "ipad", "mac"] {
            record_device(&conn, device, "j", device, "ios", 1).unwrap();
        }
        conn.execute_batch(
            "CREATE TRIGGER refuse BEFORE INSERT ON link_outbox WHEN NEW.device = 'ipad'
             BEGIN SELECT RAISE(ABORT, 'refused'); END;",
        )
        .unwrap();
        let queued = queue_for_absent(&conn, None, &[], &LinkCommand::Pause, 2).unwrap();
        let mut queued: Vec<_> = queued.iter().map(|a| a.device.as_str()).collect();
        queued.sort();
        assert_eq!(queued, ["mac", "phone"]);
        assert_eq!(devices(&conn, "link_outbox"), ["mac", "phone"]);
    }

    /// Stale devices are pruned even when the live ones cannot be marked seen.
    #[test]
    fn stale_devices_are_pruned_when_marking_the_live_ones_fails() {
        let conn = db();
        let now = 100 * DAY;
        record_device(&conn, "mac", "j", "mac", "ios", now).unwrap();
        record_device(&conn, "old-phone", "j", "old-phone", "ios", now - 40 * DAY).unwrap();
        conn.execute_batch(
            "CREATE TRIGGER refuse BEFORE UPDATE OF last_seen ON link_devices
             BEGIN SELECT RAISE(ABORT, 'refused'); END;",
        )
        .unwrap();
        assert!(forget_stale(&conn, &[("mac".into(), "j".into())], now, 30 * DAY).unwrap());
        assert_eq!(devices(&conn, "link_devices"), ["mac"]);
    }

    #[test]
    fn a_device_linked_for_a_month_is_not_forgotten() {
        let conn = db();
        let now = 100 * DAY;
        for device in ["mac", "old-phone"] {
            record_device(&conn, device, "j", device, "ios", now - 40 * DAY).unwrap();
            save_push(&conn, "j", device, "h", now - 40 * DAY).unwrap();
        }
        assert!(forget_stale(&conn, &[("mac".into(), "j".into())], now, 30 * DAY).unwrap());
        assert_eq!(devices(&conn, "link_devices"), ["mac"]);
        assert_eq!(devices(&conn, "link_handles"), ["mac"]);
    }

    #[test]
    fn a_paired_apple_tv_is_kept_however_long_it_sleeps() {
        let conn = db();
        let now = 100 * DAY;
        for device in ["tv", "other-tv"] {
            record_device(&conn, device, "j", "Living Room", "tvos", now - 90 * DAY).unwrap();
        }
        assert!(is_tv(&conn, "j", "tv").unwrap());
        assert!(!is_tv(&conn, "k", "tv").unwrap());
        let pairing = Credential {
            tv_id: "7A821767-01ED-442C-B4BB-6BDEA29109DA".into(),
            tv_key: "a2V5".into(),
            id: "ME".into(),
            seed: "c2VlZA==".into(),
        };
        save_tv_pairing(&conn, "j", "tv", Some(&pairing), 0).unwrap();
        forget_stale(&conn, &[], now, 30 * DAY).unwrap();
        let paired = tv_pairings(&conn, "j").unwrap();
        assert_eq!(paired.len(), 1);
        assert_eq!(paired[0].device, "tv");
        assert_eq!(paired[0].name, "Living Room");
        assert_eq!(paired[0].pairing, pairing);
        assert!(tv_pairings(&conn, "someone").unwrap().is_empty());
        assert_eq!(
            devices(&conn, "link_devices"),
            ["tv"],
            "the unpaired TV is forgotten as before"
        );
        save_tv_pairing(&conn, "j", "tv", None, 0).unwrap();
        assert!(tv_pairings(&conn, "j").unwrap().is_empty());
    }

    #[test]
    fn a_command_is_described_by_what_it_plays() {
        let conn = db();
        let mut ids = Vec::new();
        for (n, title) in ["Golden Standard", "Second"].into_iter().enumerate() {
            let mut meta = sample_meta(title, "Tony Petersen", "Record");
            meta.track_number = Some(n as i32 + 1);
            ids.push(upsert_track(&conn, &meta).unwrap());
        }
        let other = upsert_track(&conn, &sample_meta("Elsewhere", "Someone", "Other")).unwrap();
        let uids = uids_for(&conn, crate::db::queries::UidKind::Track, ids.clone()).unwrap();
        let play = |track_ids: Vec<String>| LinkCommand::PlayNext { track_ids };
        assert_eq!(
            describe(&conn, &play(vec![uids[&ids[0]].clone()])).unwrap(),
            Some("Golden Standard — Tony Petersen".into())
        );
        assert_eq!(
            describe(&conn, &play(ids.iter().map(i64::to_string).collect())).unwrap(),
            Some("Record — Tony Petersen".into()),
            "a whole record by bare row ids"
        );
        assert_eq!(
            describe(&conn, &play(vec![ids[0].to_string(), other.to_string()])).unwrap(),
            Some("Golden Standard — Tony Petersen, and 1 more".into())
        );
        assert_eq!(describe(&conn, &LinkCommand::Pause).unwrap(), None);
    }
}
