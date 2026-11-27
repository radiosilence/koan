//! koan clients linked to this server, and the way to reach them.
//!
//! A client that syncs from this server holds a WebSocket open at
//! `/rest/koanLink` (see `koan_core::remote::link`). Each is registered here
//! under the account it signed in as, so GraphQL (and MCP through it) can hand
//! one a list of tracks to play: build a playlist on the server, hear it on a
//! phone.

use std::sync::LazyLock;

use koan_core::db::queries::{self, UidKind};
use koan_core::remote::acks::{AckOutcome, Envelope};
use koan_core::remote::link::{LinkCommand, LinkDevice, LinkState};
use outbox::Absent;
use parking_lot::Mutex;

mod activity;
mod notify;
mod orders;
mod outbox;
mod registry;
mod relay;
#[cfg(test)]
mod testing;

use activity::*;
use relay::*;

pub use notify::{changed, changed_if_library_moved, smart_activity};
pub use orders::{Order, fulfil_from};
pub use relay::claims;

/// A linked client, as listed.
#[derive(Debug, Clone)]
pub struct ClientInfo {
    pub id: String,
    /// The device's own id, stable across its reconnects.
    pub device: String,
    pub name: String,
    pub platform: String,
    pub username: String,
    /// Unix seconds.
    pub connected_at: i64,
    /// What the client last said it was doing.
    pub state: LinkState,
    /// Unix seconds; when it was last seen playing, if ever since linking.
    pub last_played_at: Option<i64>,
    /// When `state` was reported, in Unix milliseconds.
    pub state_at: i64,
    /// Whether the client has reported its state at all. An app older than
    /// the reports never does, and its `state` then says nothing about it.
    pub reports: bool,
    /// Reached by a notification it shows rather than over its link: iOS had
    /// suspended it. What was sent runs when someone taps the notification.
    pub notified: bool,
    /// It answers commands sent with an id: see `koan_core::remote::acks`.
    pub acks: bool,
}

impl ClientInfo {
    /// Where the playhead is now, from where it was reported to be.
    pub fn position_ms(&self) -> u64 {
        let pos = self.state.position_ms;
        if !self.state.playing {
            return pos;
        }
        let run = (chrono::Utc::now().timestamp_millis() - self.state_at).max(0) as u64;
        let pos = pos + run;
        if self.state.duration_ms > 0 {
            pos.min(self.state.duration_ms)
        } else {
            pos
        }
    }
}

struct Entry {
    info: ClientInfo,
    /// The client's own id for itself, so a reconnect replaces its entry.
    device: String,
    tx: LinkTx,
    /// Sent the account's other devices whenever one changes. Asked for by
    /// the client; one that predates them would log each as a bad command.
    wants_devices: bool,
    /// Sent `LinkCommand::DeviceKeys`: it registered a key of its own, so it
    /// knows the command.
    wants_keys: bool,
}

/// How many envelopes a link holds for its socket. Far more than a device
/// that is reading ever has waiting; one that stops reading fills it.
const LINK_QUEUE: usize = 1024;

/// The way to one link's socket, bounded so a device that stops reading costs
/// a fixed amount. Commands are never dropped silently: one that does not fit
/// ends the link, as if the device had gone, and the device syncs again when
/// it relinks. Levels are dropped instead.
#[derive(Clone)]
pub struct LinkTx {
    tx: tokio::sync::mpsc::Sender<Envelope>,
    behind: std::sync::Arc<tokio::sync::Notify>,
}

impl LinkTx {
    pub fn channel() -> (Self, tokio::sync::mpsc::Receiver<Envelope>) {
        let (tx, rx) = tokio::sync::mpsc::channel(LINK_QUEUE);
        (
            Self {
                tx,
                behind: Default::default(),
            },
            rx,
        )
    }

    /// Notified when a command did not fit: the session ends the link.
    pub fn behind(&self) -> std::sync::Arc<tokio::sync::Notify> {
        self.behind.clone()
    }

    fn send(&self, envelope: impl Into<Envelope>) -> Result<(), ()> {
        match self.tx.try_send(envelope.into()) {
            Ok(()) => Ok(()),
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                self.behind.notify_one();
                Err(())
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => Err(()),
        }
    }

    fn send_lossy(&self, envelope: impl Into<Envelope>) {
        let _ = self.tx.try_send(envelope.into());
    }
}

/// A command relayed with an id, and what to do with the answer: `None` when
/// the device it went to gives none.
pub struct Acking {
    pub id: u64,
    pub reply: Box<dyn FnOnce(Option<AckOutcome>) + Send>,
}

/// An answer the server is waiting for.
struct Waiting {
    reply: Box<dyn FnOnce(Option<AckOutcome>) + Send>,
    /// The device said the command came off its link.
    received: bool,
    /// The timer that looks at it, stopped once the answer is in, so an
    /// answered command costs nothing more.
    timer: Option<tokio::task::AbortHandle>,
}

impl Waiting {
    fn answer(mut self, outcome: Option<AckOutcome>) {
        let reply = std::mem::replace(&mut self.reply, Box::new(|_| {}));
        reply(outcome);
    }
}

impl Drop for Waiting {
    fn drop(&mut self) {
        if let Some(timer) = self.timer.take() {
            timer.abort();
        }
    }
}

/// How long a linked device has to say a command came off its link before it
/// is reached as one that is away; and how long an answer to one it did take
/// is waited for, however long the command runs, before the asker is told
/// only that it arrived.
const FIRST_LOOK: std::time::Duration = std::time::Duration::from_millis(2500);
const LONGEST_ANSWER: std::time::Duration = std::time::Duration::from_secs(120);

pub use koan_core::db::queries::link::Grant;

#[derive(Default)]
pub struct Registry {
    entries: Mutex<Vec<Entry>>,
    orders: Mutex<Vec<Order>>,
    activities: Mutex<Vec<Activity>>,
    grants: Mutex<Vec<Grant>>,
    /// Which devices have another's playing bars on screen. Held in memory
    /// only, and forgotten with the watcher's link.
    level_watches: Mutex<Vec<LevelWatch>>,
    /// Where each device last linked from, by device id: its account and
    /// address. Kept in `link_devices` too, so a restart keeps it.
    addresses: Mutex<std::collections::HashMap<String, (String, std::net::IpAddr)>>,
    /// Answers awaited, by the account and device asked and the command's
    /// id. A device id is the client's own choice, so another account's
    /// device can claim one of this account's: only a link of the account the
    /// command went to can answer it.
    answers: Mutex<std::collections::HashMap<(String, String, u64), Waiting>>,
}

/// `watcher`, a device of `watcher_user`, has `target`'s playing bars on
/// screen; `target` is `owner`'s. The two accounts differ for a device shared
/// with the watcher's.
struct LevelWatch {
    owner: String,
    watcher_user: String,
    watcher: String,
    target: String,
}

/// One registry per process: the WebSocket route and the GraphQL schema are
/// built in different places and both need it.
pub fn registry() -> &'static Registry {
    static REGISTRY: LazyLock<Registry> = LazyLock::new(|| {
        let registry = Registry::default();
        *registry.orders.lock() = outbox::load_orders();
        *registry.grants.lock() = outbox::load_grants();
        *registry.addresses.lock() = outbox::load_addresses();
        registry
    });
    &REGISTRY
}

/// Leave `envelope` waiting for `username`'s `device`, as a command queued
/// while it was away is.
#[cfg(test)]
pub(crate) fn seed_outbox(username: &str, device: &str, envelope: Envelope) {
    outbox::SEEDED
        .lock()
        .push((device.to_string(), username.to_string(), envelope));
}
