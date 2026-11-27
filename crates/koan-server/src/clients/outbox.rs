//! The server-side record of devices and their waiting commands, in the
//! library database so it outlives a restart: `koan_core::db::queries::link`,
//! on the shared pool, with failures logged and survived.

use koan_core::db::queries::link;
use koan_core::remote::acks::Envelope;
use koan_core::remote::link::LinkCommand;

pub use link::{Absent, PushTarget, TvPairing};

/// Dropped undelivered after this long: a device away a month re-syncs
/// on its own when opened. A device unseen this long is forgotten with
/// its push token, so the entry a reinstall leaves behind stops being
/// offered as a device; a live one gives its token again when it links.
pub(super) const KEEP_SECS: i64 = 30 * 24 * 60 * 60;

/// Tests keep to memory: the configured database is whoever ran them.
///
/// The shared pool, because a state report from every linked device lands
/// here: `Database::open` would run the schema and a checkpoint each time.
fn db() -> Option<koan_core::db::pool::Handle<'static>> {
    if cfg!(test) {
        return None;
    }
    koan_core::db::pool::shared().get().ok()
}

/// Run `f` on the shared database, logging a failure as `what`: `None`
/// without a database or after a failure.
fn with<T>(
    what: &str,
    f: impl FnOnce(&rusqlite::Connection) -> Result<T, koan_core::db::connection::DbError>,
) -> Option<T> {
    let db = db()?;
    logged(what, f(&db.conn))
}

fn logged<T>(what: &str, result: Result<T, koan_core::db::connection::DbError>) -> Option<T> {
    result
        .inspect_err(|e| log::warn!("link: could not {what}: {e}"))
        .ok()
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

pub fn load_orders() -> Vec<super::Order> {
    with("read the standing orders", link::orders)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|b| serde_json::from_str(&b).ok())
        .collect()
}

pub fn save_order(order: &super::Order) {
    let Ok(body) = serde_json::to_string(order) else {
        return;
    };
    with("keep an order", |c| {
        link::save_order(c, &order.id, &body, order.created_at)
    });
}

pub fn drop_order(id: &str) {
    with("drop an order", |c| link::drop_order(c, id));
}

/// Record the device as linked now, forget the stale, and take what
/// waits for it. Whether any device was forgotten.
pub fn take_and_remember(
    username: &str,
    device: &str,
    name: &str,
    platform: &str,
    live: &[(String, String)],
) -> (Vec<Envelope>, bool) {
    // One connection for the three steps, each logged on its own: a device
    // that could not be recorded still takes what waits for it.
    let Some(db) = db() else {
        return (seeded(username, device), false);
    };
    let (c, now) = (&db.conn, now());
    logged(
        "record a device",
        link::record_device(c, device, username, name, platform, now),
    );
    let pruned = logged(
        "forget stale devices",
        link::forget_stale(c, live, now, KEEP_SECS),
    )
    .unwrap_or(false);
    let waiting = logged(
        "take waiting commands",
        link::take_waiting(c, device, username),
    )
    .unwrap_or_default();
    if !waiting.is_empty() {
        log::info!("link: {} waiting commands for {name}", waiting.len());
    }
    (waiting, pruned)
}

/// Commands a test has left waiting for a device: tests have no outbox
/// database.
#[cfg(test)]
pub(crate) static SEEDED: parking_lot::Mutex<Vec<(String, String, Envelope)>> =
    parking_lot::Mutex::new(Vec::new());

#[cfg(test)]
fn seeded(username: &str, device: &str) -> Vec<Envelope> {
    let mut seeded = SEEDED.lock();
    let (taken, kept) = std::mem::take(&mut *seeded)
        .into_iter()
        .partition(|(d, u, _)| d == device && u == username);
    *seeded = kept;
    taken.into_iter().map(|(_, _, e)| e).collect::<Vec<_>>()
}

#[cfg(not(test))]
fn seeded(_: &str, _: &str) -> Vec<Envelope> {
    Vec::new()
}

/// Mark `(device, username)` pairs seen now.
pub fn touch(devices: &[(String, String)]) {
    with("mark devices seen", |c| link::touch(c, devices, now()));
}

/// Queue `cmd` for each known device in scope that is not in `live`.
pub fn queue_for_absent(
    username: Option<&str>,
    live: &[(String, String)],
    cmd: &LinkCommand,
) -> Vec<Absent> {
    with("queue a command", |c| {
        link::queue_for_absent(c, username, live, cmd, now())
    })
    .unwrap_or_default()
}

/// Keep a command for `device` until it next links.
pub fn queue_for(device: &str, username: &str, envelope: &Envelope) {
    with("queue a command", |c| {
        link::queue_for(c, device, username, envelope, now())
    });
}

/// Every account on the server, by name: what an owner chooses from to
/// share a device. Any account may see them; the server's users trust
/// each other that far.
pub fn accounts() -> Vec<String> {
    with("list the accounts", |c| {
        Ok(koan_core::db::queries::auth::list_users(c)?
            .into_iter()
            .map(|u| u.username)
            .collect())
    })
    .unwrap_or_default()
}

/// Where each device last linked from, by device id.
pub fn load_addresses() -> std::collections::HashMap<String, (String, std::net::IpAddr)> {
    with("read device addresses", link::addresses).unwrap_or_default()
}

pub fn save_address(device: &str, username: &str, addr: std::net::IpAddr) {
    with("keep a device's address", |c| {
        link::save_address(c, device, username, addr)
    });
}

pub fn load_grants() -> Vec<super::Grant> {
    with("read shared devices", link::grants).unwrap_or_default()
}

pub fn save_grant(g: &super::Grant) {
    with("share a device", |c| link::save_grant(c, g, now()));
}

pub fn drop_grant(g: &super::Grant) {
    with("stop sharing a device", |c| link::drop_grant(c, g));
}

pub fn save_push(username: &str, device: &str, handle: &str) {
    with("keep a push handle", |c| {
        link::save_push(c, username, device, handle, now())
    });
}

/// Whether `device` is an Apple TV that has linked as `username`'s.
pub fn is_tv(username: &str, device: &str) -> bool {
    with("read a device", |c| link::is_tv(c, username, device)).unwrap_or(false)
}

pub fn save_tv_pairing(
    username: &str,
    device: &str,
    pairing: Option<&koan_core::remote::companion::Credential>,
) {
    with("keep a TV's pairing", |c| {
        link::save_tv_pairing(c, username, device, pairing, now())
    });
}

/// `username`'s paired Apple TVs.
pub fn tv_pairings(username: &str) -> Vec<TvPairing> {
    with("read paired TVs", |c| link::tv_pairings(c, username)).unwrap_or_default()
}

pub fn forget_push(username: &str, device: &str) {
    with("forget a push handle", |c| {
        link::forget_push(c, username, device)
    });
}

/// Devices in scope with a push token, most recently seen first.
pub fn push_targets(username: Option<&str>) -> Vec<PushTarget> {
    with("read push targets", |c| link::push_targets(c, username)).unwrap_or_default()
}

/// Forget `username`'s device `device`: its record, its push token and
/// what waits for it. It is recorded afresh if it links again.
pub fn forget_device(device: &str, username: &str) {
    with("forget a device", |c| {
        link::forget_device(c, device, username)
    });
}

/// What a playback command would play, for a notification to say.
pub fn describe(cmd: &LinkCommand) -> Option<String> {
    with("describe a command", |c| link::describe(c, cmd)).flatten()
}
