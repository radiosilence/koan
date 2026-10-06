//! The devices this one can play on, and which of them it is controlling.
//!
//! Two sources: the account's other devices, which the server sends down the
//! link, and whatever answers on the local network (`remote::nearby`), which
//! may belong to anyone. A device found both ways is one device, reached over
//! the local network while that connection is up: it is the shorter path.
//!
//! The device being controlled and the account's devices as last heard of are
//! kept on disk, so an app iOS suspended or killed opens still controlling the
//! same device, with the list drawn at once rather than after the link is back.
//!
//! A device is listed while it is present: linked, or on the network. One
//! that stops answering is not dropped on the spot. It stays as it was for
//! one full heartbeat past when it should have been heard from, so a single
//! missed signal changes nothing. Then, if this device has used it (controlled
//! it, or sent it music or a command), it is listed asleep, with when it was
//! last seen, until it is heard from again or forgotten (`forget`); one never
//! used is gone, since an asleep device nobody here plays on is only clutter.
//! Asleep, it can be chosen only if a push can wake it. A device heard from
//! again at any point is back at once, forgotten or not.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};

use crate::config::Config;
use crate::remote::acks;
use crate::remote::link::{
    self, CommandSource, LinkCommand, LinkDevice, LinkHello, LinkReport, LinkState, Local,
};

/// Another device, as the app shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub platform: String,
    /// Signed in to the same account on the same server.
    pub account: bool,
    /// Whose it is, for a device another account on the server shares with
    /// this one: playback and the queue only, as on the local network.
    pub owner: Option<String>,
    /// Connected to over the local network now.
    pub nearby: bool,
    /// Reachable at once. An account device that is not is one iOS has
    /// suspended: a command wakes it, and music reaches it as a notification.
    pub awake: bool,
    /// Not reachable for longer than a heartbeat. Not reachable for less, it
    /// is neither awake nor asleep: reconnecting, as far as anyone can tell.
    pub asleep: bool,
    /// Can be woken from asleep: the server holds a push token for it.
    /// An asleep device that cannot be is shown, and cannot be chosen.
    pub wakeable: bool,
    /// Unix seconds when it was last reachable; `None` if never, here.
    pub last_seen: Option<i64>,
    /// How far waking it has got, while it is being woken or once it failed.
    pub waking: Option<Waking>,
    /// Plays from the same library, so music can be handed between the two.
    pub same_library: bool,
    /// What it last reported.
    pub state: Option<LinkState>,
    /// When `state` was heard.
    pub heard: Instant,
    /// Why it cannot be reached, when found but not connected: shown rather
    /// than leaving the device out, so the picker says what is wrong.
    pub problem: Option<String>,
}

impl Device {
    /// Where its playhead is now, from where it was reported to be.
    pub fn position_ms(&self) -> u64 {
        let Some(state) = &self.state else { return 0 };
        if !state.playing {
            return state.position_ms;
        }
        let pos = state.position_ms + self.heard.elapsed().as_millis() as u64;
        if state.duration_ms > 0 {
            pos.min(state.duration_ms)
        } else {
            pos
        }
    }
}

#[derive(Default)]
struct Store {
    linked: bool,
    account: Vec<(LinkDevice, Instant)>,
    nearby: Vec<Nearby>,
    seen: Vec<SeenNearby>,
    target: Option<Remembered>,
    version: u64,
    /// When each device was last reachable, in Unix seconds.
    live: HashMap<String, i64>,
    /// Devices no list carries any more, kept until they are forgotten.
    departed: Vec<Departed>,
    /// The devices this one has used, by id: listed asleep when out of reach.
    used: HashSet<String>,
    /// `account` came down the link that is up now. One held from before the
    /// link last dropped says nothing about who is reachable at this moment.
    fresh: bool,
    /// Devices being woken, or that did not wake, by id.
    waking: HashMap<String, Waking>,
    /// When each device was last heard from on the local network, in Unix
    /// seconds: any message, and the moment it disconnected.
    lan_heard: HashMap<String, i64>,
    /// The wake under way, as its generation and the device: one at a time.
    attempt: Option<(u64, String)>,
    /// Bumped by every wake started or abandoned.
    wake_gen: u64,
    /// The accounts this device lets control it, as the server says.
    shares: Vec<String>,
    /// Why the server refused the last change to them.
    share_error: Option<String>,
    /// Every account on the server, to share with.
    accounts: Vec<String>,
    /// The last command that did not simply arrive, for the app to say so.
    notice: Option<Notice>,
}

/// A command that was queued for a device asleep, or did not reach one: what
/// the app tells the person, since the device's state cannot show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// Counts up, so the same notice twice is still news.
    pub seq: u64,
    /// The device's name, as listed.
    pub device: String,
    pub outcome: acks::AckOutcome,
}

/// The last command that did not simply arrive.
pub fn notice() -> Option<Notice> {
    with(|s| s.notice.clone())
}

/// What to do with a command's outcome once it is known: `None` when it was
/// sent by a way that brings no answer.
pub type Then = Box<dyn FnOnce(Option<acks::AckOutcome>) + Send>;

/// A stage of waking a device, in the order they are tried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Waking {
    /// Dialling it on the local network: seen there a moment ago, it may not
    /// be suspended yet.
    Network,
    /// A background push sent through the server. iOS delays or drops these
    /// as it sees fit, and never delivers one to an app swiped away.
    Push,
    /// A notification on the device, asking to be tapped.
    Notification,
    /// None of it worked; why, for the device's row.
    Failed(String),
}

/// A device that dropped out of every list: as it was when last reachable.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Departed {
    id: String,
    name: String,
    platform: String,
    account: bool,
    same_library: bool,
    /// Another account's, shared with this one.
    shared: bool,
}

/// One heartbeat: a link that hears nothing for this long pings, and one that
/// still hears nothing is gone. A device is asleep only after a full one has
/// passed without it.
pub const STALE_SECS: i64 = super::wire::IDLE.as_secs() as i64;

/// What is kept on disk. See the module note.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct Saved {
    target: Option<Remembered>,
    account: Vec<LinkDevice>,
    #[serde(default)]
    nearby: Vec<SeenNearby>,
    /// When each device was last reachable, Unix seconds: kept so a device
    /// the server no longer lists is not taken for one seen just now.
    #[serde(default)]
    live: HashMap<String, i64>,
    /// Devices no list carries any more, kept until forgotten.
    #[serde(default)]
    departed: Vec<Departed>,
    /// The devices this one has used. Absent from a file written before it
    /// was kept, when only the device being controlled counts.
    #[serde(default)]
    used: Option<Vec<String>>,
}

/// A device reached on the local network, and where: dialled there at once
/// on the next run, before Bonjour has announced anything.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SeenNearby {
    pub id: String,
    pub name: String,
    pub platform: String,
    pub addr: String,
    /// Unix seconds.
    pub at: i64,
}

/// A device not reached on the network in this long is forgotten.
const NEARBY_KEPT: i64 = 7 * 24 * 60 * 60;

/// The device being controlled, named so that it can be shown while it is
/// out of reach.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct Remembered {
    id: String,
    name: String,
    platform: String,
}

struct Nearby {
    hello: LinkHello,
    state: Option<LinkState>,
    at: Instant,
}

static STORE: Mutex<Option<Store>> = Mutex::new(None);
static LOCAL: OnceLock<Local> = OnceLock::new();

fn with<R>(f: impl FnOnce(&mut Store) -> R) -> R {
    let mut store = STORE.lock();
    f(store.get_or_insert_with(Store::default))
}

/// `f` changed something the app shows.
fn changed<R>(f: impl FnOnce(&mut Store) -> R) -> R {
    let out = with(|s| {
        let out = f(s);
        s.version += 1;
        out
    });
    crate::signal::engine_changed().bump();
    replan();
    out
}

/// Rung when the store changes, so the clock looks again at when the list
/// next changes on its own.
static CLOCK: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());

/// A device going asleep, or past its grace period, changes the list with
/// nothing having happened. One thread sleeps until the next such moment and
/// announces it; it is woken early whenever the store changes.
fn replan() {
    static STARTED: OnceLock<()> = OnceLock::new();
    *CLOCK.0.lock() = true;
    CLOCK.1.notify_all();
    STARTED.get_or_init(|| {
        let _ = std::thread::Builder::new()
            .name("koan-devices-clock".into())
            .spawn(clock);
    });
}

fn clock() {
    loop {
        let now = chrono::Utc::now().timestamp();
        let wait = next_change(now).map(|at| Duration::from_secs((at - now).max(0) as u64 + 1));
        let mut rung = CLOCK.0.lock();
        if !*rung {
            let timed_out = match wait {
                Some(wait) => CLOCK.1.wait_for(&mut rung, wait).timed_out(),
                None => {
                    CLOCK.1.wait(&mut rung);
                    false
                }
            };
            if timed_out {
                drop(rung);
                // Not through `changed`, which would ring the clock again.
                with(|s| s.version += 1);
                crate::signal::engine_changed().bump();
                continue;
            }
        }
        *rung = false;
    }
}

impl Store {
    fn save(&self) {
        let saved = Saved {
            target: self.target.clone(),
            account: self.account.iter().map(|(d, _)| d.clone()).collect(),
            nearby: self.seen.clone(),
            live: self.live.clone(),
            departed: self.departed.clone(),
            used: Some({
                let mut used: Vec<String> = self.used.iter().cloned().collect();
                used.sort();
                used
            }),
        };
        if let Ok(json) = serde_json::to_string(&saved) {
            let _ = std::fs::write(saved_path(), json);
        }
    }
}

fn saved_path() -> std::path::PathBuf {
    crate::config::config_dir().join("devices.json")
}

/// The target and account devices as the last run left them.
fn restore() {
    let Ok(text) = std::fs::read_to_string(saved_path()) else {
        return;
    };
    let Ok(saved) = serde_json::from_str::<Saved>(&text) else {
        return;
    };
    let now = Instant::now();
    let cutoff = chrono::Utc::now().timestamp() - NEARBY_KEPT;
    changed(|s| {
        s.target = saved.target;
        s.seen = saved
            .nearby
            .into_iter()
            .filter(|n| n.at >= cutoff)
            .collect();
        s.live = saved.live;
        s.used = match saved.used {
            Some(used) => used.into_iter().collect(),
            None => s.target.iter().map(|t| t.id.clone()).collect(),
        };
        let used = &s.used;
        s.departed = saved
            .departed
            .into_iter()
            .filter(|g| used.contains(&g.id))
            .collect();
        // Not linked until the server says so, and doing nothing anyone
        // knows of: what it was playing then is not what it is playing now.
        s.account = saved
            .account
            .into_iter()
            .map(|d| {
                (
                    LinkDevice {
                        linked: false,
                        state: None,
                        ..d
                    },
                    now,
                )
            })
            .collect();
    });
}

/// Link to the server and open this device to the local network, serving
/// `local` to both. Once per process.
pub fn start(local: Local) {
    if LOCAL.set(local.clone()).is_err() {
        return;
    }
    restore();
    link::spawn(local.clone());
    crate::remote::nearby::start(local);
}

/// This device, once started.
pub fn local() -> Option<&'static Local> {
    LOCAL.get()
}

/// The account's devices stay listed while the link is down, as they were
/// last heard of: iOS drops the link each time it suspends the app, and a
/// phone controlling a Mac should still be when it wakes. The server sends
/// them afresh when the link is back.
///
/// Nothing here lets go of the target. A device that drops out of every list
/// (a Mac asleep, a server restarting) is still the one the person picked,
/// and the app shows it as out of reach until it is back or another is.
pub fn set_linked(linked: bool) {
    crate::remote::offline::link_changed(linked);
    changed(|s| {
        s.linked = linked;
        if !linked {
            s.fresh = false;
        }
    });
}

pub fn set_account(devices: Vec<LinkDevice>) {
    let now = Instant::now();
    let unix = chrono::Utc::now().timestamp();
    changed(|s| {
        // Reachable up to this message, if the list saying so came down this
        // same link: the hysteresis runs from here, not from whenever it was
        // last reported. A list held across a gap in the link proves nothing.
        if s.fresh {
            for (old, _) in &s.account {
                if old.linked {
                    s.live.insert(old.id.clone(), unix);
                }
            }
        }
        for d in &devices {
            if d.linked {
                s.live.insert(d.id.clone(), unix);
                s.waking.remove(&d.id);
            } else if let Some(seen) = d.last_seen {
                // Whichever is later: this device may have reached it over
                // the network since.
                let live = s.live.entry(d.id.clone()).or_insert(seen);
                *live = (*live).max(seen);
            }
            s.departed.retain(|g| g.id != d.id);
        }
        let leaving: Vec<Departed> = s
            .account
            .iter()
            .filter(|(old, _)| !devices.iter().any(|d| d.id == old.id))
            .map(|(old, _)| Departed {
                id: old.id.clone(),
                name: old.name.clone(),
                platform: old.platform.clone(),
                account: old.owner.is_none(),
                shared: old.owner.is_some(),
                same_library: true,
            })
            .collect();
        for d in leaving {
            // Never seen reachable as far as this device knows: gone, rather
            // than taken for a device seen just now. And a device another
            // account stops sharing is gone at once, not asleep.
            if !s.live.contains_key(&d.id) || d.shared {
                continue;
            }
            s.departed.retain(|g| g.id != d.id);
            s.departed.push(d);
        }
        s.account = devices.into_iter().map(|d| (d, now)).collect();
        s.fresh = s.linked;
        s.save();
    });
}

/// The accounts this device is shared with, as the server last said, and
/// why it refused the last change, if it did.
pub fn set_shares(grantees: Vec<String>, error: Option<String>, accounts: Vec<String>) {
    changed(|s| {
        s.shares = grantees;
        s.share_error = error;
        s.accounts = accounts;
    });
}

/// The server's accounts, to share this device with.
pub fn accounts() -> Vec<String> {
    with(|s| s.accounts.clone())
}

pub fn shares() -> Vec<String> {
    with(|s| s.shares.clone())
}

pub fn share_error() -> Option<String> {
    with(|s| s.share_error.clone())
}

/// Signed out, or signed in elsewhere: the shares were that server's.
pub fn forget_shares() {
    set_shares(Vec::new(), None, Vec::new());
}

/// Send `cmd` to `id` because `source` asked this device to: a hand-off.
///
/// The account's own request goes anywhere. Anyone else's acts as the asker,
/// never with this account's powers, so it does not reach this account's own
/// devices up its link: a stranger's (Playback only) goes over the network or
/// nowhere; a sharing account's, or a trusted network device's, goes over the
/// network or to a device that is not this account's (the asker's own,
/// through the server).
pub fn send_for(source: CommandSource, id: &str, cmd: LinkCommand) -> Result<(), String> {
    send_for_then(source, id, cmd, None)
}

/// `send_for`, handing the outcome to `then` rather than to the app's notice.
pub fn send_for_then(
    source: CommandSource,
    id: &str,
    cmd: LinkCommand,
    then: Option<Then>,
) -> Result<(), String> {
    match source {
        CommandSource::Account => send_then(id, cmd, then),
        CommandSource::Stranger => {
            send_nearby(id, cmd)?;
            if let Some(then) = then {
                then(None);
            }
            Ok(())
        }
        CommandSource::Shared | CommandSource::Nearby => {
            let (nearby, own) = with(|s| {
                (
                    s.nearby.iter().any(|n| n.hello.id == id),
                    s.account
                        .iter()
                        .any(|(d, _)| d.id == id && d.owner.is_none()),
                )
            });
            if own && !nearby {
                return Err(format!("{id} is this account's, not the asker's"));
            }
            send_then(id, cmd, then)
        }
    }
}

/// Send `cmd` to `id` over the local network only, never up this device's
/// link. For what a stranger asked of this device: over the network it can
/// only reach what the stranger could have reached itself; over the link it
/// would act as this device's account.
pub fn send_nearby(id: &str, cmd: LinkCommand) -> Result<(), String> {
    let nearby = with(|s| s.nearby.iter().any(|n| n.hello.id == id));
    if nearby && cmd.allowed_nearby() && crate::remote::nearby::send(id, cmd) {
        Ok(())
    } else {
        Err(format!("{id} is not on this network"))
    }
}

/// Let the account `grantee` on this server control this device, or stop.
/// The server answers with the list as it now stands.
pub fn share(grantee: &str, allow: bool) -> Result<(), String> {
    let grantee = grantee.trim();
    if grantee.is_empty() {
        return Err("Name an account on your server.".into());
    }
    if link::report(LinkReport::Share {
        grantee: grantee.to_string(),
        allow,
    }) {
        Ok(())
    } else {
        Err("Not connected to your server.".into())
    }
}

/// The devices reached on this network before, to dial first.
pub fn remembered_nearby() -> Vec<SeenNearby> {
    with(|s| s.seen.clone())
}

pub fn nearby_hello(hello: LinkHello, addr: &str) {
    changed(|s| {
        s.seen.retain(|n| n.id != hello.id);
        s.seen.push(SeenNearby {
            id: hello.id.clone(),
            name: hello.name.clone(),
            platform: hello.platform.clone(),
            addr: addr.to_string(),
            at: chrono::Utc::now().timestamp(),
        });
        s.save();
        s.live
            .insert(hello.id.clone(), chrono::Utc::now().timestamp());
        s.departed.retain(|g| g.id != hello.id);
        s.waking.remove(&hello.id);
        s.lan_heard
            .insert(hello.id.clone(), chrono::Utc::now().timestamp());
        s.nearby.retain(|n| n.hello.id != hello.id);
        s.nearby.push(Nearby {
            hello,
            state: None,
            at: Instant::now(),
        });
    });
}

pub fn nearby_state(id: &str, state: LinkState) {
    changed(|s| {
        if let Some(n) = s.nearby.iter_mut().find(|n| n.hello.id == id) {
            n.state = Some(state);
            n.at = Instant::now();
            let now = chrono::Utc::now().timestamp();
            s.live.insert(id.to_string(), now);
            s.lan_heard.insert(id.to_string(), now);
        }
    });
}

pub fn nearby_gone(id: &str) {
    let cfg = Config::load().unwrap_or_default();
    let ours = link::library_fingerprint(&cfg);
    changed(|s| {
        let Some(at) = s.nearby.iter().position(|n| n.hello.id == id) else {
            return;
        };
        let gone = s.nearby.remove(at);
        let now = chrono::Utc::now().timestamp();
        s.live.insert(id.to_string(), now);
        s.lan_heard.insert(id.to_string(), now);
        // An account device is still listed by the server; only a stranger
        // has to be remembered here.
        if s.account.iter().any(|(d, _)| d.id == id) {
            return;
        }
        s.departed.retain(|g| g.id != id);
        s.departed.push(Departed {
            id: gone.hello.id.clone(),
            name: gone.hello.name.clone(),
            platform: gone.hello.platform.clone(),
            account: false,
            same_library: ours.is_some() && gone.hello.library == ours,
            shared: false,
        });
        s.save();
    });
}

/// Forget the device `id`: out of the list until it links or is heard on the
/// network again, which brings it straight back. An account device is
/// forgotten by the server too, push token and all, and every other device
/// of the account drops it. A device another account shares with this one
/// is forgotten by declining the share. Only a
/// device out of reach is forgotten: one reachable would be back at once.
pub fn forget(id: &str) -> Result<(), String> {
    let device = list().into_iter().find(|d| d.id == id);
    if device.as_ref().is_some_and(|d| d.awake) {
        return Err("Only a device out of reach can be forgotten.".into());
    }
    let account = with(|s| s.account.iter().any(|(d, _)| d.id == id));
    if account
        && !link::report(LinkReport::Forget {
            device: id.to_string(),
        })
    {
        return Err("Not connected to your server, which remembers it too.".into());
    }
    forgotten(id);
    Ok(())
}

/// This device sent `id` something: from now on it is kept, asleep, when it
/// is out of reach.
fn used(id: &str) {
    with(|s| {
        if s.used.insert(id.to_string()) {
            s.save();
        }
    });
}

/// Drop `id` from everything this device remembers of it: the server forgot
/// it, or this device was asked to.
pub fn forgotten(id: &str) {
    changed(|s| {
        s.used.remove(id);
        s.account.retain(|(d, _)| d.id != id);
        s.departed.retain(|g| g.id != id);
        s.seen.retain(|n| n.id != id);
        s.live.remove(id);
        s.lan_heard.remove(id);
        s.waking.remove(id);
        if s.target.as_ref().is_some_and(|t| t.id == id) {
            s.target = None;
        }
        s.save();
    });
    log::info!("devices: forgot {id}");
}

/// Something beside the list that the app shows with it has changed: the
/// server's profile, whether this device is listening.
pub fn touch() {
    changed(|_| ());
}

/// Bumped by every change to what `list` returns, and by `touch`.
pub fn version() -> u64 {
    with(|s| s.version)
}

/// Whether the link to the server is up.
pub fn linked() -> bool {
    with(|s| s.linked)
}

/// Every other device, those playing first.
pub fn list() -> Vec<Device> {
    list_at(chrono::Utc::now().timestamp())
}

/// `list` as of `now`, Unix seconds.
fn list_at(now: i64) -> Vec<Device> {
    let cfg = Config::load().unwrap_or_default();
    let ours = link::library_fingerprint(&cfg);
    with(|s| {
        let live = |id: &str| s.live.get(id).copied();
        // Missed for less than a heartbeat.
        let fresh = |id: &str| live(id).is_some_and(|at| now - at <= STALE_SECS);
        // Out of reach past the heartbeat, only a device used here is listed.
        let kept = |id: &str| fresh(id) || s.used.contains(id);
        let mut out: Vec<Device> = s
            .account
            .iter()
            .filter_map(|(d, at)| {
                let near = s.nearby.iter().find(|n| n.hello.id == d.id);
                let awake = d.linked || near.is_some();
                if !awake && !kept(&d.id) {
                    return None;
                }
                Some(Device {
                    id: d.id.clone(),
                    name: d.name.clone(),
                    platform: d.platform.clone(),
                    account: d.owner.is_none(),
                    owner: d.owner.clone(),
                    nearby: near.is_some(),
                    awake,
                    asleep: !awake && !fresh(&d.id),
                    // As the server says: a push token, and a push key to
                    // send with. One that says nothing lists only those.
                    wakeable: d.wakeable.unwrap_or(true),
                    last_seen: live(&d.id),
                    waking: None,
                    same_library: true,
                    // The network's state is fresher, but only the server's
                    // carries the outputs: the network is told none.
                    state: match (near.and_then(|n| n.state.clone()), &d.state) {
                        (Some(mut fresh), Some(linked)) => {
                            fresh.outputs = linked.outputs.clone();
                            Some(fresh)
                        }
                        (Some(fresh), None) => Some(fresh),
                        (None, linked) => linked.clone(),
                    },
                    heard: near.map_or(*at, |n| n.at),
                    problem: None,
                })
            })
            .collect();
        for n in &s.nearby {
            if out.iter().any(|d| d.id == n.hello.id) {
                continue;
            }
            out.push(Device {
                id: n.hello.id.clone(),
                name: n.hello.name.clone(),
                platform: n.hello.platform.clone(),
                account: false,
                owner: None,
                nearby: true,
                awake: true,
                asleep: false,
                wakeable: false,
                last_seen: live(&n.hello.id),
                waking: None,
                same_library: ours.is_some() && n.hello.library == ours,
                state: n.state.clone(),
                heard: n.at,
                problem: None,
            });
        }
        for g in &s.departed {
            if out.iter().any(|d| d.id == g.id) || !kept(&g.id) {
                continue;
            }
            let last = live(&g.id);
            // Nothing routes to it now, whatever the label: reconnecting for
            // a heartbeat, then asleep.
            out.push(Device {
                id: g.id.clone(),
                name: g.name.clone(),
                platform: g.platform.clone(),
                account: g.account,
                owner: None,
                nearby: false,
                awake: false,
                asleep: !fresh(&g.id),
                // A stranger met on this network may be woken by the server,
                // which does so for a device behind the same router as this
                // one; one of the account's that left the list has no token.
                wakeable: !g.account && s.linked,
                last_seen: last,
                waking: None,
                same_library: g.same_library,
                // What it was doing is not what it is doing now.
                state: None,
                heard: Instant::now(),
                problem: None,
            });
        }
        // A device announced on this network but not connected is left out:
        // it cannot be played on, and a phone that left or a stranger's app
        // that is not discoverable would sit in the list as a reason only.
        // The device being controlled, out of every list for now.
        if let Some(t) = &s.target
            && !out.iter().any(|d| d.id == t.id)
        {
            out.push(Device {
                id: t.id.clone(),
                name: t.name.clone(),
                platform: t.platform.clone(),
                account: false,
                owner: None,
                nearby: false,
                awake: false,
                asleep: true,
                wakeable: false,
                last_seen: live(&t.id),
                waking: None,
                same_library: true,
                state: None,
                heard: Instant::now(),
                problem: Some("Out of reach".into()),
            });
        }
        for d in &mut out {
            d.waking = s.waking.get(&d.id).cloned();
        }
        out.sort_by_key(|d| {
            (
                !d.state.as_ref().is_some_and(|st| st.playing),
                !d.awake,
                d.name.to_lowercase(),
            )
        });
        out
    })
}

/// When `list` next changes on its own: a device crossing into asleep.
fn next_change(now: i64) -> Option<i64> {
    with(|s| {
        let reachable = |id: &str| {
            s.nearby.iter().any(|n| n.hello.id == id)
                || s.account.iter().any(|(d, _)| d.id == id && d.linked)
        };
        s.live
            .iter()
            .filter(|(id, _)| !reachable(id))
            .map(|(_, at)| at + STALE_SECS)
            .filter(|t| *t > now)
            .min()
    })
}

/// Whether the app may choose `id`: not when nothing reaches it now and
/// nothing can wake it, where a command would go nowhere.
pub fn choosable(id: &str) -> Result<(), String> {
    match list().into_iter().find(|d| d.id == id) {
        Some(d) if !d.awake && !d.wakeable && d.problem.is_none() => Err(if d.asleep {
            format!("{} is asleep and cannot be woken from here.", d.name)
        } else {
            format!("{} is reconnecting.", d.name)
        }),
        _ => Ok(()),
    }
}

/// A device seen on the local network this recently is dialled there first.
const NETWORK_RECENT: i64 = 60;
/// How long each stage is given to bring the device in before the next.
const NETWORK_WAIT: Duration = Duration::from_secs(3);
const PUSH_WAIT: Duration = Duration::from_secs(6);
const NOTIFICATION_WAIT: Duration = Duration::from_secs(45);

/// The stages to try, each with how long to wait on it. A device that
/// nothing can wake through the server is only tried on the network.
fn wake_plan(seen_nearby_recently: bool, wakeable: bool) -> Vec<(Waking, Duration)> {
    let mut plan = Vec::new();
    if seen_nearby_recently {
        plan.push((Waking::Network, NETWORK_WAIT));
    }
    if wakeable {
        plan.push((Waking::Push, PUSH_WAIT));
        plan.push((Waking::Notification, NOTIFICATION_WAIT));
    }
    plan
}

/// How a wake ended.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Ended {
    Reached(Waking),
    Failed(String),
    /// Another device was chosen meanwhile: nothing more is sent.
    Cancelled,
}

/// Walk `plan`: `act` starts each stage, `wait` blocks until the device is
/// reachable, the stage's time is up or the wake is abandoned, saying whether
/// it was reached. `current` is false once another wake has replaced this one.
fn run_wake(
    plan: &[(Waking, Duration)],
    current: impl Fn() -> bool,
    mut act: impl FnMut(&Waking) -> Result<(), String>,
    mut wait: impl FnMut(Duration) -> bool,
) -> Ended {
    let mut last = None;
    for (stage, time) in plan {
        if !current() {
            return Ended::Cancelled;
        }
        if let Err(why) = act(stage) {
            return Ended::Failed(why);
        }
        last = Some(stage);
        if wait(*time) {
            return Ended::Reached(stage.clone());
        }
    }
    if !current() {
        return Ended::Cancelled;
    }
    Ended::Failed(match last {
        Some(Waking::Notification) => {
            "Did not wake. A notification is waiting on it to be tapped.".into()
        }
        _ => "Did not wake.".into(),
    })
}

fn reachable(id: &str) -> bool {
    with(|s| {
        s.nearby.iter().any(|n| n.hello.id == id)
            || s.account.iter().any(|(d, _)| d.id == id && d.linked)
    })
}

/// Heard from on the local network within `NETWORK_RECENT` of `now`.
fn recently_on_network(id: &str, now: i64) -> bool {
    with(|s| {
        s.lan_heard
            .get(id)
            .is_some_and(|at| now - at <= NETWORK_RECENT)
    })
}

/// Start a wake of `id`: its generation, or `None` when one of `id` is
/// already under way, which this then joins. Abandons any other.
fn begin_wake(id: &str) -> Option<u64> {
    with(|s| {
        if s.attempt.as_ref().is_some_and(|(_, d)| d == id) {
            return None;
        }
        s.wake_gen += 1;
        s.attempt = Some((s.wake_gen, id.to_string()));
        Some(s.wake_gen)
    })
}

fn wake_is_current(generation: u64) -> bool {
    with(|s| s.attempt.as_ref().is_some_and(|(g, _)| *g == generation))
}

/// Abandon the wake under way, unless it is of `keep`.
fn cancel_wake(keep: Option<&str>) {
    let cancelled = with(|s| {
        let other = s
            .attempt
            .as_ref()
            .is_some_and(|(_, d)| Some(d.as_str()) != keep);
        if other {
            let (_, id) = s.attempt.take().expect("checked");
            s.waking.remove(&id);
            s.wake_gen += 1;
        }
        other
    });
    if cancelled {
        // The abandoned wake's thread is waiting on this.
        crate::signal::engine_changed().bump();
    }
}

/// Wake `id`, which is not reachable, in the background: on the local network
/// if it was heard there in the last minute, then, when the server can wake
/// it, a background push and then a notification on it to tap. The device's
/// row says which stage it is at, and why it failed if it did. Each step is
/// logged with its time from the start, so a wake that did not happen shows
/// where it stopped.
///
/// One wake at a time: choosing another device abandons this one, so nothing
/// more reaches the device left behind, and choosing this one again joins it.
/// It does not wait for the device to be labelled asleep: a phone suspended
/// seconds ago is the one most worth waking.
pub fn wake(id: &str) {
    let id = id.to_string();
    if reachable(&id) {
        return;
    }
    let Some(device) = list().into_iter().find(|d| d.id == id) else {
        return;
    };
    let now = chrono::Utc::now().timestamp();
    // A stranger on the network has nothing to answer a push with.
    let plan = wake_plan(recently_on_network(&id, now), device.wakeable);
    if plan.is_empty() {
        return;
    }
    let Some(generation) = begin_wake(&id) else {
        log::info!("wake: {}: already being woken", device.name);
        return;
    };
    let name = device.name;
    let _ = std::thread::Builder::new()
        .name("koan-wake".into())
        .spawn(move || {
            let started = Instant::now();
            let ms = || started.elapsed().as_millis();
            let current = || wake_is_current(generation);
            let ended = run_wake(
                &plan,
                current,
                |stage| {
                    log::info!("wake: {name}: {stage:?} at +{}ms", ms());
                    changed(|s| s.waking.insert(id.clone(), stage.clone()));
                    match stage {
                        Waking::Network => {
                            crate::remote::nearby::dial_now();
                            Ok(())
                        }
                        Waking::Push | Waking::Notification => {
                            let notify = *stage == Waking::Notification;
                            if link::report(LinkReport::Wake {
                                to: id.clone(),
                                notify,
                            }) {
                                Ok(())
                            } else {
                                Err("Not connected to your server, which wakes it.".into())
                            }
                        }
                        Waking::Failed(_) => Ok(()),
                    }
                },
                |time| {
                    let until = Instant::now() + time;
                    let signal = crate::signal::engine_changed();
                    let mut seen = signal.generation();
                    while !reachable(&id) {
                        let left = until.saturating_duration_since(Instant::now());
                        if left.is_zero() || !current() {
                            return false;
                        }
                        seen = signal.wait_until(seen, left);
                    }
                    true
                },
            );
            match &ended {
                Ended::Reached(stage) => {
                    log::info!("wake: {name}: reached at +{}ms, after {stage:?}", ms());
                }
                Ended::Failed(why) => {
                    log::warn!("wake: {name}: did not wake in {}ms: {why}", ms());
                }
                Ended::Cancelled => {
                    log::info!("wake: {name}: abandoned at +{}ms for another device", ms());
                }
            }
            changed(|s| {
                if !s.attempt.as_ref().is_some_and(|(g, _)| *g == generation) {
                    return;
                }
                s.attempt = None;
                match ended {
                    Ended::Failed(why) => {
                        s.waking.insert(id.clone(), Waking::Failed(why));
                    }
                    _ => {
                        s.waking.remove(&id);
                    }
                }
            });
        });
}

/// The device the app is controlling; `None` for this one.
pub fn target() -> Option<String> {
    with(|s| s.target.as_ref().map(|t| t.id.clone()))
}

pub fn set_target(id: Option<String>) {
    cancel_wake(id.as_deref());
    let listed = id
        .as_ref()
        .and_then(|id| list().into_iter().find(|d| d.id == *id));
    changed(|s| {
        if let Some(id) = &id {
            s.used.insert(id.clone());
        }
        s.target = id.map(|id| match listed {
            Some(d) => Remembered {
                id,
                name: d.name,
                platform: d.platform,
            },
            None => Remembered {
                name: id.clone(),
                id,
                platform: String::new(),
            },
        });
        s.save();
    });
    // Controlling another device keeps a backgrounded app awake.
    crate::quiet::reapply();
}

/// The app is in front again after iOS may have suspended it: link now,
/// prove every connection is still alive, and look at the network afresh.
/// What makes the other devices appear the moment the app opens rather than
/// when a dead socket finally times out.
pub fn resume() {
    link::nudge();
    crate::remote::wire::probe_all();
    crate::remote::nearby::refresh();
}

/// Where the target's playhead is now and whether it is playing, from what
/// it last reported: the network's report when there is one, as `list`
/// prefers it. Cheap enough for every display frame, which `list` is not.
pub fn target_playhead() -> Option<(u64, bool)> {
    with(|s| {
        let (state, at) = reported(s, &s.target.as_ref()?.id)?;
        let mut position = state.position_ms;
        if state.playing {
            position += at.elapsed().as_millis() as u64;
            if state.duration_ms > 0 {
                position = position.min(state.duration_ms);
            }
        }
        Some((position, state.playing))
    })
}

/// What `id` last reported and when it was heard: the network's report when
/// there is one, as `list` prefers it.
fn reported<'s>(s: &'s Store, id: &str) -> Option<(&'s LinkState, Instant)> {
    s.nearby
        .iter()
        .find(|n| n.hello.id == id)
        .and_then(|n| Some((n.state.as_ref()?, n.at)))
        .or_else(|| {
            s.account
                .iter()
                .find(|(d, _)| d.id == id)
                .and_then(|(d, at)| Some((d.state.as_ref()?, *at)))
        })
}

/// The server's id for the track `id` last reported as current.
pub fn current_track(id: &str) -> Option<String> {
    with(|s| {
        let (state, _) = reported(s, id)?;
        state.queue.iter().find(|e| e.current)?.track_id.clone()
    })
}

/// What `id` last reported, to compare with what it reports next.
pub fn last_report(id: &str) -> Option<LinkState> {
    with(|s| reported(s, id).map(|(state, _)| state.clone()))
}

/// Wait up to `within` for `id` to report `track_id` as its current track
/// in a queue it did not have before: `before` is `last_report` from just
/// before asking. Sending a command only queues it: on a socket that may
/// have died unnoticed, in the server's store for a device asleep, or in a
/// push nobody has tapped. A hand-off learns the music arrived from this.
pub fn await_current(
    id: &str,
    track_id: &str,
    before: Option<&LinkState>,
    within: Duration,
) -> bool {
    let took = || with(|s| reported(s, id).is_some_and(|(now, _)| took(before, now, track_id)));
    await_until(took, within)
}

/// Whether `now` shows `track_id` arrived since `before`. A hand-off replaces
/// the queue, so its entries are new: the current one has an id `before`'s
/// did not. Being on the same track already, or any other report since (a
/// seek, the outputs, the sleep timer), is not an answer. A device that
/// names no entries has only its whole report to go on.
fn took(before: Option<&LinkState>, now: &LinkState, track_id: &str) -> bool {
    let current = |state: &LinkState| state.queue.iter().find(|e| e.current).cloned();
    let Some(entry) = current(now) else {
        return false;
    };
    if entry.track_id.as_deref() != Some(track_id) {
        return false;
    }
    let was = before.and_then(current);
    match (&entry.id, was.as_ref().and_then(|e| e.id.as_ref())) {
        (Some(id), Some(was)) => id != was,
        (Some(_), None) => true,
        (None, _) => before != Some(now),
    }
}

/// Wait up to `within` for `done` to hold, looking again at each change to
/// the engine or to this store.
pub fn await_until(done: impl Fn() -> bool, within: Duration) -> bool {
    let until = Instant::now() + within;
    let signal = crate::signal::engine_changed();
    let mut seen = signal.generation();
    loop {
        if done() {
            return true;
        }
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return false;
        }
        seen = signal.wait_until(seen, left);
    }
}

/// The target as `list` would give it.
pub fn target_device() -> Option<Device> {
    let id = target()?;
    list().into_iter().find(|d| d.id == id)
}

/// Get `cmd` to the device `id` over a connection that is up now: the local
/// network's, else the link. Never queued, never a push, never the HTTP
/// fallback `send` has: for what is only worth saying while someone is there
/// to hear it, like `WatchLevels`. A stop goes both ways, since the start may
/// have taken either. `false` when there was no way through.
pub fn send_live(id: &str, cmd: LinkCommand) -> bool {
    let everywhere = matches!(cmd, LinkCommand::WatchLevels { on: false });
    let near = cmd.allowed_nearby() && crate::remote::nearby::send(id, cmd.clone());
    if near && !everywhere {
        return true;
    }
    let linked = link::report(LinkReport::Command {
        to: id.to_string(),
        command: cmd,
        ack: None,
    });
    near || linked
}

/// Get `cmd` to the device `id`: over the local network if connected there,
/// else up the link, else in one request to the server, which is what a Live
/// Activity's button has while iOS keeps the app's link down.
pub fn send(id: &str, cmd: LinkCommand) -> Result<(), String> {
    send_then(id, cmd, None)
}

/// `send`, handing the outcome to `then` once it is known, rather than to the
/// app's notice. Returns as soon as the command is on its way.
pub fn send_then(id: &str, cmd: LinkCommand, then: Option<Then>) -> Result<(), String> {
    choosable(id)?;
    used(id);
    let (nearby, own, shared) = with(|s| {
        let listed = s.account.iter().find(|(d, _)| d.id == id);
        (
            s.nearby.iter().any(|n| n.hello.id == id),
            listed.is_some_and(|(d, _)| d.owner.is_none()),
            listed.is_some_and(|(d, _)| d.owner.is_some()),
        )
    });
    // Another account's device, or one on the network, takes the playback
    // set at most, and decides for itself; what only an account may send its
    // own devices goes to them alone.
    if !own && !cmd.allowed_playback() {
        return Err("Only your own devices can be asked that.".into());
    }
    let routes = Routes {
        lan: nearby,
        lan_answers: nearby && answers_on_network(id),
        link_answers: answers_by_link(id),
    };
    // One id for the command whichever way it goes, so a device it reaches
    // twice acts on it once; and listened for before it is sent, so an answer
    // quicker than this thread is still heard.
    let ack = acks::next_id();
    let answer = acks::expect(ack);
    if routes.lan {
        let sent = if routes.lan_answers {
            crate::remote::nearby::send_acked(id, cmd.clone(), ack)
        } else {
            crate::remote::nearby::send(id, cmd.clone())
        };
        if sent {
            follow(id, cmd, ack, answer, Route::Network, routes, then);
            return Ok(());
        }
    }
    let account = own || shared;
    if link::report(LinkReport::Command {
        to: id.to_string(),
        command: cmd.clone(),
        ack: routes.link_answers.then_some(ack),
    }) {
        follow(id, cmd, ack, answer, Route::Link, routes, then);
        return Ok(());
    }
    if !account && !nearby {
        // Not heard of here, but perhaps by the server: the target of a Live
        // Activity outlives this process's list of devices.
        log::info!("devices: {id} is not listed; asking the server");
    }
    acks::forget(ack);
    let outcome = ask_server(id, &cmd, Some(ack))?;
    told(id, &cmd, outcome, then);
    Ok(())
}

/// The ways a command can reach a device, and which of them bring an answer.
#[derive(Debug, Clone, Copy)]
struct Routes {
    lan: bool,
    lan_answers: bool,
    link_answers: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    Network,
    Link,
}

/// How long to wait for an answer by each way before trying the next. Up the
/// link it is the server's round trip and its own fallback to a push.
const NETWORK_ANSWER: Duration = Duration::from_secs(1);
const LINK_ANSWER: Duration = Duration::from_secs(3);

/// Wait for `id`'s answer to the command sent under `ack` by `route`, off the
/// caller's thread, and try the next way when none comes. A way that brings
/// no answer is taken as sent, as it always was.
fn follow(
    id: &str,
    cmd: LinkCommand,
    ack: u64,
    answer: crossbeam_channel::Receiver<acks::AckOutcome>,
    route: Route,
    routes: Routes,
    then: Option<Then>,
) {
    let answers = match route {
        Route::Network => routes.lan_answers,
        Route::Link => routes.link_answers,
    };
    if !answers {
        acks::forget(ack);
        told(id, &cmd, None, then);
        return;
    }
    let id = id.to_owned();
    let _ = std::thread::Builder::new()
        .name("koan-ack".into())
        .spawn(move || {
            let walked = walk(&id, &cmd, ack, &answer, route, routes);
            acks::forget(ack);
            match walked {
                Ok(outcome) => told(&id, &cmd, outcome, then),
                Err(error) => unanswered(&id, &cmd, error, then),
            }
        });
}

/// What became of a command sent under `ack`: the first answer by any way.
/// `Err` when this device could not ask the server either: no answer, which
/// says nothing of whether an earlier way delivered it.
fn walk(
    id: &str,
    cmd: &LinkCommand,
    ack: u64,
    answer: &crossbeam_channel::Receiver<acks::AckOutcome>,
    first: Route,
    routes: Routes,
) -> Result<Option<acks::AckOutcome>, String> {
    if first == Route::Network {
        if let Ok(outcome) = answer.recv_timeout(NETWORK_ANSWER) {
            return Ok(Some(outcome));
        }
        log::info!("devices: no answer from {id} on the network; trying the link");
        if routes.link_answers
            && link::report(LinkReport::Command {
                to: id.to_string(),
                command: cmd.clone(),
                ack: Some(ack),
            })
            && let Ok(outcome) = answer.recv_timeout(LINK_ANSWER)
        {
            return Ok(Some(outcome));
        }
    } else if let Ok(outcome) = answer.recv_timeout(LINK_ANSWER) {
        return Ok(Some(outcome));
    }
    log::info!("devices: no answer from {id}; asking the server");
    ask_server(id, cmd, Some(ack))
}

/// No answer came, and this device could not ask the server: the command may
/// have arrived by an earlier way or may not. A sender that takes the outcome
/// is told none, as for a way that brings no answer, so it does not act as
/// if refused; the app is told the device could not be reached.
fn unanswered(id: &str, cmd: &LinkCommand, error: String, then: Option<Then>) {
    match then {
        Some(then) => {
            log::warn!("devices: no answer from {id} for {cmd:?}, and the server: {error}");
            then(None);
        }
        None => told(id, cmd, Some(acks::AckOutcome::Failed { error }), None),
    }
}

/// What a command came to, once known. `None`: sent by a way that brings no
/// answer.
/// `then` has it when given; otherwise one that was queued or did not arrive
/// becomes the app's notice.
fn told(id: &str, cmd: &LinkCommand, outcome: Option<acks::AckOutcome>, then: Option<Then>) {
    match &outcome {
        Some(acks::AckOutcome::Done) | None => {}
        Some(acks::AckOutcome::Queued) => {
            log::info!("devices: {id} is asleep; {cmd:?} waits for it")
        }
        Some(other) => log::warn!("devices: {cmd:?} did not reach {id}: {other:?}"),
    }
    if let Some(then) = then {
        then(outcome);
        return;
    }
    let Some(outcome) = outcome.filter(|o| *o != acks::AckOutcome::Done) else {
        return;
    };
    let device = list()
        .into_iter()
        .find(|d| d.id == id)
        .map_or_else(|| id.to_owned(), |d| d.name);
    changed(|s| {
        let seq = s.notice.as_ref().map_or(1, |n| n.seq + 1);
        s.notice = Some(Notice {
            seq,
            device,
            outcome,
        });
    });
}

/// Whether `id`, reached on the local network, answers commands.
fn answers_on_network(id: &str) -> bool {
    with(|s| s.nearby.iter().any(|n| n.hello.id == id && n.hello.acks))
}

/// Whether `id`, reached up the link, answers: the server relays answers and
/// says the device gives them.
fn answers_by_link(id: &str) -> bool {
    crate::remote::profile::current().is_some_and(|p| p.offers(crate::remote::profile::ACK))
        && with(|s| s.account.iter().any(|(d, _)| d.id == id && d.acks))
}

/// Get `cmd` to `id` in one request to the server, which relays it down the
/// device's link or wakes it. With `ack`, a server that answers waits a moment
/// for the device and says how it went; one that does not answers `None`.
fn ask_server(
    id: &str,
    cmd: &LinkCommand,
    ack: Option<u64>,
) -> Result<Option<acks::AckOutcome>, String> {
    let cfg = Config::load().unwrap_or_default();
    let client = crate::helpers::subsonic_client(&cfg).ok_or("not signed in to a server")?;
    let json = serde_json::to_string(cmd).map_err(|e| e.to_string())?;
    client
        .koan_command(id, &json, ack)
        .map_err(|e| e.to_string())
}

/// `send`, by the server first: for the lock screen's buttons. iOS runs
/// their intent without waking the app's scene, so the link the app left
/// open is most likely dead, and `send` would put the command into it and
/// call it sent. The server knows whether the device is linked, and pushes
/// to it if not. A device the server cannot reach, one only on this
/// network, falls back to `send`.
pub fn send_by_server_first(id: &str, cmd: LinkCommand) -> Result<(), String> {
    server_first(
        || {
            ask_server(id, &cmd, Some(acks::next_id())).map(|outcome| match outcome {
                Some(acks::AckOutcome::Refused { reason }) => Err(reason),
                outcome => {
                    told(id, &cmd, outcome, None);
                    Ok(())
                }
            })?
        },
        || send(id, cmd.clone()),
    )
}

fn server_first(
    server: impl FnOnce() -> Result<(), String>,
    otherwise: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    server().or_else(|why| {
        log::info!("devices: the server did not take it ({why}); sending it here");
        otherwise()
    })
}

/// This device's id, as other devices know it.
pub fn this_id() -> Option<String> {
    local().map(|l| l.identity.device_id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_live_send_with_no_way_through_sends_nothing() {
        // No connection on the network and no link: false, and no fallback.
        assert!(!send_live(
            "nowhere-at-all",
            LinkCommand::WatchLevels { on: true }
        ));
    }

    fn device(id: &str, playing: bool) -> LinkDevice {
        LinkDevice {
            id: id.into(),
            name: id.into(),
            platform: "macos".into(),
            linked: true,
            state: Some(LinkState {
                playing,
                position_ms: 1000,
                duration_ms: 2000,
                ..Default::default()
            }),
            last_seen: None,
            wakeable: None,
            owner: None,
            acks: false,
        }
    }

    /// The store is process-wide: each test walks it through its states with
    /// this held, rather than racing the others over it.
    static STORE_LOCK: Mutex<()> = Mutex::new(());

    fn hello(id: &str) -> LinkHello {
        LinkHello {
            id: id.into(),
            name: id.into(),
            platform: "ios".into(),
            library: None,
            acks: false,
            nonce: None,
        }
    }

    #[test]
    fn the_lock_screen_goes_by_the_server_and_falls_back_only_when_refused() {
        let local = std::cell::Cell::new(0);
        let here = || {
            local.set(local.get() + 1);
            Ok(())
        };
        assert_eq!(server_first(|| Ok(()), here), Ok(()));
        assert_eq!(local.get(), 0, "the server took it: nothing sent here");

        assert_eq!(server_first(|| Err("no such device".into()), here), Ok(()));
        assert_eq!(local.get(), 1, "refused there: sent here instead");

        assert_eq!(
            server_first(|| Err("offline".into()), || Err("not reachable".into())),
            Err("not reachable".to_string())
        );
    }

    /// A device on `track`, as the entry `entry` of its queue.
    fn playing(track: &str, entry: &str) -> LinkState {
        LinkState {
            queue: vec![crate::remote::link::LinkQueueEntry {
                id: Some(entry.into()),
                track_id: Some(track.into()),
                current: true,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    /// What the apps are given for a device's playhead runs on from the
    /// report, so stamping it with the time the list was published does not
    /// set it back by however long ago the device reported.
    #[test]
    fn a_playing_devices_playhead_runs_on_from_its_report() {
        let _held = STORE_LOCK.lock();
        crate::config::isolate_config_for_tests();
        with(|s| *s = Store::default());
        nearby_hello(hello("speaker"), "10.0.0.6:5626");
        let at = |playing| LinkState {
            playing,
            position_ms: 1000,
            duration_ms: 60_000,
            ..Default::default()
        };
        let position = || {
            list()
                .into_iter()
                .find(|d| d.id == "speaker")
                .unwrap()
                .position_ms()
        };

        nearby_state("speaker", at(true));
        std::thread::sleep(Duration::from_millis(50));
        assert!(position() >= 1050, "{}", position());

        nearby_state("speaker", at(false));
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(position(), 1000, "paused, it stays put");
    }

    #[test]
    fn a_command_queued_or_refused_becomes_the_notice_unless_its_sender_takes_it() {
        let _held = STORE_LOCK.lock();
        crate::config::isolate_config_for_tests();
        with(|s| *s = Store::default());
        nearby_hello(hello("phone"), "10.0.0.5:5626");

        told(
            "phone",
            &LinkCommand::Pause,
            Some(acks::AckOutcome::Done),
            None,
        );
        told("phone", &LinkCommand::Pause, None, None);
        assert_eq!(notice(), None, "arrived, or sent as ever: nothing to say");

        told(
            "phone",
            &LinkCommand::Pause,
            Some(acks::AckOutcome::Queued),
            None,
        );
        let queued = notice().unwrap();
        assert_eq!(
            (queued.device.as_str(), &queued.outcome),
            ("phone", &acks::AckOutcome::Queued)
        );

        let failed = acks::AckOutcome::Failed {
            error: "gone".into(),
        };
        told("phone", &LinkCommand::Pause, Some(failed.clone()), None);
        assert_eq!(
            notice().unwrap().seq,
            queued.seq + 1,
            "the same twice is still news"
        );

        // A sender that takes the outcome itself, as a hand-off does.
        let got = std::sync::Arc::new(parking_lot::Mutex::new(None));
        let into = got.clone();
        let before = notice();
        told(
            "phone",
            &LinkCommand::Pause,
            Some(failed.clone()),
            Some(Box::new(move |o| *into.lock() = Some(o))),
        );
        assert_eq!(*got.lock(), Some(Some(failed)));
        assert_eq!(notice(), before, "not said twice");
    }

    /// This device failing to reach the server, after no answer by the other
    /// ways, is not the target refusing: the command may have arrived by an
    /// earlier way. A hand-off waiting on it hears no answer, so it does not
    /// resume the music here; the app is told the device could not be reached.
    #[test]
    fn failing_to_ask_the_server_is_no_answer_not_a_refusal() {
        let _held = STORE_LOCK.lock();
        crate::config::isolate_config_for_tests();
        with(|s| *s = Store::default());
        nearby_hello(hello("phone"), "10.0.0.5:5626");

        let got = std::sync::Arc::new(parking_lot::Mutex::new(None));
        let into = got.clone();
        unanswered(
            "phone",
            &LinkCommand::Pause,
            "server unreachable".into(),
            Some(Box::new(move |o| *into.lock() = Some(o))),
        );
        assert_eq!(*got.lock(), Some(None), "no answer, not a refusal");
        assert_eq!(notice(), None);

        unanswered(
            "phone",
            &LinkCommand::Pause,
            "server unreachable".into(),
            None,
        );
        assert!(matches!(
            notice().map(|n| n.outcome),
            Some(acks::AckOutcome::Failed { .. })
        ));
    }

    #[test]
    fn a_hand_off_is_taken_only_when_the_device_reports_the_track() {
        let _held = STORE_LOCK.lock();
        crate::config::isolate_config_for_tests();
        with(|s| *s = Store::default());
        nearby_hello(hello("phone"), "10.0.0.5:5626");
        nearby_state("phone", playing("before", "e1"));
        let short = Duration::from_millis(50);

        // Sent, and nothing heard back: queued is not taken.
        let before = last_report("phone");
        assert!(!await_current("phone", "handed", before.as_ref(), short));

        // Already on the handed track, and then any other report from that
        // same queue entry (a seek, say), is no answer.
        nearby_state("phone", playing("handed", "e2"));
        let before = last_report("phone");
        let mut seeked = playing("handed", "e2");
        seeked.position_ms = 5000;
        nearby_state("phone", seeked);
        assert!(!await_current("phone", "handed", before.as_ref(), short));

        // A new queue on another track is not this one.
        let reporter = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(20));
            nearby_state("phone", playing("other", "e3"));
        });
        assert!(!await_current(
            "phone",
            "handed",
            before.as_ref(),
            Duration::from_millis(200)
        ));
        reporter.join().unwrap();

        // The device saying it has the track in the queue it was sent.
        let before = last_report("phone");
        let reporter = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(20));
            nearby_state("phone", playing("handed", "e4"));
        });
        assert!(await_current(
            "phone",
            "handed",
            before.as_ref(),
            Duration::from_secs(5)
        ));
        reporter.join().unwrap();
    }

    #[test]
    fn a_device_naming_no_entries_is_judged_on_its_whole_report() {
        let unnamed = |track: &str, position_ms| LinkState {
            position_ms,
            queue: vec![crate::remote::link::LinkQueueEntry {
                track_id: Some(track.into()),
                current: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        let before = unnamed("handed", 0);
        assert!(!took(Some(&before), &before, "handed"));
        assert!(took(Some(&before), &unnamed("handed", 10), "handed"));
        assert!(took(None, &unnamed("handed", 0), "handed"));
    }

    #[test]
    fn a_device_that_stops_answering_sleeps_after_a_heartbeat_and_stays() {
        let _held = STORE_LOCK.lock();
        crate::config::isolate_config_for_tests();
        with(|s| *s = Store::default());
        let week = 7 * 24 * 60 * 60;

        nearby_hello(hello("stranger"), "10.0.0.5:5626");
        used("stranger");
        nearby_gone("stranger");
        let now = with(|s| s.live["stranger"]);
        let at = |t: i64| list_at(t).into_iter().find(|d| d.id == "stranger");

        let missed_one = at(now + STALE_SECS - 1).expect("still listed");
        assert!(
            !missed_one.asleep,
            "one missed heartbeat does not make it asleep"
        );
        assert!(
            !missed_one.awake && !missed_one.nearby,
            "nor does it pretend to be reachable"
        );
        let asleep = at(now + STALE_SECS + 1).expect("listed asleep");
        assert!(asleep.asleep && !asleep.awake && !asleep.wakeable);
        assert_eq!(asleep.last_seen, Some(now));
        assert!(asleep.state.is_none(), "what it was doing is not news");
        assert!(
            at(now + week).is_some_and(|d| d.asleep),
            "kept long past the old grace, until forgotten"
        );
        assert_eq!(
            next_change(now),
            Some(now + STALE_SECS),
            "the clock wakes for the next crossing"
        );
        assert_eq!(next_change(now + STALE_SECS), None, "and then for nothing");

        // Heard from again: back at once, with no wait.
        nearby_hello(hello("stranger"), "10.0.0.5:5626");
        let back = at(now + STALE_SECS + 5).unwrap();
        assert!(back.awake && back.nearby && !back.asleep);

        // Asleep, a stranger cannot be chosen or sent to.
        nearby_gone("stranger");
        with(|s| *s.live.get_mut("stranger").unwrap() -= STALE_SECS + 10);
        assert!(choosable("stranger").is_err());
        assert!(send("stranger", LinkCommand::Pause).is_err());

        // An account device a push can wake, asleep for days.
        let mut phone = device("phone", false);
        phone.linked = false;
        phone.last_seen = Some(now - week);
        phone.wakeable = Some(true);
        set_account(vec![phone]);
        used("phone");
        let phone = list_at(now).into_iter().find(|d| d.id == "phone").unwrap();
        assert!(phone.asleep && phone.wakeable);
        assert_eq!(phone.last_seen, Some(now - week));
        assert!(choosable("phone").is_ok());

        // An account device that unlinks without a push token: reconnecting
        // for a heartbeat, then asleep, and kept.
        set_linked(true);
        set_account(vec![device("mac", false)]);
        used("mac");
        set_account(Vec::new());
        let left = with(|s| s.live["mac"]);
        let mac = |t: i64| list_at(t).into_iter().find(|d| d.id == "mac");
        let reconnecting = mac(left + 1).unwrap();
        assert!(!reconnecting.awake && !reconnecting.asleep);
        let asleep = mac(left + STALE_SECS + 1).unwrap();
        assert!(asleep.asleep && !asleep.wakeable && asleep.account);
        assert!(mac(left + week).is_some());
        with(|s| *s = Store::default());
    }

    /// A device seen and never used is gone once out of reach past the
    /// heartbeat; one used is kept asleep, across a relaunch, until forgotten.
    #[test]
    fn only_a_device_used_here_is_kept_asleep() {
        let _held = STORE_LOCK.lock();
        crate::config::isolate_config_for_tests();
        with(|s| *s = Store::default());

        nearby_hello(hello("passing"), "10.0.0.5:5626");
        nearby_hello(hello("played-on"), "10.0.0.6:5626");
        used("played-on");
        nearby_gone("passing");
        nearby_gone("played-on");
        let left = with(|s| s.live["passing"]);
        let listed = |t: i64, id: &str| list_at(t).into_iter().any(|d| d.id == id);
        assert!(listed(left + 1, "passing"), "reconnecting for a heartbeat");
        assert!(
            !listed(left + STALE_SECS + 1, "passing"),
            "never used: gone"
        );
        assert!(listed(left + STALE_SECS + 1, "played-on"), "used: asleep");

        // An account device the server still lists, asleep, never used here.
        let mut phone = device("old-phone", false);
        phone.linked = false;
        phone.last_seen = Some(left - 3600);
        set_account(vec![phone]);
        assert!(!listed(left, "old-phone"));

        with(|s| *s = Store::default());
        restore();
        let later = left + STALE_SECS + 1;
        assert!(
            list_at(later)
                .iter()
                .any(|d| d.id == "played-on" && d.asleep),
            "kept across a relaunch"
        );
        assert!(!listed(later, "passing"));

        forget("played-on").unwrap();
        assert!(!list().iter().any(|d| d.id == "played-on"));
        with(|s| *s = Store::default());
        restore();
        assert!(
            !list().iter().any(|d| d.id == "played-on"),
            "forgotten on disk"
        );
        with(|s| *s = Store::default());
    }

    /// The device being controlled is used, whatever else is not; a file from
    /// before this was kept counts only that one, so old clutter clears.
    #[test]
    fn the_controlled_device_is_kept_and_an_old_file_keeps_only_it() {
        let _held = STORE_LOCK.lock();
        crate::config::isolate_config_for_tests();
        with(|s| *s = Store::default());
        let gone = |id: &str| Departed {
            id: id.into(),
            name: id.into(),
            platform: "ios".into(),
            account: false,
            same_library: false,
            shared: false,
        };
        let old = serde_json::json!({
            "target": {"id": "tv", "name": "tv", "platform": "tvos"},
            "account": [],
            "live": {"tv": 1, "sim": 1},
            "departed": [gone("tv"), gone("sim")],
        });
        std::fs::write(saved_path(), old.to_string()).unwrap();
        restore();
        let ids: Vec<String> = list().into_iter().map(|d| d.id).collect();
        assert_eq!(
            ids,
            ["tv"],
            "the target stays; the simulator nobody used goes"
        );

        set_target(None);
        nearby_hello(hello("mac"), "10.0.0.7:5626");
        set_target(Some("mac".into()));
        set_target(None);
        nearby_gone("mac");
        with(|s| *s.live.get_mut("mac").unwrap() -= STALE_SECS + 10);
        assert!(
            list().iter().any(|d| d.id == "mac" && d.asleep),
            "controlled once: kept"
        );
        with(|s| *s = Store::default());
    }

    /// Forgotten here, a stranger is gone, and kept gone across a relaunch;
    /// heard on the network again, it is back. Forgetting is not a block.
    #[test]
    fn a_forgotten_device_is_gone_until_it_comes_back() {
        let _held = STORE_LOCK.lock();
        crate::config::isolate_config_for_tests();
        with(|s| *s = Store::default());
        nearby_hello(hello("stranger"), "10.0.0.5:5626");
        assert!(forget("stranger").is_err(), "not while it is reachable");
        used("stranger");
        nearby_gone("stranger");
        // Kept across a relaunch, asleep, until forgotten.
        with(|s| *s = Store::default());
        restore();
        assert!(list().iter().any(|d| d.id == "stranger"));
        forget("stranger").unwrap();
        assert!(!list().iter().any(|d| d.id == "stranger"));
        assert!(remembered_nearby().iter().all(|n| n.id != "stranger"));
        with(|s| *s = Store::default());
        restore();
        assert!(
            !list().iter().any(|d| d.id == "stranger"),
            "forgotten on disk too"
        );
        nearby_hello(hello("stranger"), "10.0.0.5:5626");
        assert!(list().iter().any(|d| d.id == "stranger" && d.awake));
        with(|s| *s = Store::default());
    }

    /// One of the account's is forgotten through the server, which has to be
    /// reachable to hear it; what the server says it forgot goes from here.
    #[test]
    fn an_account_device_is_forgotten_through_the_server() {
        let _held = STORE_LOCK.lock();
        crate::config::isolate_config_for_tests();
        with(|s| *s = Store::default());
        let mut phone = device("phone", false);
        phone.linked = false;
        phone.wakeable = Some(true);
        phone.last_seen = Some(chrono::Utc::now().timestamp() - 3600);
        set_account(vec![phone]);
        used("phone");
        assert!(
            forget("phone").is_err(),
            "no link here: the server would still list it and wake it"
        );
        assert!(list().iter().any(|d| d.id == "phone"));
        // What every linked device of the account hears once it is.
        forgotten("phone");
        assert!(!list().iter().any(|d| d.id == "phone"));
        with(|s| *s = Store::default());
    }

    /// Run a wake with a device that answers in stage `answers_in`, if any:
    /// the stages started, and how it ended.
    fn walk(recent: bool, answers_in: Option<Waking>) -> (Vec<Waking>, Ended) {
        let plan = wake_plan(recent, true);
        let mut started = Vec::new();
        let current = std::cell::RefCell::new(None);
        let ended = run_wake(
            &plan,
            || true,
            |stage| {
                started.push(stage.clone());
                *current.borrow_mut() = Some(stage.clone());
                Ok(())
            },
            |_| *current.borrow() == answers_in,
        );
        (started, ended)
    }

    #[test]
    fn waking_tries_the_network_first_only_when_it_was_there_a_moment_ago() {
        let (tried, _) = walk(true, None);
        assert_eq!(tried, [Waking::Network, Waking::Push, Waking::Notification]);
        let (tried, _) = walk(false, None);
        assert_eq!(tried, [Waking::Push, Waking::Notification]);
        assert_eq!(
            wake_plan(true, false)
                .iter()
                .map(|(w, _)| w.clone())
                .collect::<Vec<_>>(),
            [Waking::Network],
            "a stranger is only ever tried on the network"
        );
        assert!(wake_plan(false, false).is_empty());
    }

    #[test]
    fn a_wake_stops_at_the_stage_that_reached_it() {
        let (tried, ended) = walk(true, Some(Waking::Network));
        assert_eq!(tried, [Waking::Network], "no push for a phone still awake");
        assert_eq!(ended, Ended::Reached(Waking::Network));
        let (tried, ended) = walk(false, Some(Waking::Push));
        assert_eq!(
            tried,
            [Waking::Push],
            "no notification once the push worked"
        );
        assert_eq!(ended, Ended::Reached(Waking::Push));
    }

    #[test]
    fn a_push_that_does_not_wake_it_falls_back_to_a_notification_then_says_so() {
        let (tried, ended) = walk(false, None);
        assert_eq!(tried.last(), Some(&Waking::Notification));
        assert!(matches!(ended, Ended::Failed(why) if why.contains("notification")));
        let plan = wake_plan(false, true);
        assert!(plan[0].1 <= Duration::from_secs(8) && plan[0].1 >= Duration::from_secs(5));
    }

    #[test]
    fn a_stage_that_cannot_start_ends_the_wake_with_why() {
        let ended = run_wake(
            &wake_plan(false, true),
            || true,
            |_| Err("Not connected".into()),
            |_| true,
        );
        assert_eq!(ended, Ended::Failed("Not connected".into()));
    }

    /// Choosing another device mid-wake sends nothing more to the one left
    /// behind: no notification "wants to play here" for a choice taken back.
    #[test]
    fn a_wake_abandoned_for_another_choice_sends_nothing_more() {
        let live = std::cell::Cell::new(true);
        let mut tried = Vec::new();
        let ended = run_wake(
            &wake_plan(true, true),
            || live.get(),
            |stage| {
                tried.push(stage.clone());
                Ok(())
            },
            |_| {
                live.set(false);
                false
            },
        );
        assert_eq!(tried, [Waking::Network]);
        assert_eq!(ended, Ended::Cancelled);
    }

    #[test]
    fn one_wake_at_a_time_joined_by_the_same_choice_and_ended_by_another() {
        let _held = STORE_LOCK.lock();
        with(|s| *s = Store::default());
        let first = begin_wake("phone").expect("starts");
        assert!(begin_wake("phone").is_none(), "choosing it again joins it");
        assert!(wake_is_current(first));
        cancel_wake(Some("phone"));
        assert!(wake_is_current(first), "the same choice does not end it");
        cancel_wake(Some("mac"));
        assert!(!wake_is_current(first), "another choice does");
        let second = begin_wake("mac").unwrap();
        assert!(begin_wake("phone").is_some() && !wake_is_current(second));
        with(|s| *s = Store::default());
    }

    /// The wake starts the moment a device stops being reachable, not once
    /// the heartbeat has passed and it is labelled asleep.
    #[test]
    fn a_device_chosen_seconds_after_it_dropped_is_woken() {
        let _held = STORE_LOCK.lock();
        crate::config::isolate_config_for_tests();
        with(|s| *s = Store::default());
        set_linked(true);
        set_account(vec![device("phone", false)]);
        let mut phone = device("phone", false);
        phone.linked = false;
        phone.wakeable = Some(true);
        phone.last_seen = Some(chrono::Utc::now().timestamp() - 10);
        set_account(vec![phone]);
        let listed = list().into_iter().find(|d| d.id == "phone").unwrap();
        assert!(!listed.asleep && !listed.awake, "reconnecting, not asleep");
        wake("phone");
        // Its thread may already have finished: with no link here, the push
        // stage fails at once and says so on the row.
        let ran = (0..100).any(|_| {
            let seen = with(|s| {
                s.attempt.as_ref().is_some_and(|(_, d)| d == "phone")
                    || s.waking.contains_key("phone")
            });
            if !seen {
                std::thread::sleep(Duration::from_millis(10));
            }
            seen
        });
        assert!(ran, "the pipeline runs");
        cancel_wake(None);
        with(|s| *s = Store::default());
    }

    /// A device linked for ten minutes that left half a minute ago is tried
    /// on the network first: the time is when it was last heard there, not
    /// when it first connected.
    #[test]
    fn last_heard_on_the_network_is_the_last_message_not_the_first() {
        let _held = STORE_LOCK.lock();
        crate::config::isolate_config_for_tests();
        with(|s| *s = Store::default());
        nearby_hello(hello("phone"), "10.0.0.7:5626");
        with(|s| {
            for seen in &mut s.seen {
                seen.at -= 10 * 60;
            }
        });
        nearby_state("phone", LinkState::default());
        nearby_gone("phone");
        let left = with(|s| s.lan_heard["phone"]);
        assert!(recently_on_network("phone", left + 30));
        assert!(!recently_on_network("phone", left + NETWORK_RECENT + 1));
        with(|s| *s = Store::default());
    }

    /// A device met only on the network, asleep, is woken through the
    /// server, which decides whether it may by where the two devices are; with
    /// no server to ask, it is not woken at all.
    #[test]
    fn a_stranger_asleep_on_the_network_is_woken_through_the_server_if_linked() {
        let _held = STORE_LOCK.lock();
        crate::config::isolate_config_for_tests();
        with(|s| *s = Store::default());
        nearby_hello(hello("stranger"), "10.0.0.5:5626");
        used("stranger");
        nearby_gone("stranger");
        with(|s| {
            *s.live.get_mut("stranger").unwrap() -= 10 * 60;
            *s.lan_heard.get_mut("stranger").unwrap() -= 10 * 60;
        });
        let listed = list().into_iter().find(|d| d.id == "stranger").unwrap();
        assert!(listed.asleep && !listed.wakeable, "no server to ask");
        wake("stranger");
        assert!(
            with(|s| s.attempt.is_none() && s.waking.is_empty()),
            "too long gone for the network, and no server"
        );

        set_linked(true);
        let listed = list().into_iter().find(|d| d.id == "stranger").unwrap();
        assert!(listed.asleep && listed.wakeable);
        assert!(choosable("stranger").is_ok());
        let plan = wake_plan(false, listed.wakeable);
        assert_eq!(plan[0].0, Waking::Push, "asks the server");
        with(|s| *s = Store::default());
    }

    /// What a stranger or a sharing account asks this device to pass on goes
    /// over the network or nowhere: never up this device's link, as its
    /// account, to devices the asker has no claim on.
    #[test]
    fn a_hand_off_asked_by_a_stranger_never_goes_up_the_link() {
        let _held = STORE_LOCK.lock();
        crate::config::isolate_config_for_tests();
        with(|s| *s = Store::default());
        set_linked(true);
        set_account(vec![device("mac", false)]);
        let play = LinkCommand::Play {
            track_ids: vec!["t".into()],
            start_at: 0,
            position_ms: 0,
            paused: false,
            handoff: true,
        };
        assert!(
            send_nearby("mac", play).is_err(),
            "the account's Mac is reached only through the link"
        );
        with(|s| *s = Store::default());
    }

    /// A hand-off asked by anyone but the account acts as the asker: never up
    /// this device's link to the account's own devices.
    #[test]
    fn a_hand_off_for_someone_else_never_reaches_the_accounts_own_devices() {
        let _held = STORE_LOCK.lock();
        crate::config::isolate_config_for_tests();
        with(|s| *s = Store::default());
        set_linked(true);
        set_account(vec![device("mac", false)]);
        let play = || LinkCommand::Play {
            track_ids: vec!["t".into()],
            start_at: 0,
            position_ms: 0,
            paused: false,
            handoff: true,
        };
        for source in [
            CommandSource::Shared,
            CommandSource::Nearby,
            CommandSource::Stranger,
        ] {
            let refused = send_for(source, "mac", play()).unwrap_err();
            assert!(
                refused.contains("this account's") || refused.contains("not on this network"),
                "{source:?}: {refused}"
            );
        }
        with(|s| *s = Store::default());
    }

    /// Another account's device that stops being shared is gone at once.
    #[test]
    fn a_device_no_longer_shared_is_dropped_at_once() {
        let _held = STORE_LOCK.lock();
        crate::config::isolate_config_for_tests();
        with(|s| *s = Store::default());
        set_linked(true);
        let mut theirs = device("their-phone", false);
        theirs.owner = Some("k".into());
        set_account(vec![theirs]);
        let listed = list().into_iter().find(|d| d.id == "their-phone").unwrap();
        assert_eq!(listed.owner.as_deref(), Some("k"));
        assert!(!listed.account, "not this account's own");
        assert!(send("their-phone", LinkCommand::Sync { full: false }).is_err());
        set_account(Vec::new());
        assert!(!list().iter().any(|d| d.id == "their-phone"));
        with(|s| *s = Store::default());
    }

    /// A stranger whose connection just closed reads as reconnecting, and is
    /// not offered: nothing would carry a command to it.
    #[test]
    fn a_stranger_that_just_left_cannot_be_chosen() {
        let _held = STORE_LOCK.lock();
        crate::config::isolate_config_for_tests();
        with(|s| *s = Store::default());
        nearby_hello(hello("stranger"), "10.0.0.5:5626");
        nearby_gone("stranger");
        let listed = list().into_iter().find(|d| d.id == "stranger").unwrap();
        assert!(!listed.awake && !listed.asleep && !listed.nearby);
        assert!(choosable("stranger").is_err());
        assert!(send("stranger", LinkCommand::Pause).is_err());
        with(|s| *s = Store::default());
    }

    /// A list held while this app's own link was down says nothing about
    /// when anyone was seen: the server's `last_seen` stands.
    #[test]
    fn a_list_held_across_a_gap_in_the_link_does_not_stamp_now() {
        let _held = STORE_LOCK.lock();
        crate::config::isolate_config_for_tests();
        with(|s| *s = Store::default());
        set_linked(true);
        set_account(vec![device("phone", true)]);
        used("phone");
        // Suspended for hours; the phone left meanwhile.
        with(|s| *s.live.get_mut("phone").unwrap() -= 4 * 60 * 60);
        set_linked(false);
        let hours_ago = chrono::Utc::now().timestamp() - 3 * 60 * 60;
        set_linked(true);
        let mut phone = device("phone", false);
        phone.linked = false;
        phone.last_seen = Some(hours_ago);
        set_account(vec![phone]);
        let phone = list().into_iter().find(|d| d.id == "phone").unwrap();
        assert!(phone.asleep, "not reconnecting: it went hours ago");
        // The stamp from when it was linked is earlier than the server's, so
        // the server's stands.
        assert!(phone.last_seen.unwrap() >= hours_ago);
        assert!(
            phone.last_seen.unwrap() < chrono::Utc::now().timestamp() - STALE_SECS,
            "not seen just now"
        );
        with(|s| *s = Store::default());
    }

    /// Relaunched, a device the server no longer lists is gone, not a device
    /// seen just now; and one it does list shows nothing it was doing.
    #[test]
    fn a_restored_device_the_server_drops_is_gone() {
        let _held = STORE_LOCK.lock();
        crate::config::isolate_config_for_tests();
        with(|s| *s = Store::default());
        // What an older run left: no times for these.
        let saved = Saved {
            target: None,
            account: vec![device("mac", true), device("phone", true)],
            nearby: Vec::new(),
            live: HashMap::new(),
            departed: Vec::new(),
            used: None,
        };
        std::fs::write(saved_path(), serde_json::to_string(&saved).unwrap()).unwrap();
        restore();
        let listed = list();
        assert!(listed.iter().all(|d| d.state.is_none() && !d.awake));
        set_linked(true);
        let mut phone = device("phone", false);
        phone.linked = false;
        phone.wakeable = Some(true);
        set_account(vec![phone]);
        assert!(
            !list().iter().any(|d| d.id == "mac"),
            "never seen here, and not listed: dropped"
        );
        with(|s| *s = Store::default());
    }

    /// A server with no push key cannot wake anything, whatever tokens it
    /// holds: such a device is shown asleep and cannot be chosen.
    #[test]
    fn a_device_the_server_cannot_push_is_not_wakeable() {
        let _held = STORE_LOCK.lock();
        crate::config::isolate_config_for_tests();
        with(|s| *s = Store::default());
        let mut phone = device("phone", false);
        phone.linked = false;
        phone.wakeable = Some(false);
        phone.last_seen = Some(chrono::Utc::now().timestamp() - 3600);
        set_account(vec![phone]);
        used("phone");
        let phone = list().into_iter().find(|d| d.id == "phone").unwrap();
        assert!(phone.asleep && !phone.wakeable);
        assert!(choosable("phone").is_err());
        with(|s| *s = Store::default());
    }

    #[test]
    fn a_device_found_both_ways_is_one_and_a_lost_target_is_kept() {
        let _held = STORE_LOCK.lock();
        crate::config::isolate_config_for_tests();
        with(|s| *s = Store::default());
        set_account(vec![device("mac", false), device("phone", true)]);
        let listed = list();
        assert_eq!(listed[0].id, "phone", "playing first");
        assert_eq!(listed.len(), 2);

        nearby_hello(
            LinkHello {
                id: "mac".into(),
                name: "Mac".into(),
                platform: "macos".into(),
                library: None,
                acks: false,
                nonce: None,
            },
            "mac.local:5626",
        );
        nearby_hello(
            LinkHello {
                id: "tv".into(),
                name: "Living room".into(),
                platform: "macos".into(),
                library: Some("elsewhere".into()),
                acks: false,
                nonce: None,
            },
            "10.0.0.9:5626",
        );
        let listed = list();
        assert_eq!(listed.len(), 3);
        let mac = listed.iter().find(|d| d.id == "mac").unwrap();
        assert!(mac.account && mac.nearby && mac.same_library);
        let tv = listed.iter().find(|d| d.id == "tv").unwrap();
        assert!(!tv.account && !tv.same_library);

        set_target(Some("tv".into()));
        nearby_gone("tv");
        // Long past the heartbeat: asleep, kept, and still the target.
        with(|s| *s.live.get_mut("tv").unwrap() -= STALE_SECS + 31 * 60);
        assert_eq!(target(), Some("tv".into()), "kept while out of reach");
        let tv = list().into_iter().find(|d| d.id == "tv").unwrap();
        assert_eq!(tv.name, "Living room");
        assert!(!tv.awake && tv.asleep);

        // What a relaunch finds.
        with(|s| *s = Store::default());
        restore();
        assert_eq!(target(), Some("tv".into()));
        let listed = list();
        assert!(
            listed.iter().any(|d| d.id == "mac" && !d.awake),
            "last heard of, not linked"
        );
        let remembered = remembered_nearby();
        assert!(
            remembered
                .iter()
                .any(|n| n.id == "tv" && n.addr == "10.0.0.9:5626")
        );

        // What a device on the network asks this one to pass on stays on the
        // network: the account's Mac, out of reach there, is not sent to up
        // the link.
        nearby_gone("mac");
        let play = LinkCommand::Play {
            track_ids: vec!["t".into()],
            start_at: 0,
            position_ms: 0,
            paused: false,
            handoff: true,
        };
        assert!(listed.iter().any(|d| d.id == "mac" && d.account));
        assert!(send_nearby("mac", play).is_err());

        set_target(None);
        set_account(Vec::new());
        nearby_gone("mac");
        assert!(
            list().iter().all(|d| d.last_seen.is_some()),
            "only what has just left, kept for a heartbeat"
        );
        with(|s| *s = Store::default());
    }
}
