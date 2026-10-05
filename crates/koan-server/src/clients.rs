//! koan clients linked to this server, and the way to reach them.
//!
//! A client that syncs from this server holds a WebSocket open at
//! `/rest/koanLink` (see `koan_core::remote::link`). Each is registered here
//! under the account it signed in as, so GraphQL (and MCP through it) can hand
//! one a list of tracks to play: build a playlist on the server, hear it on a
//! phone.

use std::sync::LazyLock;

use koan_core::db::queries::{self, UidKind};
use koan_core::remote::link::{LinkCommand, LinkDevice, LinkState};
use outbox::Absent;
use parking_lot::Mutex;
use tokio::sync::mpsc::UnboundedSender;

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
    tx: UnboundedSender<LinkCommand>,
    /// Sent the account's other devices whenever one changes. Asked for by
    /// the client; one that predates them would log each as a bad command.
    wants_devices: bool,
}

/// A Live Activity on a phone showing another device, and where to push its
/// updates.
struct Activity {
    username: String,
    /// The phone showing it.
    watcher: String,
    /// The device it shows.
    target: String,
    token: String,
    sandbox: bool,
    /// What it was last sent, so only a change goes out: Apple budgets these.
    sent: Option<crate::push::ActivityState>,
}

/// "When this album is in the library, queue it on my device": a request made
/// before the album exists, fulfilled by the scan that finds it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Order {
    pub id: String,
    /// Whose devices it may go to.
    pub username: Option<String>,
    /// A client id or name; `None` for whichever `send` would pick then.
    pub client: Option<String>,
    pub artist: String,
    pub album: String,
    /// Insert after the current track rather than at the end.
    pub play_next: bool,
    /// Add to this playlist rather than a device's queue.
    #[serde(default)]
    pub playlist: Option<i64>,
    /// Only these tracks of the album (title substrings, in this order);
    /// empty for all of it.
    #[serde(default)]
    pub titles: Vec<String>,
    /// Unix seconds.
    pub created_at: i64,
}

/// An order nobody's scan has fulfilled in this long is dropped.
const ORDER_TTL: i64 = 24 * 60 * 60;

/// The owner of `device` lets the account `grantee` control it: playback and
/// the queue, as a device on the same network may, and nothing of the
/// owner's account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    pub device: String,
    pub owner: String,
    pub grantee: String,
}

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

impl Registry {
    /// Add a client, replacing any earlier connection from the same device
    /// and account. Returns its id.
    pub fn register(
        &self,
        username: &str,
        name: &str,
        platform: &str,
        device: &str,
        tx: UnboundedSender<LinkCommand>,
        wants_devices: bool,
    ) -> String {
        let id = uuid::Uuid::now_v7().to_string();
        if let Some((sent, how)) = WOKEN
            .lock()
            .remove(&(device.to_string(), username.to_string()))
        {
            log::info!(
                "wake: {name} linked {}ms after its {how}",
                sent.elapsed().as_millis()
            );
        }
        // What waited for this device while it was away goes down the new
        // link first.
        let live = self.live();
        for cmd in outbox::take_and_remember(username, device, name, platform, &live) {
            let _ = tx.send(cmd);
        }
        let mut entries = self.entries.lock();
        entries.retain(|e| !(e.device == device && e.info.username == username));
        entries.push(Entry {
            info: ClientInfo {
                id: id.clone(),
                device: device.to_string(),
                name: name.to_string(),
                platform: platform.to_string(),
                username: username.to_string(),
                connected_at: chrono::Utc::now().timestamp(),
                state: LinkState::default(),
                last_played_at: None,
                state_at: chrono::Utc::now().timestamp_millis(),
                reports: false,
                notified: false,
            },
            device: device.to_string(),
            tx,
            wants_devices,
        });
        drop(entries);
        // Always, empty or not: an app signed in elsewhere before holds that
        // server's list.
        self.send_shares(username, device, None);
        self.announce(username);
        // A device that relinked has a new session, which holds no watch:
        // tell it again that it is watched, if it is.
        if self
            .level_watches
            .lock()
            .iter()
            .any(|w| w.owner == username && w.target == device)
        {
            self.send_live(username, device, LinkCommand::WatchLevels { on: true });
        }
        id
    }

    /// Record what a client says it is doing.
    pub fn report(&self, id: &str, state: LinkState) {
        let mut entries = self.entries.lock();
        if let Some(e) = entries.iter_mut().find(|e| e.info.id == id) {
            if state.playing || e.info.state.playing {
                e.info.last_played_at = Some(chrono::Utc::now().timestamp());
            }
            e.info.state = state;
            e.info.state_at = chrono::Utc::now().timestamp_millis();
            e.info.reports = true;
            let (username, device) = (e.info.username.clone(), e.device.clone());
            drop(entries);
            self.announce(&username);
            self.update_activities(&username, &device);
        }
    }

    /// Record where Apple's push service reaches this device.
    pub fn set_push(&self, username: &str, device: &str, token: &str, sandbox: bool) {
        outbox::save_push(username, device, token, sandbox);
    }

    /// Close every link `username` has open. Dropping an entry's sender ends
    /// its session, and the client must then sign in again to reconnect.
    pub fn disconnect(&self, username: &str) {
        self.entries.lock().retain(|e| e.info.username != username);
    }

    pub fn unregister(&self, id: &str) {
        let mut entries = self.entries.lock();
        let gone = entries
            .iter()
            .find(|e| e.info.id == id)
            .map(|e| (e.device.clone(), e.info.username.clone()));
        entries.retain(|e| e.info.id != id);
        drop(entries);
        if let Some((device, username)) = gone {
            outbox::touch(&[(device.clone(), username.clone())]);
            self.announce(&username);
            // A controller that went stops watching whatever it watched.
            let targets: Vec<String> = self
                .level_watches
                .lock()
                .iter()
                .filter(|w| w.watcher_user == username && w.watcher == device)
                .map(|w| w.target.clone())
                .collect();
            for target in targets {
                self.watch_levels(&username, &device, &target, false);
            }
        }
    }

    /// `watcher` has `target`'s playing bars on screen, or no longer has. The
    /// target is told to send its levels while anyone watches, and to stop
    /// when the last one goes. Only over a live link: levels are no reason to
    /// wake a phone or to queue a command for later.
    pub fn watch_levels(&self, username: &str, watcher: &str, target: &str, on: bool) {
        // A device shared with `username` is watched as its owner's.
        let owner = self
            .shared_owner(username, target)
            .unwrap_or_else(|| username.to_string());
        let mut watches = self.level_watches.lock();
        watches.retain(|w| {
            !(w.watcher_user == username && w.watcher == watcher && w.target == target)
        });
        if on {
            watches.push(LevelWatch {
                owner: owner.clone(),
                watcher_user: username.into(),
                watcher: watcher.into(),
                target: target.into(),
            });
        }
        let watched = watches
            .iter()
            .any(|w| w.owner == owner && w.target == target);
        drop(watches);
        if on || !watched {
            self.send_live(&owner, target, LinkCommand::WatchLevels { on: watched });
        }
    }

    /// A frame of `from`'s levels, for each device watching it. Not stored:
    /// one that cannot be delivered now is of no use later.
    pub fn levels(&self, username: &str, from: &str, f: koan_core::remote::levels::Frame) {
        let watchers: Vec<(String, String)> = self
            .level_watches
            .lock()
            .iter()
            .filter(|w| w.owner == username && w.target == from)
            .map(|w| (w.watcher_user.clone(), w.watcher.clone()))
            .collect();
        for (watcher_user, watcher) in watchers {
            self.send_live(
                &watcher_user,
                &watcher,
                LinkCommand::Levels {
                    from: from.to_string(),
                    f,
                },
            );
        }
    }

    /// Send `cmd` to `device` if it is linked now, and nowhere else.
    fn send_live(&self, username: &str, device: &str, cmd: LinkCommand) {
        if let Some(e) = self
            .entries
            .lock()
            .iter()
            .find(|e| e.info.username == username && e.device == device)
        {
            let _ = e.tx.send(cmd);
        }
    }

    /// Send each of `username`'s links that asked for them the account's
    /// other devices: those linked, with what each is doing, and those a push
    /// can wake.
    fn announce(&self, username: &str) {
        self.announce_to(username);
        // Those `username` shares a device with see it change too.
        let grantees: Vec<String> = {
            let mut g: Vec<String> = self
                .grants
                .lock()
                .iter()
                .filter(|g| g.owner == username)
                .map(|g| g.grantee.clone())
                .collect();
            g.sort();
            g.dedup();
            g
        };
        for grantee in grantees {
            self.announce_to(&grantee);
        }
    }

    /// `announce` for `username` alone.
    fn announce_to(&self, username: &str) {
        let listening = |e: &Entry| e.info.username == username && e.wants_devices;
        if !self.entries.lock().iter().any(listening) {
            return;
        }
        let asleep = outbox::push_targets(Some(username));
        // Waking takes a push key as well as the device's token.
        let can_push = crate::push::pusher().is_some();
        let entries = self.entries.lock();
        let ours: Vec<&Entry> = entries
            .iter()
            .filter(|e| e.info.username == username)
            .collect();
        if !ours.iter().any(|e| e.wants_devices) {
            return;
        }
        let mut all: Vec<LinkDevice> = ours
            .iter()
            .map(|e| LinkDevice {
                id: e.device.clone(),
                name: e.info.name.clone(),
                platform: e.info.platform.clone(),
                linked: true,
                state: e.info.reports.then(|| LinkState {
                    position_ms: e.info.position_ms(),
                    ..e.info.state.clone()
                }),
                last_seen: None,
                wakeable: Some(can_push && asleep.iter().any(|t| t.device == e.device)),
                owner: None,
            })
            .collect();
        for t in asleep {
            if !all.iter().any(|d| d.id == t.device) {
                all.push(LinkDevice {
                    id: t.device,
                    name: t.name,
                    platform: t.platform,
                    linked: false,
                    state: None,
                    last_seen: Some(t.last_seen),
                    wakeable: Some(can_push),
                    owner: None,
                });
            }
        }
        all.extend(self.shared_devices(username, &entries, can_push));
        for e in ours.iter().filter(|e| e.wants_devices) {
            let devices = all.iter().filter(|d| d.id != e.device).cloned().collect();
            let _ = e.tx.send(LinkCommand::Devices { devices });
        }
    }

    /// The devices shared with `grantee`, as `grantee` sees them: their
    /// state, with nothing of their owner's own (the outputs are the owner's
    /// to choose); and absent ones only where a push can wake them.
    fn shared_devices(&self, grantee: &str, entries: &[Entry], can_push: bool) -> Vec<LinkDevice> {
        let grants: Vec<Grant> = self
            .grants
            .lock()
            .iter()
            .filter(|g| g.grantee == grantee)
            .cloned()
            .collect();
        let mut out = Vec::new();
        for g in grants {
            let linked = entries
                .iter()
                .find(|e| e.device == g.device && e.info.username == g.owner);
            match linked {
                Some(e) => out.push(LinkDevice {
                    id: e.device.clone(),
                    name: e.info.name.clone(),
                    platform: e.info.platform.clone(),
                    linked: true,
                    state: e.info.reports.then(|| LinkState {
                        position_ms: e.info.position_ms(),
                        ..e.info.state.clone()
                    }),
                    last_seen: None,
                    wakeable: Some(can_push),
                    owner: Some(g.owner.clone()),
                }),
                None => {
                    if let Some(t) = outbox::push_targets(Some(&g.owner))
                        .into_iter()
                        .find(|t| t.device == g.device)
                    {
                        out.push(LinkDevice {
                            id: t.device,
                            name: t.name,
                            platform: t.platform,
                            linked: false,
                            state: None,
                            last_seen: Some(t.last_seen),
                            wakeable: Some(can_push),
                            owner: Some(g.owner.clone()),
                        });
                    }
                }
            }
        }
        out
    }

    /// Let `grantee` control `owner`'s device `device`, or stop letting it.
    /// Asked by the device itself; the caller has checked `grantee` is an
    /// account on this server.
    pub fn share(
        &self,
        owner: &str,
        device: &str,
        grantee: &str,
        allow: bool,
    ) -> Result<(), String> {
        if grantee == owner {
            return Err("a device is already its own account's".into());
        }
        let grant = Grant {
            device: device.to_string(),
            owner: owner.to_string(),
            grantee: grantee.to_string(),
        };
        {
            let mut grants = self.grants.lock();
            grants.retain(|g| *g != grant);
            if allow {
                grants.push(grant.clone());
            }
        }
        if allow {
            outbox::save_grant(&grant);
        } else {
            outbox::drop_grant(&grant);
        }
        log::info!(
            "share: {owner}'s {device} {} {grantee}",
            if allow {
                "shared with"
            } else {
                "no longer shared with"
            }
        );
        self.send_shares(owner, device, None);
        self.announce_to(grantee);
        Ok(())
    }

    /// Tell `owner`'s device `device` whom it is shared with, and why its last
    /// request was refused, if it was.
    pub fn send_shares(&self, owner: &str, device: &str, error: Option<String>) {
        let mut grantees: Vec<String> = self
            .grants
            .lock()
            .iter()
            .filter(|g| g.owner == owner && g.device == device)
            .map(|g| g.grantee.clone())
            .collect();
        grantees.sort();
        if let Some(e) = self
            .entries
            .lock()
            .iter()
            .find(|e| e.info.username == owner && e.device == device && e.wants_devices)
        {
            let accounts = outbox::accounts()
                .into_iter()
                .filter(|a| a != owner)
                .collect();
            let _ = e.tx.send(LinkCommand::Shares {
                grantees,
                error,
                accounts,
            });
        }
    }

    /// `device`, of `username`'s, linked from `addr`.
    pub fn seen_at(&self, device: &str, username: &str, addr: std::net::IpAddr) {
        self.addresses
            .lock()
            .insert(device.to_string(), (username.to_string(), addr));
        outbox::save_address(device, username, addr);
    }

    /// Whose push token wakes `to` when `username`'s device `from` asks: the
    /// owner of a device shared with `username`; the account of one last seen
    /// at the same address as `from` is now, which is to say behind the same
    /// router, a household's; else `username`'s own. A wake grants nothing
    /// else: what may then be done to the device follows the share or the
    /// network as ever.
    fn wake_owner(&self, username: &str, from: &str, to: &str) -> String {
        if let Some(owner) = self.shared_owner(username, to) {
            return owner;
        }
        let addresses = self.addresses.lock();
        let here = addresses
            .get(from)
            .filter(|(user, _)| user == username)
            .map(|(_, addr)| *addr);
        match (here, addresses.get(to)) {
            (Some(here), Some((owner, there))) if *there == here => owner.clone(),
            _ => username.to_string(),
        }
    }

    /// The account `to` belongs to, when `from`, a device of `owner`'s, is
    /// shared with it.
    fn grantee_of(&self, owner: &str, from: &str, to: &str) -> Option<String> {
        let to_user = self
            .entries
            .lock()
            .iter()
            .find(|e| e.device == to && e.info.username != owner)
            .map(|e| e.info.username.clone())?;
        self.grants
            .lock()
            .iter()
            .any(|g| g.owner == owner && g.device == from && g.grantee == to_user)
            .then_some(to_user)
    }

    /// Whose device `to` is, when it is not `username`'s own but shared with
    /// it: the owner.
    fn shared_owner(&self, username: &str, to: &str) -> Option<String> {
        self.grants
            .lock()
            .iter()
            .find(|g| g.grantee == username && g.device == to)
            .map(|g| g.owner.clone())
    }

    /// Wake `to`, one of `username`'s devices that is not linked, for `from`
    /// (a device id), which chose it: with a background push, or with
    /// `notify` a notification asking to be tapped. Logged with the push's
    /// round trip, and remembered so the link that follows is logged with how
    /// long it took.
    pub fn wake(&self, username: &str, from: &str, to: &str, notify: bool) {
        let from_name = self
            .list(Some(username))
            .into_iter()
            .find(|c| c.device == from)
            .map_or_else(|| "Another device".to_string(), |c| c.name);
        let owner = self.wake_owner(username, from, to);
        if owner != username {
            log::info!("wake: {to} is {owner}'s, woken for {username}");
        }
        let username = &owner;
        if self.list(Some(username)).iter().any(|c| c.device == to) {
            log::info!("wake: {to} is already linked");
            return;
        }
        let Some(pusher) = crate::push::pusher() else {
            log::info!("wake: no push key, so {to} cannot be woken");
            return;
        };
        let Some(target) = outbox::push_targets(Some(username))
            .into_iter()
            .find(|t| t.device == to)
        else {
            log::info!("wake: {to} has given no push token");
            return;
        };
        let (push, how) = if notify {
            (
                crate::push::Push::Summon {
                    title: format!("{from_name} wants to play here"),
                    body: "Tap to open kōan and let it.".into(),
                },
                "notification",
            )
        } else {
            (crate::push::Push::WakeNow, "wake push")
        };
        WOKEN.lock().insert(
            (target.device.clone(), target.username.clone()),
            (std::time::Instant::now(), how),
        );
        std::thread::spawn(move || {
            let started = std::time::Instant::now();
            deliver_push(pusher, &target, &push);
            log::info!(
                "wake: {how} for {} answered by APNs in {}ms",
                target.name,
                started.elapsed().as_millis()
            );
        });
    }

    /// Forget `device`, one of `username`'s that is not linked: what the
    /// server keeps of it, its push token included, so nothing is pushed to
    /// it again; and every linked device of the account drops it. Another
    /// account's device is out of reach of this, by construction: every
    /// record is keyed by account. A device that links again is recorded
    /// afresh.
    pub fn forget(&self, username: &str, device: &str) -> Result<(), String> {
        // Another account's device, shared with this one: forgetting it
        // declines the share, or the next list would bring it straight back.
        if let Some(owner) = self.shared_owner(username, device) {
            return self.share(&owner, device, username, false);
        }
        if self.list(Some(username)).iter().any(|c| c.device == device) {
            return Err(format!("{device} is linked; it would be back at once"));
        }
        outbox::forget_device(device, username);
        log::info!("devices: {username} forgot {device}");
        self.broadcast(
            Some(username),
            LinkCommand::Forgotten {
                device: device.to_string(),
            },
        );
        self.announce(username);
        Ok(())
    }

    /// Relay `command` from one of `username`'s devices to another, `to`.
    pub fn relay(
        &self,
        username: &str,
        to: &str,
        command: LinkCommand,
    ) -> Result<ClientInfo, String> {
        self.relay_from(username, None, to, command)
    }

    /// `relay`, knowing which of `username`'s devices, `from`, sent it: what
    /// lets a device shared with another account hand its music to that
    /// account's device, the one way a grant runs backwards.
    pub fn relay_from(
        &self,
        username: &str,
        from: Option<&str>,
        to: &str,
        command: LinkCommand,
    ) -> Result<ClientInfo, String> {
        // Levels are relayed only between live links, by `watch_levels` and
        // `levels`: never queued for a device that is away, never a push.
        if matches!(
            command,
            LinkCommand::Devices { .. }
                | LinkCommand::Forgotten { .. }
                | LinkCommand::HistoryChanged
                | LinkCommand::Levels { .. }
                | LinkCommand::WatchLevels { .. }
                | LinkCommand::Shares { .. }
                | LinkCommand::Shared { .. }
        ) {
            return Err("not a command".into());
        }
        // A device shared with `username` takes the playback set, marked as
        // another account's so it runs as that account's request and never
        // with its owner's powers.
        if let Some(owner) = self.shared_owner(username, to) {
            if !command.allowed_playback() {
                return Err(format!("{to} is shared for playback only"));
            }
            let command = LinkCommand::Shared {
                command: Box::new(command),
            };
            return self.send(Some(&owner), Some(to), command);
        }
        // The other way: a device shared with `to`'s account hands its music
        // there, as "Move here" from that account asks it to. That and only
        // that: a grant does not let the owner's device command the grantee's.
        if let Some(from) = from
            && let Some(grantee) = self.grantee_of(username, from, to)
        {
            if !matches!(command, LinkCommand::Play { handoff: true, .. }) {
                return Err(format!("{to} is another account's"));
            }
            let command = LinkCommand::Shared {
                command: Box::new(command),
            };
            return self.send(Some(&grantee), Some(to), command);
        }
        self.send(Some(username), Some(to), command)
    }

    /// Where to push a Live Activity's updates: `watcher` shows `target`.
    /// `None` ends it.
    pub fn set_activity(
        &self,
        username: &str,
        watcher: &str,
        activity: Option<(String, String, bool)>,
    ) {
        let mut activities = self.activities.lock();
        activities.retain(|a| !(a.username == username && a.watcher == watcher));
        let Some((token, target, sandbox)) = activity else {
            return;
        };
        activities.push(Activity {
            username: username.to_string(),
            watcher: watcher.to_string(),
            target: target.clone(),
            token,
            sandbox,
            sent: None,
        });
        drop(activities);
        self.update_activities(username, &target);
    }

    /// Push `target`'s state to every Live Activity showing it, where it has
    /// changed in a way the activity shows.
    fn update_activities(&self, username: &str, target: &str) {
        let Some(pusher) = crate::push::pusher() else {
            return;
        };
        // A device shared with `username` is its owner's to look up, and the
        // owner's reports reach those it is shared with too.
        let owner = self
            .shared_owner(username, target)
            .unwrap_or_else(|| username.to_string());
        let Some(info) = self
            .list(Some(&owner))
            .into_iter()
            .find(|c| c.device == target)
        else {
            return;
        };
        let watchers: Vec<String> = self
            .grants
            .lock()
            .iter()
            .filter(|g| g.owner == owner && g.device == target)
            .map(|g| g.grantee.clone())
            .chain(std::iter::once(owner.clone()))
            .collect();
        let state = crate::push::ActivityState::of(&info);
        let mut due = Vec::new();
        for a in self.activities.lock().iter_mut() {
            if watchers.contains(&a.username)
                && a.target == target
                && a.sent.as_ref().is_none_or(|s| s.differs(&state))
            {
                a.sent = Some(state.clone());
                due.push((
                    a.token.clone(),
                    a.sandbox,
                    a.watcher.clone(),
                    a.username.clone(),
                ));
            }
        }
        if due.is_empty() {
            return;
        }
        std::thread::spawn(move || {
            for (token, sandbox, watcher, username) in due {
                let push = crate::push::Push::Activity(state.clone());
                match pusher.send(&token, sandbox, &push) {
                    crate::push::Outcome::Sent => {}
                    crate::push::Outcome::Gone => {
                        log::info!("push: a Live Activity on {watcher} has ended");
                        registry().set_activity(&username, &watcher, None);
                    }
                    crate::push::Outcome::Failed(e) => {
                        log::warn!("push: Live Activity on {watcher}: {e}");
                    }
                }
            }
        });
    }

    /// Clients `username` may command, newest first; every client for `None`.
    pub fn list(&self, username: Option<&str>) -> Vec<ClientInfo> {
        let mut out: Vec<ClientInfo> = self
            .entries
            .lock()
            .iter()
            .filter(|e| username.is_none_or(|u| e.info.username == u))
            .map(|e| e.info.clone())
            .collect();
        out.sort_by_key(|c| std::cmp::Reverse(c.connected_at));
        out
    }

    /// Send to `id` (an id or a name), or with none to the client the
    /// command most likely means: the one playing, else the one that played
    /// within `RECENT`, else the only one linked. `Err` names the choices when
    /// there is no telling, so whoever asked can ask the person.
    pub fn send(
        &self,
        username: Option<&str>,
        id: Option<&str>,
        cmd: LinkCommand,
    ) -> Result<ClientInfo, String> {
        let clients = self.list(username);
        let target = match id {
            Some(id) => clients
                .iter()
                .find(|c| c.id == id || c.device == id || c.name.eq_ignore_ascii_case(id)),
            None if clients.is_empty() => None,
            None => Some(pick(&clients, chrono::Utc::now().timestamp())?),
        };
        // Not linked: a phone iOS has suspended is woken to take it.
        let Some(target) = target else {
            return reach_absent(username, id, &cmd).unwrap_or_else(|| {
                Err(match id {
                    Some(id) => format!("no linked client {id}; see `clients`"),
                    None => "no koan app is linked to this server; open koan on the device".into(),
                })
            });
        };
        let entries = self.entries.lock();
        let entry = entries
            .iter()
            .find(|e| e.info.id == target.id)
            .ok_or("that client has just gone")?;
        entry
            .tx
            .send(cmd)
            .map_err(|_| "that client has just gone".to_string())?;
        Ok(target.clone())
    }
}

impl Registry {
    pub fn add_order(&self, order: Order) {
        outbox::save_order(&order);
        self.orders.lock().push(order);
    }

    pub fn orders(&self, username: Option<&str>) -> Vec<Order> {
        self.orders
            .lock()
            .iter()
            .filter(|o| username.is_none() || o.username.as_deref() == username)
            .cloned()
            .collect()
    }

    pub fn cancel_order(&self, username: Option<&str>, id: &str) -> bool {
        let mut orders = self.orders.lock();
        let before = orders.len();
        orders
            .retain(|o| !(o.id == id && (username.is_none() || o.username.as_deref() == username)));
        let gone = orders.len() != before;
        if gone {
            outbox::drop_order(id);
        }
        gone
    }

    fn done(&self, id: &str) {
        self.orders.lock().retain(|o| o.id != id);
        outbox::drop_order(id);
    }

    /// Send every order whose album the library now holds, and drop it.
    /// `find` answers an order with the album's track uids, in order.
    pub fn fulfil_orders(&self, find: impl Fn(&Order) -> Option<Vec<String>>) {
        let now = chrono::Utc::now().timestamp();
        let pending: Vec<Order> = {
            let mut orders = self.orders.lock();
            for o in orders.iter().filter(|o| now - o.created_at >= ORDER_TTL) {
                outbox::drop_order(&o.id);
            }
            orders.retain(|o| now - o.created_at < ORDER_TTL);
            orders.clone()
        };
        for order in pending.into_iter().filter(|o| o.playlist.is_none()) {
            let Some(ids) = find(&order).filter(|ids| !ids.is_empty()) else {
                continue;
            };
            let track_ids = ids;
            let cmd = if order.play_next {
                LinkCommand::PlayNext { track_ids }
            } else {
                LinkCommand::Enqueue { track_ids }
            };
            match self.send(order.username.as_deref(), order.client.as_deref(), cmd) {
                Ok(c) => {
                    log::info!(
                        "link: {} — {} arrived; queued on {}",
                        order.artist,
                        order.album,
                        c.name
                    );
                    self.done(&order.id);
                }
                // No device to send to yet: kept, and tried after the next scan.
                Err(e) => log::info!("link: {} — {} arrived but {e}", order.artist, order.album),
            }
        }
    }
}

/// Fulfil standing orders against the library at `db_path`.
pub fn fulfil_from(db_path: &std::path::Path) {
    let registry = registry();
    if registry.orders.lock().is_empty() {
        return;
    }
    let Ok(db) = koan_core::db::connection::Database::open_existing(db_path) else {
        return;
    };
    // Playlist orders are the server's own to carry out: add the tracks, and
    // the playlist reaches every device like any other edit.
    let for_playlists: Vec<Order> = registry
        .orders
        .lock()
        .iter()
        .filter(|o| o.playlist.is_some())
        .cloned()
        .collect();
    let mut edited = false;
    for order in for_playlists {
        let Some(playlist) = order.playlist else {
            continue;
        };
        let Some(ids) = order_tracks(&db.conn, &order) else {
            continue;
        };
        match koan_core::db::queries::add_tracks(&db.conn, playlist, &ids) {
            Ok(_) => {
                // Tests would push to whatever server this machine signs in to.
                if !cfg!(test) {
                    koan_core::playlists::push_to_remote(playlist);
                }
                log::info!(
                    "link: {} — {} arrived; added {} tracks to playlist {playlist}",
                    order.artist,
                    order.album,
                    ids.len()
                );
                registry.done(&order.id);
                edited = true;
            }
            Err(e) => log::warn!("link: could not add to playlist {playlist}: {e}"),
        }
    }
    if edited {
        changed();
    }
    registry.fulfil_orders(|order| {
        let rows = order_tracks(&db.conn, order)?;
        queries::uids_in_order(&db.conn, UidKind::Track, &rows).ok()
    });
}

/// The tracks an order asks for, once its album is in the library: all of
/// it, or the named ones in the order named. `None` until they are there.
fn order_tracks(conn: &rusqlite::Connection, order: &Order) -> Option<Vec<i64>> {
    let tracks = album_tracks(conn, &order.artist, &order.album)?;
    if order.titles.is_empty() {
        return Some(tracks.into_iter().map(|(id, _)| id).collect());
    }
    let picked: Vec<i64> = order
        .titles
        .iter()
        .filter_map(|want| {
            let want = want.to_lowercase();
            tracks
                .iter()
                .find(|(_, t)| t.to_lowercase().contains(&want))
                .map(|(id, _)| *id)
        })
        .collect();
    (!picked.is_empty()).then_some(picked)
}

/// The newest album whose artist and title contain these, as its tracks in
/// disc and track order.
pub fn album_tracks(
    conn: &rusqlite::Connection,
    artist: &str,
    album: &str,
) -> Option<Vec<(i64, String)>> {
    let like = |s: &str| format!("%{}%", s.replace(['%', '_'], ""));
    let album_id: i64 = conn
        .query_row(
            "SELECT al.id FROM albums al JOIN artists a ON a.id = al.artist_id
              WHERE a.name LIKE ?1 COLLATE NOCASE AND al.title LIKE ?2 COLLATE NOCASE
              ORDER BY al.id DESC LIMIT 1",
            [like(artist), like(album)],
            |r| r.get(0),
        )
        .ok()?;
    let mut stmt = conn
        .prepare("SELECT id, title FROM tracks WHERE album_id = ?1 ORDER BY disc, track_number, id")
        .ok()?;
    let tracks = stmt
        .query_map([album_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .ok()?
        .filter_map(Result::ok)
        .collect();
    Some(tracks)
}

impl Registry {
    /// Send to every client `username` may command. The names of those it
    /// reached.
    pub fn broadcast(&self, username: Option<&str>, cmd: LinkCommand) -> Vec<String> {
        let ids: Vec<String> = self.list(username).into_iter().map(|c| c.id).collect();
        let entries = self.entries.lock();
        entries
            .iter()
            .filter(|e| ids.contains(&e.info.id) && e.tx.send(cmd.clone()).is_ok())
            .map(|e| e.info.name.clone())
            .collect()
    }
}

impl Registry {
    /// Send to every device `username` may command: at once to those linked,
    /// and to those that have linked before but are away now, when they next
    /// link. For commands still right hours later (`Sync`, `Evict`), never
    /// playback. The names reached now, and the names it waits for.
    pub fn deliver(&self, username: Option<&str>, cmd: LinkCommand) -> (Vec<String>, Vec<String>) {
        let (sent, queued) = self.link_or_queue(username, &cmd);
        wake(&queued.iter().map(Absent::key).collect::<Vec<_>>());
        (sent, queued.into_iter().map(|q| q.name).collect())
    }

    fn link_or_queue(
        &self,
        username: Option<&str>,
        cmd: &LinkCommand,
    ) -> (Vec<String>, Vec<Absent>) {
        let sent = self.broadcast(username, cmd.clone());
        let queued = outbox::queue_for_absent(username, &self.live(), cmd);
        (sent, queued)
    }

    /// Each linked device, as `(device, username)`.
    fn live(&self) -> Vec<(String, String)> {
        self.entries
            .lock()
            .iter()
            .map(|e| (e.device.clone(), e.info.username.clone()))
            .collect()
    }
}

impl Absent {
    fn key(&self) -> (String, String) {
        (self.device.clone(), self.username.clone())
    }
}

/// Wake these absent devices, each a `(device, username)`, where a push can:
/// each links, and takes what was queued for it.
fn wake(devices: &[(String, String)]) {
    let Some(pusher) = crate::push::pusher() else {
        return;
    };
    let targets: Vec<outbox::PushTarget> = outbox::push_targets(None)
        .into_iter()
        .filter(|t| {
            devices
                .iter()
                .any(|(device, username)| *device == t.device && *username == t.username)
        })
        .collect();
    if targets.is_empty() {
        return;
    }
    std::thread::spawn(move || {
        for t in targets {
            deliver_push(pusher, &t, &crate::push::Push::Wake);
        }
    });
}

/// Reach a device that is not linked: the one named, else the one seen most
/// recently.
///
/// Music is sent as a notification to tap, at once: iOS does not let an app it
/// woke start audio, so waking it for that only delays the notification. Every
/// other command goes into the device's outbox with a background push to wake
/// it, and runs as if it had been linked. `None` without a push key, or with no
/// device in scope that has given a push token.
fn reach_absent(
    username: Option<&str>,
    id: Option<&str>,
    cmd: &LinkCommand,
) -> Option<Result<ClientInfo, String>> {
    let pusher = crate::push::pusher()?;
    // Most recently seen first: a reinstall leaves its old entry behind under
    // the same name, and the newest is the one in the person's hand.
    let target = outbox::push_targets(username)
        .into_iter()
        .find(|t| id.is_none_or(|id| t.device == id || t.name.eq_ignore_ascii_case(id)))?;
    let info = ClientInfo {
        id: target.device.clone(),
        device: target.device.clone(),
        name: target.name.clone(),
        platform: target.platform.clone(),
        username: target.username.clone(),
        connected_at: 0,
        state: LinkState::default(),
        last_played_at: None,
        state_at: 0,
        reports: false,
        notified: true,
    };
    // What it asks, whoever asks it.
    let asked = match cmd {
        LinkCommand::Shared { command } => command.as_ref(),
        cmd => cmd,
    };
    let verb = match asked {
        LinkCommand::Play { .. } | LinkCommand::JumpTo { .. } | LinkCommand::Resume => Some("Play"),
        _ => None,
    };
    let push = match verb {
        Some(verb) => crate::push::Push::Notify {
            title: format!("{verb} on {}", target.name),
            body: outbox::describe(asked).unwrap_or_else(|| "From your koan server".into()),
            command: serde_json::to_value(cmd).ok()?,
            image: cover_track(asked)
                .and_then(outbox::track_row)
                .and_then(|t| pusher.cover_link(t)),
        },
        None => {
            outbox::queue_for(&target.device, &target.username, cmd);
            crate::push::Push::Wake
        }
    };
    std::thread::spawn(move || deliver_push(pusher, &target, &push));
    Some(Ok(info))
}

/// The track whose album cover a notification for `cmd` shows. Commands carry
/// uids, so this is the id as sent; see `outbox::track_row`.
fn cover_track(cmd: &LinkCommand) -> Option<&str> {
    match cmd {
        LinkCommand::Play {
            track_ids,
            start_at,
            ..
        } => track_ids
            .get(*start_at as usize)
            .or(track_ids.first())
            .map(String::as_str),
        LinkCommand::JumpTo { track_id } => Some(track_id),
        _ => None,
    }
}

/// Send one push, forgetting a token Apple says is no longer good.
fn deliver_push(
    pusher: &crate::push::Pusher,
    target: &outbox::PushTarget,
    push: &crate::push::Push,
) {
    use crate::push::Outcome;
    match pusher.send(&target.token, target.sandbox, push) {
        Outcome::Sent => log::info!("push: sent to {}", target.name),
        Outcome::Gone => {
            log::info!(
                "push: {}'s token is no longer valid; forgotten",
                target.name
            );
            outbox::forget_push(&target.username, &target.device);
        }
        Outcome::Failed(e) => log::warn!("push: to {} failed: {e}", target.name),
    }
}

/// Wakes sent and not yet answered by a link, by `(device, username)`: when,
/// and which kind. A device that does not link is never answered; the map
/// is bounded by the devices that have pushed tokens.
type Woken = std::collections::HashMap<(String, String), (std::time::Instant, &'static str)>;

static WOKEN: LazyLock<Mutex<Woken>> = LazyLock::new(Default::default);

/// After `user` played or favourited something: re-evaluate their smart
/// playlists that read it, and have every device pull any that moved, so a
/// "most played" list on an idle device does not wait for its next read.
pub fn smart_activity(
    db: &koan_core::db::connection::Database,
    user: i64,
    fields: &[koan_core::smart::Field],
) {
    match koan_core::db::queries::smart::refresh_after_activity(&db.conn, user, fields) {
        Ok(moved) if !moved.is_empty() => changed(),
        Ok(_) => {}
        Err(e) => log::warn!("smart playlists not refreshed after activity: {e}"),
    }
}

/// Have every device pull what the server just changed (a playlist edited,
/// albums added): at once where linked, on next link where not. Syncs waiting
/// for a device collapse into one, and so do the pushes that wake it: see
/// `Wakes`.
pub fn changed() {
    let (_, queued) = registry().link_or_queue(None, &LinkCommand::Sync { full: false });
    if queued.is_empty() || crate::push::pusher().is_none() {
        return;
    }
    let mut wakes = WAKES.lock();
    wakes.add(queued.iter().map(Absent::key), std::time::Instant::now());
    if !wakes.timer {
        wakes.timer = true;
        std::thread::spawn(send_wakes);
    }
}

/// How long the library has to stay still before a suspended device is woken
/// to sync. A download of several albums scans after each one; iOS rations
/// background pushes, and the device needs waking once, at the end.
const QUIET: std::time::Duration = std::time::Duration::from_secs(30);

/// However busy the library stays, a device waits no longer than this.
const LONGEST_WAIT: std::time::Duration = std::time::Duration::from_secs(5 * 60);

static WAKES: Mutex<Wakes> = Mutex::new(Wakes {
    pending: Vec::new(),
    timer: false,
});

/// Background pushes held back until the library is quiet: at most one
/// pending per device.
struct Wakes {
    /// `(device, username)`, when first asked for, when last asked for.
    pending: Vec<((String, String), std::time::Instant, std::time::Instant)>,
    /// Whether a thread is waiting to send them.
    timer: bool,
}

impl Wakes {
    fn add(
        &mut self,
        devices: impl IntoIterator<Item = (String, String)>,
        now: std::time::Instant,
    ) {
        for key in devices {
            match self.pending.iter_mut().find(|(k, _, _)| *k == key) {
                Some((_, _, last)) => *last = now,
                None => self.pending.push((key, now, now)),
            }
        }
    }

    fn due_at(first: std::time::Instant, last: std::time::Instant) -> std::time::Instant {
        (last + QUIET).min(first + LONGEST_WAIT)
    }

    /// The earliest a pending wake is due.
    fn next(&self) -> Option<std::time::Instant> {
        self.pending
            .iter()
            .map(|(_, first, last)| Self::due_at(*first, *last))
            .min()
    }

    /// Take the wakes due by `now`.
    fn take_due(&mut self, now: std::time::Instant) -> Vec<(String, String)> {
        let (due, waiting) = std::mem::take(&mut self.pending)
            .into_iter()
            .partition(|(_, first, last)| Self::due_at(*first, *last) <= now);
        self.pending = waiting;
        due.into_iter().map(|(key, _, _)| key).collect()
    }
}

/// Send each wake once it is due, until none is pending. A device that has
/// linked meanwhile took its sync down the link and is not pushed.
fn send_wakes() {
    loop {
        let (due, next) = {
            let mut wakes = WAKES.lock();
            let due = wakes.take_due(std::time::Instant::now());
            let next = wakes.next();
            if due.is_empty() && next.is_none() {
                wakes.timer = false;
                return;
            }
            (due, next)
        };
        let live = registry().live();
        let absent: Vec<(String, String)> = due.into_iter().filter(|d| !live.contains(d)).collect();
        if !absent.is_empty() {
            wake(&absent);
        }
        if let Some(next) = next {
            std::thread::sleep(next.saturating_duration_since(std::time::Instant::now()));
        }
    }
}

/// After a library scan or sync: if the library holds different tracks or
/// albums from when this was last asked, tell every device. A scan that found
/// nothing new, which is most of them, sends nothing.
pub fn changed_if_library_moved(conn: &rusqlite::Connection) {
    static LAST: parking_lot::Mutex<Option<(i64, i64, i64)>> = parking_lot::Mutex::new(None);
    let Ok(now) = conn.query_row(
        "SELECT (SELECT COUNT(*) FROM tracks), (SELECT COALESCE(MAX(id), 0) FROM tracks),
                (SELECT COUNT(*) FROM albums)",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ) else {
        return;
    };
    let before = LAST.lock().replace(now);
    if before.is_some_and(|b| b != now) {
        changed();
    }
}

/// The server-side record of devices and their waiting commands, in the
/// library database so it outlives a restart.
mod outbox {
    use koan_core::db::queries;
    use koan_core::remote::link::LinkCommand;

    /// Dropped undelivered after this long: a device away a month re-syncs
    /// on its own when opened. A device unseen this long is forgotten with
    /// its push token, so the entry a reinstall leaves behind stops being
    /// offered as a device; a live one gives its token again when it links.
    const KEEP_SECS: i64 = 30 * 24 * 60 * 60;

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

    pub fn load_orders() -> Vec<super::Order> {
        let Some(db) = db() else { return Vec::new() };
        db.conn
            .prepare("SELECT body FROM link_orders ORDER BY created_at")
            .and_then(|mut s| {
                s.query_map([], |r| r.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()
            })
            .unwrap_or_default()
            .into_iter()
            .filter_map(|b| serde_json::from_str(&b).ok())
            .collect()
    }

    pub fn save_order(order: &super::Order) {
        let (Some(db), Ok(body)) = (db(), serde_json::to_string(order)) else {
            return;
        };
        let _ = db.conn.execute(
            "INSERT OR REPLACE INTO link_orders (id, body, created_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![order.id, body, order.created_at],
        );
    }

    pub fn drop_order(id: &str) {
        if let Some(db) = db() {
            let _ = db
                .conn
                .execute("DELETE FROM link_orders WHERE id = ?1", [id]);
        }
    }

    pub fn take_and_remember(
        username: &str,
        device: &str,
        name: &str,
        platform: &str,
        live: &[(String, String)],
    ) -> Vec<LinkCommand> {
        let Some(db) = db() else { return Vec::new() };
        let now = chrono::Utc::now().timestamp();
        let _ = db.conn.execute(
            "INSERT INTO link_devices (device, username, name, platform, last_seen) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (device, username) DO UPDATE SET name = ?3, platform = ?4, last_seen = ?5",
            rusqlite::params![device, username, name, platform, now],
        );
        forget_stale(&db.conn, live, now);
        let waiting: Vec<(i64, String)> = db
            .conn
            .prepare("SELECT id, command FROM link_outbox WHERE device = ?1 AND username = ?2 ORDER BY id")
            .and_then(|mut s| {
                s.query_map([device, username], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect()
            })
            .unwrap_or_default();
        let _ = db.conn.execute(
            "DELETE FROM link_outbox WHERE device = ?1 AND username = ?2",
            [device, username],
        );
        if !waiting.is_empty() {
            log::info!("link: {} waiting commands for {name}", waiting.len());
        }
        waiting
            .into_iter()
            .filter_map(|(_, c)| serde_json::from_str(&c).ok())
            .collect()
    }

    /// Drop undelivered commands, devices and push tokens older than
    /// `KEEP_SECS`. A link held open that long is seen, not stale.
    pub(super) fn forget_stale(conn: &rusqlite::Connection, live: &[(String, String)], now: i64) {
        touch_with(conn, live, now);
        let _ = conn.execute(
            "DELETE FROM link_outbox WHERE created_at < ?1",
            [now - KEEP_SECS],
        );
        let _ = conn.execute(
            "DELETE FROM link_push WHERE (device, username) IN
               (SELECT device, username FROM link_devices WHERE last_seen < ?1)",
            [now - KEEP_SECS],
        );
        let _ = conn.execute(
            "DELETE FROM link_devices WHERE last_seen < ?1",
            [now - KEEP_SECS],
        );
    }

    /// Mark `(device, username)` pairs seen now.
    pub fn touch(devices: &[(String, String)]) {
        if let Some(db) = db() {
            touch_with(&db.conn, devices, chrono::Utc::now().timestamp());
        }
    }

    fn touch_with(conn: &rusqlite::Connection, devices: &[(String, String)], now: i64) {
        for (device, username) in devices {
            let _ = conn.execute(
                "UPDATE link_devices SET last_seen = ?1 WHERE device = ?2 AND username = ?3",
                rusqlite::params![now, device, username],
            );
        }
    }

    /// A known device that was not linked when something was queued for it.
    pub struct Absent {
        pub device: String,
        pub username: String,
        pub name: String,
    }

    /// Queue `cmd` for each known device in scope that is not in `live`.
    pub fn queue_for_absent(
        username: Option<&str>,
        live: &[(String, String)],
        cmd: &LinkCommand,
    ) -> Vec<Absent> {
        let Some(db) = db() else { return Vec::new() };
        let known: Vec<(String, String, String)> = db
            .conn
            .prepare("SELECT device, username, name FROM link_devices")
            .and_then(|mut s| {
                s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                    .collect()
            })
            .unwrap_or_default();
        let Ok(text) = serde_json::to_string(cmd) else {
            return Vec::new();
        };
        let absent: Vec<_> = known
            .into_iter()
            .filter(|(device, user, _)| {
                !username.is_some_and(|u| u != user)
                    && !live.iter().any(|(d, u)| d == device && u == user)
            })
            .collect();
        if absent.is_empty() {
            return Vec::new();
        }
        let is_sync = matches!(cmd, LinkCommand::Sync { .. });
        let now = chrono::Utc::now().timestamp();
        // Every playlist edit and scan lands here: one transaction, not two
        // per device.
        koan_core::db::queries::atomically(&db.conn, || {
            let mut queued = Vec::new();
            for (device, user, name) in absent {
                if is_sync {
                    // One pending sync is enough; a full one covers an incremental.
                    let _ = db.conn.execute(
                        "DELETE FROM link_outbox WHERE device = ?1 AND username = ?2 AND command LIKE '{\"type\":\"sync\"%'",
                        [&device, &user],
                    );
                }
                if db
                    .conn
                    .execute(
                        "INSERT INTO link_outbox (device, username, command, created_at) VALUES (?1, ?2, ?3, ?4)",
                        rusqlite::params![device, user, text, now],
                    )
                    .is_ok()
                {
                    queued.push(Absent {
                        device,
                        username: user,
                        name,
                    });
                }
            }
            Ok::<_, rusqlite::Error>(queued)
        })
        .unwrap_or_default()
    }

    /// A device Apple's push service can reach.
    #[derive(Clone)]
    pub struct PushTarget {
        pub device: String,
        pub username: String,
        pub name: String,
        pub platform: String,
        pub token: String,
        pub sandbox: bool,
        /// Unix seconds.
        pub last_seen: i64,
    }

    /// Queue `cmd` for one device, to go down its next link.
    pub fn queue_for(device: &str, username: &str, cmd: &LinkCommand) {
        let (Some(db), Ok(text)) = (db(), serde_json::to_string(cmd)) else {
            return;
        };
        let _ = db.conn.execute(
            "INSERT INTO link_outbox (device, username, command, created_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![device, username, text, chrono::Utc::now().timestamp()],
        );
    }

    /// Every account on the server, by name: what an owner chooses from to
    /// share a device. Any account may see them; the server's users trust
    /// each other that far.
    pub fn accounts() -> Vec<String> {
        let Some(db) = db() else { return Vec::new() };
        koan_core::db::queries::auth::list_users(&db.conn)
            .map(|users| users.into_iter().map(|u| u.username).collect())
            .unwrap_or_default()
    }

    /// Where each device last linked from, by device id: the most recent of
    /// its rows, should it have linked as more than one account.
    pub fn load_addresses() -> std::collections::HashMap<String, (String, std::net::IpAddr)> {
        let Some(db) = db() else {
            return Default::default();
        };
        load_addresses_in(&db.conn)
    }

    pub(super) fn load_addresses_in(
        conn: &rusqlite::Connection,
    ) -> std::collections::HashMap<String, (String, std::net::IpAddr)> {
        conn.prepare(
            "SELECT device, username, addr FROM link_devices WHERE addr IS NOT NULL ORDER BY last_seen",
        )
        .and_then(|mut s| {
            s.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()
        })
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(device, username, addr)| Some((device, (username, addr.parse().ok()?))))
        .collect()
    }

    pub fn save_address(device: &str, username: &str, addr: std::net::IpAddr) {
        if let Some(db) = db() {
            save_address_in(&db.conn, device, username, addr);
        }
    }

    pub(super) fn save_address_in(
        conn: &rusqlite::Connection,
        device: &str,
        username: &str,
        addr: std::net::IpAddr,
    ) {
        let _ = conn.execute(
            "UPDATE link_devices SET addr = ?1 WHERE device = ?2 AND username = ?3",
            rusqlite::params![addr.to_string(), device, username],
        );
    }

    pub fn load_grants() -> Vec<super::Grant> {
        let Some(db) = db() else { return Vec::new() };
        db.conn
            .prepare("SELECT device, owner, grantee FROM link_grants ORDER BY created_at")
            .and_then(|mut s| {
                s.query_map([], |r| {
                    Ok(super::Grant {
                        device: r.get(0)?,
                        owner: r.get(1)?,
                        grantee: r.get(2)?,
                    })
                })?
                .collect()
            })
            .unwrap_or_default()
    }

    pub fn save_grant(g: &super::Grant) {
        if let Some(db) = db() {
            let _ = db.conn.execute(
                "INSERT OR IGNORE INTO link_grants (device, owner, grantee, created_at) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![g.device, g.owner, g.grantee, chrono::Utc::now().timestamp()],
            );
        }
    }

    pub fn drop_grant(g: &super::Grant) {
        if let Some(db) = db() {
            let _ = db.conn.execute(
                "DELETE FROM link_grants WHERE device = ?1 AND owner = ?2 AND grantee = ?3",
                [&g.device, &g.owner, &g.grantee],
            );
        }
    }

    pub fn save_push(username: &str, device: &str, token: &str, sandbox: bool) {
        let Some(db) = db() else { return };
        let _ = db.conn.execute(
            "INSERT INTO link_push (device, username, token, sandbox, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (device, username) DO UPDATE SET token = ?3, sandbox = ?4, updated_at = ?5",
            rusqlite::params![device, username, token, sandbox, chrono::Utc::now().timestamp()],
        );
    }

    pub fn forget_push(username: &str, device: &str) {
        if let Some(db) = db() {
            let _ = db.conn.execute(
                "DELETE FROM link_push WHERE device = ?1 AND username = ?2",
                [device, username],
            );
        }
    }

    /// Devices in scope with a push token, most recently seen first.
    pub fn push_targets(username: Option<&str>) -> Vec<PushTarget> {
        let Some(db) = db() else { return Vec::new() };
        push_targets_in(&db.conn, username)
    }

    pub(super) fn push_targets_in(
        conn: &rusqlite::Connection,
        username: Option<&str>,
    ) -> Vec<PushTarget> {
        conn
            .prepare(
                "SELECT p.device, p.username, d.name, d.platform, p.token, p.sandbox, d.last_seen
                   FROM link_push p JOIN link_devices d ON d.device = p.device AND d.username = p.username
                  WHERE ?1 IS NULL OR p.username = ?1
                  ORDER BY d.last_seen DESC",
            )
            .and_then(|mut s| {
                s.query_map([username], |r| {
                    Ok(PushTarget {
                        device: r.get(0)?,
                        username: r.get(1)?,
                        name: r.get(2)?,
                        platform: r.get(3)?,
                        token: r.get(4)?,
                        sandbox: r.get(5)?,
                        last_seen: r.get(6)?,
                    })
                })?
                .collect()
            })
            .unwrap_or_default()
    }

    /// Forget `username`'s device `device`: its record, its push token and
    /// what waits for it. It is recorded afresh if it links again.
    pub fn forget_device(device: &str, username: &str) {
        if let Some(db) = db() {
            forget_device_in(&db.conn, device, username);
        }
    }

    pub(super) fn forget_device_in(conn: &rusqlite::Connection, device: &str, username: &str) {
        for table in ["link_push", "link_outbox", "link_devices"] {
            let _ = conn.execute(
                &format!("DELETE FROM {table} WHERE device = ?1 AND username = ?2"),
                [device, username],
            );
        }
    }

    /// The row id of a track a command names by uid or row id.
    pub fn track_row(id: &str) -> Option<i64> {
        let db = db()?;
        queries::resolve_id(&db.conn, queries::UidKind::Track, id)
            .ok()
            .flatten()
    }

    /// What a playback command would play, for a notification to say:
    /// "Golden Standard — Tony Petersen", or a track and how many follow.
    pub fn describe(cmd: &LinkCommand) -> Option<String> {
        let ids: Vec<&String> = match cmd {
            LinkCommand::Play { track_ids, .. }
            | LinkCommand::Enqueue { track_ids }
            | LinkCommand::PlayNext { track_ids } => track_ids.iter().collect(),
            LinkCommand::JumpTo { track_id } => vec![track_id],
            _ => return None,
        };
        let db = db()?;
        let ids: Vec<i64> = ids
            .into_iter()
            .filter_map(|t| {
                queries::resolve_id(&db.conn, queries::UidKind::Track, t)
                    .ok()
                    .flatten()
            })
            .collect();
        let row = |id: i64| {
            db.conn
                .query_row(
                    "SELECT t.title, COALESCE(a.name, ''), COALESCE(al.title, ''), t.album_id
                       FROM tracks t LEFT JOIN artists a ON a.id = t.artist_id
                       LEFT JOIN albums al ON al.id = t.album_id WHERE t.id = ?1",
                    [id],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                            r.get::<_, Option<i64>>(3)?,
                        ))
                    },
                )
                .ok()
        };
        let (title, artist, album, album_id) = row(*ids.first()?)?;
        let one_album = ids.len() > 1
            && album_id.is_some()
            && ids
                .iter()
                .all(|id| row(*id).is_some_and(|r| r.3 == album_id));
        Some(match (one_album, ids.len()) {
            (true, _) => format!("{album} — {artist}"),
            (false, 1) => format!("{title} — {artist}"),
            (false, n) => format!("{title} — {artist}, and {} more", n - 1),
        })
    }
}

/// How long ago a client can have stopped playing and still be the obvious
/// one to send music to.
const RECENT: i64 = 6 * 60 * 60;

fn pick(clients: &[ClientInfo], now: i64) -> Result<&ClientInfo, String> {
    if let Some(c) = clients.iter().find(|c| c.state.playing) {
        return Ok(c);
    }
    if let Some(c) = clients
        .iter()
        .filter(|c| c.last_played_at.is_some_and(|t| now - t < RECENT))
        .max_by_key(|c| c.last_played_at)
    {
        return Ok(c);
    }
    match clients {
        [] => Err("no koan app is linked to this server; open koan on the device".into()),
        [only] => Ok(only),
        several => Err(format!(
            "several koan apps are linked and none has played recently: {}. Ask which, then pass `client`",
            several
                .iter()
                .map(|c| format!("{} ({}, id {})", c.name, c.platform, c.device))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_watch_is_never_queued_and_is_renewed_when_the_target_relinks() {
        let reg = Registry::default();
        let watches = |rx: &mut tokio::sync::mpsc::UnboundedReceiver<LinkCommand>| {
            std::iter::from_fn(|| rx.try_recv().ok())
                .filter(|c| matches!(c, LinkCommand::WatchLevels { .. }))
                .collect::<Vec<_>>()
        };
        // Not as a relayed command: that path queues and pushes.
        assert!(
            reg.relay("rl", "rl-phone", LinkCommand::WatchLevels { on: true })
                .is_err()
        );
        // Watched while away: nothing is queued for it.
        reg.watch_levels("rl", "rl-mac", "rl-phone", true);
        let (tx, mut phone) = tokio::sync::mpsc::unbounded_channel();
        reg.register("rl", "phone", "ios", "rl-phone", tx, false);
        assert_eq!(
            watches(&mut phone),
            vec![LinkCommand::WatchLevels { on: true }],
            "told once, on linking, because it is watched now"
        );
        // Relinked: a new session, told again.
        let (tx, mut phone) = tokio::sync::mpsc::unbounded_channel();
        reg.register("rl", "phone", "ios", "rl-phone", tx, false);
        assert_eq!(
            watches(&mut phone),
            vec![LinkCommand::WatchLevels { on: true }]
        );
    }

    #[test]
    fn levels_reach_a_watcher_only_while_it_watches() {
        use koan_core::remote::levels::Frame;
        let reg = Registry::default();
        let (tx_mac, mut mac) = tokio::sync::mpsc::unbounded_channel();
        let (tx_phone, mut phone) = tokio::sync::mpsc::unbounded_channel();
        let mac_id = reg.register("lv", "mac", "macos", "lv-mac", tx_mac, false);
        reg.register("lv", "phone", "ios", "lv-phone", tx_phone, false);
        let levels = |rx: &mut tokio::sync::mpsc::UnboundedReceiver<LinkCommand>| {
            std::iter::from_fn(|| rx.try_recv().ok())
                .filter(|c| {
                    matches!(
                        c,
                        LinkCommand::Levels { .. } | LinkCommand::WatchLevels { .. }
                    )
                })
                .collect::<Vec<_>>()
        };
        let f = Frame(1_000, 1, 2, 3);

        reg.levels("lv", "lv-phone", f);
        assert!(levels(&mut mac).is_empty(), "nobody watching");

        reg.watch_levels("lv", "lv-mac", "lv-phone", true);
        assert_eq!(
            levels(&mut phone),
            vec![LinkCommand::WatchLevels { on: true }]
        );
        reg.levels("lv", "lv-phone", f);
        assert_eq!(
            levels(&mut mac),
            vec![LinkCommand::Levels {
                from: "lv-phone".into(),
                f
            }]
        );

        // The watcher's link goes: the phone is told to stop.
        reg.unregister(&mac_id);
        assert_eq!(
            levels(&mut phone),
            vec![LinkCommand::WatchLevels { on: false }]
        );
        reg.watch_levels("lv", "lv-mac", "lv-phone", true);
        reg.watch_levels("lv", "lv-mac", "lv-phone", false);
        assert_eq!(
            levels(&mut phone),
            vec![
                LinkCommand::WatchLevels { on: true },
                LinkCommand::WatchLevels { on: false }
            ]
        );
    }

    #[test]
    fn a_playlist_order_adds_the_named_tracks_once_they_arrive() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("koan.db");
        let db = koan_core::db::connection::Database::open(&path).unwrap();
        let playlist = koan_core::db::queries::create_playlist(
            &db.conn,
            koan_core::db::queries::LOCAL_USER,
            "cyberpunk",
            None,
        )
        .unwrap();
        let order = Order {
            id: "o1".into(),
            username: None,
            client: None,
            artist: "Perturbator".into(),
            album: "Dangerous Days".into(),
            play_next: false,
            playlist: Some(playlist),
            titles: vec!["Future Club".into()],
            created_at: chrono::Utc::now().timestamp(),
        };
        registry().add_order(order);

        // Not in the library yet: nothing happens, and the order waits.
        fulfil_from(&path);
        assert!(registry().orders(None).iter().any(|o| o.id == "o1"));

        db.conn
            .execute_batch(
                "INSERT INTO artists (id, name) VALUES (1, 'Perturbator');
                 INSERT INTO albums (id, title, artist_id) VALUES (1, 'Dangerous Days', 1);
                 INSERT INTO tracks (id, title, album_id, artist_id, track_number, path) VALUES
                   (1, 'Welcome Back', 1, 1, 1, '/1.flac'), (2, 'Future Club', 1, 1, 2, '/2.flac');",
            )
            .unwrap();
        fulfil_from(&path);
        let held: Vec<i64> = db
            .conn
            .prepare("SELECT track_id FROM playlist_tracks WHERE playlist_id = ?1")
            .unwrap()
            .query_map([playlist], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(held, [2]);
        assert!(!registry().orders(None).iter().any(|o| o.id == "o1"));
    }

    use super::*;

    #[test]
    fn a_notification_cover_is_named_by_the_uid_the_command_carries() {
        // Commands carry uids; parsing them as row ids found no cover.
        let uid = "0190a5b2-7c3d-7e4f-8a1b-2c3d4e5f6a7b".to_string();
        let play = LinkCommand::Play {
            track_ids: vec!["x".into(), uid.clone()],
            start_at: 1,
            position_ms: 0,
            paused: false,
            handoff: false,
        };
        assert_eq!(cover_track(&play), Some(uid.as_str()));
    }

    #[test]
    fn a_reconnect_replaces_the_device_and_commands_reach_it() {
        let reg = Registry::default();
        let (tx1, _rx1) = tokio::sync::mpsc::unbounded_channel();
        let (tx2, mut rx2) = tokio::sync::mpsc::unbounded_channel();
        let (tx3, _rx3) = tokio::sync::mpsc::unbounded_channel();
        reg.register("j", "phone", "ios", "dev-1", tx1, false);
        let id = reg.register("j", "phone", "ios", "dev-1", tx2, false);
        reg.register("someone", "laptop", "macos", "dev-2", tx3, false);

        assert_eq!(reg.list(Some("j")).len(), 1);
        assert_eq!(reg.list(None).len(), 2);

        let sent = reg.send(Some("j"), None, LinkCommand::Pause).unwrap();
        assert_eq!(sent.id, id);
        assert_eq!(rx2.try_recv().unwrap(), LinkCommand::Pause);

        // Another account's device is not this account's to command.
        assert!(
            reg.send(Some("j"), Some("laptop"), LinkCommand::Pause)
                .is_err()
        );

        reg.unregister(&id);
        assert!(reg.send(Some("j"), None, LinkCommand::Pause).is_err());
    }

    #[test]
    fn disconnecting_an_account_closes_only_its_links() {
        let reg = Registry::default();
        let (tx1, mut rx1) = tokio::sync::mpsc::unbounded_channel();
        let (tx2, mut rx2) = tokio::sync::mpsc::unbounded_channel();
        reg.register("j", "phone", "ios", "dev-1", tx1, false);
        reg.register("someone", "laptop", "macos", "dev-2", tx2, false);

        reg.disconnect("j");
        assert!(reg.list(Some("j")).is_empty());
        // The session sees its channel close, which is what ends it.
        assert!(matches!(
            rx1.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected)
        ));
        assert_eq!(reg.list(Some("someone")).len(), 1);
        assert!(matches!(
            rx2.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
    }

    #[test]
    fn a_burst_of_changes_wakes_each_device_once_when_it_goes_quiet() {
        let t0 = std::time::Instant::now();
        let s = std::time::Duration::from_secs;
        let phone = || ("dev-1".to_string(), "j".to_string());
        let ipad = || ("dev-2".to_string(), "j".to_string());
        let mut wakes = Wakes {
            pending: Vec::new(),
            timer: false,
        };

        wakes.add([phone()], t0);
        wakes.add([phone(), ipad()], t0 + s(10));
        wakes.add([phone()], t0 + s(20));
        assert_eq!(wakes.pending.len(), 2);

        // Quiet is measured from each device's last change.
        assert!(wakes.take_due(t0 + s(39)).is_empty());
        assert_eq!(wakes.take_due(t0 + s(40)), [ipad()]);
        assert_eq!(wakes.next(), Some(t0 + s(50)));
        assert_eq!(wakes.take_due(t0 + s(50)), [phone()]);
        assert_eq!(wakes.next(), None);
    }

    #[test]
    fn a_library_that_never_goes_quiet_still_wakes_devices() {
        let t0 = std::time::Instant::now();
        let phone = || ("dev-1".to_string(), "j".to_string());
        let mut wakes = Wakes {
            pending: Vec::new(),
            timer: false,
        };
        let mut sent = 0;
        for i in 0..40 {
            let now = t0 + std::time::Duration::from_secs(i * 10);
            sent += wakes.take_due(now).len();
            wakes.add([phone()], now);
        }
        // Six and a half minutes of changes every ten seconds: one push at
        // the five-minute mark, and one pending.
        assert_eq!(sent, 1);
        assert_eq!(wakes.pending.len(), 1);
    }

    #[test]
    fn the_device_playing_is_the_one_meant() {
        let reg = Registry::default();
        let (tx1, _rx1) = tokio::sync::mpsc::unbounded_channel();
        let (tx2, mut rx2) = tokio::sync::mpsc::unbounded_channel();
        let mac = reg.register("j", "mac", "macos", "dev-1", tx1, false);
        let phone = reg.register("j", "phone", "ios", "dev-2", tx2, false);

        // Two idle devices: no telling, so the caller is told to ask.
        let err = reg.send(Some("j"), None, LinkCommand::Pause).unwrap_err();
        assert!(err.contains("mac") && err.contains("phone"), "{err}");

        reg.report(
            &phone,
            LinkState {
                playing: true,
                ..Default::default()
            },
        );
        assert_eq!(
            reg.send(Some("j"), None, LinkCommand::Pause).unwrap().id,
            phone
        );
        assert_eq!(rx2.try_recv().unwrap(), LinkCommand::Pause);

        // Stopped a moment ago: still the one meant, over the Mac.
        reg.report(&phone, LinkState::default());
        assert_eq!(
            reg.send(Some("j"), None, LinkCommand::Pause).unwrap().id,
            phone
        );
        let _ = mac;
    }

    fn drain(rx: &mut tokio::sync::mpsc::UnboundedReceiver<LinkCommand>) -> Vec<LinkCommand> {
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    }

    /// `j` shares the phone with `k`, not the Mac.
    fn shared() -> (
        Registry,
        tokio::sync::mpsc::UnboundedReceiver<LinkCommand>,
        tokio::sync::mpsc::UnboundedReceiver<LinkCommand>,
        tokio::sync::mpsc::UnboundedReceiver<LinkCommand>,
    ) {
        let reg = Registry::default();
        let (phone_tx, mut phone) = tokio::sync::mpsc::unbounded_channel();
        let (mac_tx, mac) = tokio::sync::mpsc::unbounded_channel();
        let (k_tx, k) = tokio::sync::mpsc::unbounded_channel();
        let id = reg.register("j", "phone", "ios", "dev-phone", phone_tx, true);
        reg.register("j", "mac", "macos", "dev-mac", mac_tx, true);
        reg.register("k", "laptop", "macos", "dev-k", k_tx, true);
        reg.report(
            &id,
            LinkState {
                playing: true,
                title: Some("Roygbiv".into()),
                outputs: Some(Default::default()),
                ..Default::default()
            },
        );
        reg.share("j", "dev-phone", "k", true).unwrap();
        drain(&mut phone);
        (reg, phone, mac, k)
    }

    #[test]
    fn a_grantee_sees_the_shared_device_and_nothing_else_of_the_owner() {
        let (_reg, _phone, _mac, mut k) = shared();
        let listed = drain(&mut k)
            .into_iter()
            .rev()
            .find_map(|c| match c {
                LinkCommand::Devices { devices } => Some(devices),
                _ => None,
            })
            .expect("told of it");
        assert_eq!(listed.len(), 1, "the phone, not the Mac");
        let phone = &listed[0];
        assert_eq!(phone.id, "dev-phone");
        assert_eq!(phone.owner.as_deref(), Some("j"));
        let state = phone.state.as_ref().expect("its state");
        assert_eq!(state.title.as_deref(), Some("Roygbiv"));
        assert!(
            state.outputs.is_some(),
            "outputs, to choose from: the output is in the playback set"
        );
    }

    /// A granted account (`k`, say read-only) controlling the owner's (`j`,
    /// say admin) phone gets the playback set and nothing of `j`'s account:
    /// what touches the library is refused, and every command arrives marked
    /// as `k`'s, so the phone runs it with no power to sync, and no command
    /// in the set writes favourites, playlists or history.
    #[test]
    fn a_grantee_controls_playback_as_itself_and_nothing_of_the_owners_account() {
        let (reg, mut phone, mut mac, _k) = shared();
        let wrapped = |c: LinkCommand| LinkCommand::Shared {
            command: Box::new(c),
        };
        for cmd in [
            LinkCommand::Pause,
            LinkCommand::SetRendererVolume { volume: 40 },
            LinkCommand::SleepTimer {
                timer: Some(koan_core::player::state::SleepTimer::After { minutes: 30 }),
            },
            LinkCommand::HandOff { to: "dev-k".into() },
        ] {
            reg.relay("k", "dev-phone", cmd.clone()).unwrap();
            assert_eq!(drain(&mut phone), [wrapped(cmd)]);
        }
        for cmd in [
            LinkCommand::Sync { full: false },
            LinkCommand::Evict { track_ids: vec![] },
            LinkCommand::Shared {
                command: Box::new(LinkCommand::Pause),
            },
        ] {
            assert!(!cmd.allowed_playback());
            assert!(reg.relay("k", "dev-phone", cmd).is_err());
        }
        assert!(drain(&mut phone).is_empty());
        assert!(
            reg.relay("k", "dev-mac", LinkCommand::Pause).is_err(),
            "not shared, not reachable"
        );
        assert!(
            drain(&mut mac)
                .iter()
                .all(|c| matches!(c, LinkCommand::Devices { .. } | LinkCommand::Shares { .. })),
            "news, and no command"
        );
        assert!(reg.shared_owner("k", "dev-mac").is_none(), "nor wakeable");
    }

    /// "Move here" both ways: the grantee pulls the shared phone's music to
    /// its own device, which the phone sends as a hand-off; the grant lets
    /// that one command run backwards and nothing else.
    #[test]
    fn a_hand_off_runs_both_ways_across_a_grant_and_nothing_else_does() {
        let (reg, _phone, _mac, mut k) = shared();
        drain(&mut k);
        let play = LinkCommand::Play {
            track_ids: vec!["t".into()],
            start_at: 0,
            position_ms: 1000,
            paused: false,
            handoff: true,
        };
        reg.relay_from("j", Some("dev-phone"), "dev-k", play.clone())
            .unwrap();
        assert!(drain(&mut k).contains(&LinkCommand::Shared {
            command: Box::new(play.clone())
        }));
        assert!(
            reg.relay_from("j", Some("dev-phone"), "dev-k", LinkCommand::Pause)
                .is_err(),
            "the owner's phone does not command the grantee's devices"
        );
        assert!(
            reg.relay_from("j", Some("dev-mac"), "dev-k", play.clone())
                .is_err(),
            "only the shared device"
        );
        let not_a_hand_off = LinkCommand::Play {
            track_ids: vec!["t".into()],
            start_at: 0,
            position_ms: 0,
            paused: false,
            handoff: false,
        };
        assert!(
            reg.relay_from("j", Some("dev-phone"), "dev-k", not_a_hand_off)
                .is_err(),
            "a hand-off, not any play"
        );
    }

    #[test]
    fn revoking_ends_control_at_once() {
        let (reg, mut phone, _mac, mut k) = shared();
        reg.share("j", "dev-phone", "k", false).unwrap();
        assert!(reg.relay("k", "dev-phone", LinkCommand::Pause).is_err());
        assert!(
            drain(&mut phone)
                .iter()
                .all(|c| !matches!(c, LinkCommand::Pause))
        );
        assert!(reg.shared_owner("k", "dev-phone").is_none());
        let listed = drain(&mut k)
            .into_iter()
            .rev()
            .find_map(|c| match c {
                LinkCommand::Devices { devices } => Some(devices),
                _ => None,
            })
            .expect("told it is gone");
        assert!(listed.is_empty());
    }

    #[test]
    fn a_device_hears_whom_it_is_shared_with_every_time_it_links() {
        let reg = Registry::default();
        let (tx, mut phone) = tokio::sync::mpsc::unbounded_channel();
        reg.register("j", "phone", "ios", "dev-phone", tx, true);
        let first = drain(&mut phone);
        assert!(
            first.contains(&LinkCommand::Shares {
                grantees: vec![],
                error: None,
                accounts: vec![],
            }),
            "an empty list replaces one from another server"
        );
        reg.send_shares(
            "j",
            "dev-phone",
            Some("There is no account called x".into()),
        );
        assert!(drain(&mut phone).iter().any(
            |c| matches!(c, LinkCommand::Shares { error: Some(e), .. } if e.contains("no account"))
        ));
    }

    #[test]
    fn the_owner_is_told_who_it_shares_with() {
        let reg = Registry::default();
        let (tx, mut phone) = tokio::sync::mpsc::unbounded_channel();
        reg.register("j", "phone", "ios", "dev-phone", tx, true);
        reg.share("j", "dev-phone", "k", true).unwrap();
        reg.share("j", "dev-phone", "m", true).unwrap();
        let last = drain(&mut phone).into_iter().rev().find_map(|c| match c {
            LinkCommand::Shares { grantees, .. } => Some(grantees),
            _ => None,
        });
        assert_eq!(last, Some(vec!["k".to_string(), "m".to_string()]));
        assert!(
            reg.share("j", "dev-phone", "j", true).is_err(),
            "not with itself"
        );
    }

    /// Whose token wakes a device: another account's, behind the same router
    /// as the asker; a shared one's owner from anywhere; and otherwise only
    /// the asker's own, so a device elsewhere of another account is refused.
    #[test]
    fn a_device_is_woken_for_another_account_on_its_network_or_by_grant() {
        let reg = Registry::default();
        let home: std::net::IpAddr = "203.0.113.7".parse().unwrap();
        let away: std::net::IpAddr = "198.51.100.2".parse().unwrap();
        reg.seen_at("dev-ipad", "sarita", home);
        reg.seen_at("dev-phone", "admin", home);
        assert_eq!(
            reg.wake_owner("admin", "dev-phone", "dev-ipad"),
            "sarita",
            "same address: the iPad's own token"
        );
        reg.seen_at("dev-phone", "admin", away);
        assert_eq!(
            reg.wake_owner("admin", "dev-phone", "dev-ipad"),
            "admin",
            "elsewhere, with no grant: only admin's own, which it is not"
        );
        reg.share("sarita", "dev-ipad", "admin", true).unwrap();
        assert_eq!(
            reg.wake_owner("admin", "dev-phone", "dev-ipad"),
            "sarita",
            "shared: from anywhere"
        );
        // An address is believed only for the account that linked from it.
        reg.seen_at("dev-other", "mallory", home);
        assert_eq!(reg.wake_owner("admin", "dev-other", "dev-tv"), "admin");
    }

    /// The server restarts with every release; a backgrounded iPad has to
    /// stay wakeable across one without being opened again.
    #[test]
    fn where_a_device_last_linked_from_survives_a_restart() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        koan_core::db::schema::create_tables(&conn).unwrap();
        for (device, user, seen) in [("dev-ipad", "sarita", 1), ("dev-phone", "admin", 2)] {
            conn.execute(
                "INSERT INTO link_devices (device, username, name, platform, last_seen) VALUES (?1, ?2, ?1, 'ios', ?3)",
                rusqlite::params![device, user, seen],
            )
            .unwrap();
        }
        let home: std::net::IpAddr = "203.0.113.7".parse().unwrap();
        outbox::save_address_in(&conn, "dev-ipad", "sarita", home);
        outbox::save_address_in(&conn, "dev-phone", "admin", home);

        // A fresh server, from what was saved.
        let reg = Registry::default();
        *reg.addresses.lock() = outbox::load_addresses_in(&conn);
        assert_eq!(
            reg.wake_owner("admin", "dev-phone", "dev-ipad"),
            "sarita",
            "still woken through its own account after the restart"
        );
    }

    /// Forgetting a device tells the account's linked devices to drop it,
    /// and no other account's; a linked device is not forgotten, since it
    /// would be back at once.
    #[test]
    fn a_forgotten_device_is_dropped_by_the_accounts_devices() {
        let reg = Registry::default();
        let (mac_tx, mut mac) = tokio::sync::mpsc::unbounded_channel();
        let (other_tx, mut other) = tokio::sync::mpsc::unbounded_channel();
        reg.register("j", "mac", "macos", "dev-mac", mac_tx, true);
        reg.register("k", "laptop", "macos", "dev-k", other_tx, true);
        while mac.try_recv().is_ok() {}
        while other.try_recv().is_ok() {}

        reg.forget("j", "dev-phone").unwrap();
        let heard: Vec<LinkCommand> = std::iter::from_fn(|| mac.try_recv().ok()).collect();
        assert!(heard.contains(&LinkCommand::Forgotten {
            device: "dev-phone".into()
        }));
        assert!(
            std::iter::from_fn(|| other.try_recv().ok())
                .all(|c| !matches!(c, LinkCommand::Forgotten { .. })),
            "another account hears nothing of it"
        );
        assert!(
            reg.forget("j", "dev-mac").is_err(),
            "linked: it would be back"
        );
        assert!(
            reg.relay(
                "j",
                "dev-mac",
                LinkCommand::Forgotten { device: "x".into() }
            )
            .is_err(),
            "news from the server, not a command a device may send"
        );
    }

    /// A device shared by another account is forgotten by declining the
    /// share: its owner keeps it and is told, and it is not listed again.
    #[test]
    fn forgetting_a_shared_device_declines_the_share() {
        let (reg, mut phone, _mac, mut k) = shared();
        drain(&mut k);
        reg.forget("k", "dev-phone").unwrap();
        assert!(reg.shared_owner("k", "dev-phone").is_none());
        assert!(
            drain(&mut phone)
                .iter()
                .any(|c| matches!(c, LinkCommand::Shares { grantees, .. } if grantees.is_empty()))
        );
        let listed = drain(&mut k).into_iter().rev().find_map(|c| match c {
            LinkCommand::Devices { devices } => Some(devices),
            _ => None,
        });
        assert_eq!(listed.map(|d| d.len()), Some(0));
    }

    /// The server forgets a device's push token with it, so nothing is
    /// pushed to it again; and only the account that owns it can.
    #[test]
    fn forgetting_a_device_stops_pushes_to_it_and_only_for_its_account() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        koan_core::db::schema::create_tables(&conn).unwrap();
        for user in ["j", "k"] {
            conn.execute(
                "INSERT INTO link_devices (device, username, name, platform, last_seen) VALUES ('dev-phone', ?1, 'phone', 'ios', 1)",
                [user],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO link_push (device, username, token, sandbox, updated_at) VALUES ('dev-phone', ?1, 't', 0, 1)",
                [user],
            )
            .unwrap();
        }
        outbox::forget_device_in(&conn, "dev-phone", "k");
        assert_eq!(
            outbox::push_targets_in(&conn, Some("j")).len(),
            1,
            "k forgetting its own leaves j's alone"
        );
        outbox::forget_device_in(&conn, "dev-phone", "j");
        assert!(outbox::push_targets_in(&conn, Some("j")).is_empty());
        assert!(outbox::push_targets_in(&conn, None).is_empty());
    }

    #[test]
    fn a_device_hears_its_peers_and_commands_reach_them_by_device_id() {
        let reg = Registry::default();
        let (tx1, mut rx1) = tokio::sync::mpsc::unbounded_channel();
        let (tx2, mut rx2) = tokio::sync::mpsc::unbounded_channel();
        let (tx3, mut rx3) = tokio::sync::mpsc::unbounded_channel();
        reg.register("j", "mac", "macos", "dev-mac", tx1, true);
        let phone = reg.register("j", "phone", "ios", "dev-phone", tx2, true);
        reg.register("someone", "laptop", "macos", "dev-other", tx3, true);

        reg.report(
            &phone,
            LinkState {
                playing: true,
                title: Some("Roygbiv".into()),
                ..Default::default()
            },
        );
        let devices = drain(&mut rx1)
            .into_iter()
            .rev()
            .find_map(|c| match c {
                LinkCommand::Devices { devices } => Some(devices),
                _ => None,
            })
            .expect("the Mac is told of the phone");
        assert_eq!(devices.len(), 1, "not itself, not another account's");
        assert_eq!(devices[0].id, "dev-phone");
        assert_eq!(
            devices[0].state.as_ref().and_then(|s| s.title.as_deref()),
            Some("Roygbiv")
        );
        while rx3.try_recv().is_ok() {}
        assert!(rx3.try_recv().is_err());

        while rx2.try_recv().is_ok() {}
        reg.relay("j", "dev-phone", LinkCommand::Pause).unwrap();
        assert_eq!(rx2.try_recv().unwrap(), LinkCommand::Pause);
        assert!(
            reg.relay("someone", "dev-phone", LinkCommand::Pause)
                .is_err(),
            "another account cannot reach it"
        );
        assert!(
            reg.relay("j", "dev-phone", LinkCommand::Devices { devices: vec![] })
                .is_err()
        );
    }

    #[test]
    fn a_device_linked_for_a_month_is_not_forgotten() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        koan_core::db::schema::create_tables(&conn).unwrap();
        let now = 100 * 24 * 60 * 60;
        let long_ago = now - 40 * 24 * 60 * 60;
        for device in ["mac", "old-phone"] {
            conn.execute(
                "INSERT INTO link_devices (device, username, name, platform, last_seen) VALUES (?1, 'j', ?1, 'ios', ?2)",
                rusqlite::params![device, long_ago],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO link_push (device, username, token, sandbox, updated_at) VALUES (?1, 'j', 't', 0, ?2)",
                rusqlite::params![device, long_ago],
            )
            .unwrap();
        }
        outbox::forget_stale(&conn, &[("mac".into(), "j".into())], now);
        let left = |table: &str| -> Vec<String> {
            conn.prepare(&format!("SELECT device FROM {table} ORDER BY device"))
                .unwrap()
                .query_map([], |r| r.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        assert_eq!(left("link_devices"), ["mac"]);
        assert_eq!(left("link_push"), ["mac"]);
    }
}
