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
//! A device that stops answering is not dropped on the spot. It stays as it
//! was for one full heartbeat past when it should have been heard from, so a
//! single missed signal changes nothing; then it is listed asleep, with when
//! it was last seen; and a device that cannot be woken is dropped once
//! `devices.asleep_grace_mins` has passed. One that a push can wake stays
//! listed, since choosing it is how it is woken. A device heard from again
//! at any point is back at once.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};

use crate::config::Config;
use crate::remote::link::{self, LinkCommand, LinkDevice, LinkHello, LinkReport, LinkState, Local};

/// Another device, as the app shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub platform: String,
    /// Signed in to the same account on the same server.
    pub account: bool,
    /// Connected to over the local network now.
    pub nearby: bool,
    /// Reachable at once. An account device that is not is one iOS has
    /// suspended: a command wakes it, and music reaches it as a notification.
    pub awake: bool,
    /// Not heard from for longer than a heartbeat. `awake` is false too.
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
    /// Devices no list carries any more, kept until the grace period ends.
    departed: Vec<Departed>,
    /// Devices being woken, or that did not wake, by id.
    waking: HashMap<String, Waking>,
}

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
#[derive(Debug, Clone)]
struct Departed {
    id: String,
    name: String,
    platform: String,
    account: bool,
    nearby: bool,
    same_library: bool,
    state: Option<LinkState>,
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
        let grace = i64::from(Config::cached().devices.asleep_grace_mins) * 60;
        let now = chrono::Utc::now().timestamp();
        let wait =
            next_change(now, grace).map(|at| Duration::from_secs((at - now).max(0) as u64 + 1));
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
        // Not linked until the server says so: shown as last heard of.
        s.account = saved
            .account
            .into_iter()
            .map(|d| (LinkDevice { linked: false, ..d }, now))
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
    changed(|s| s.linked = linked);
}

pub fn set_account(devices: Vec<LinkDevice>) {
    let now = Instant::now();
    let unix = chrono::Utc::now().timestamp();
    changed(|s| {
        // Reachable up to this message: the hysteresis runs from here, not
        // from whenever it was last reported.
        for (old, _) in &s.account {
            if old.linked {
                s.live.insert(old.id.clone(), unix);
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
                account: true,
                nearby: false,
                same_library: true,
                state: old.state.clone(),
            })
            .collect();
        for d in leaving {
            // Listed until now, so last seen now if not since: a departed
            // device always has a time to expire from.
            s.live.entry(d.id.clone()).or_insert(unix);
            s.departed.retain(|g| g.id != d.id);
            s.departed.push(d);
        }
        s.account = devices.into_iter().map(|d| (d, now)).collect();
        s.save();
    });
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
            s.live
                .insert(id.to_string(), chrono::Utc::now().timestamp());
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
        s.live
            .insert(id.to_string(), chrono::Utc::now().timestamp());
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
            nearby: true,
            same_library: ours.is_some() && gone.hello.library == ours,
            state: gone.state,
        });
    });
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
    let cfg = Config::load().unwrap_or_default();
    list_at(
        chrono::Utc::now().timestamp(),
        i64::from(cfg.devices.asleep_grace_mins) * 60,
    )
}

/// `list` as of `now`, Unix seconds, dropping what cannot be woken once it
/// has been asleep for `grace` seconds.
fn list_at(now: i64, grace: i64) -> Vec<Device> {
    let cfg = Config::load().unwrap_or_default();
    let ours = link::library_fingerprint(&cfg);
    with(|s| {
        let live = |id: &str| s.live.get(id).copied();
        // Reachable, or missed for less than a heartbeat.
        let fresh = |id: &str| live(id).is_some_and(|at| now - at <= STALE_SECS);
        let mut out: Vec<Device> = s
            .account
            .iter()
            .map(|(d, at)| {
                let near = s.nearby.iter().find(|n| n.hello.id == d.id);
                let awake = d.linked || near.is_some() || fresh(&d.id);
                Device {
                    id: d.id.clone(),
                    name: d.name.clone(),
                    platform: d.platform.clone(),
                    account: true,
                    nearby: near.is_some(),
                    awake,
                    asleep: !awake,
                    // The server lists an unlinked device only when a push
                    // can wake it.
                    wakeable: true,
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
                }
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
            if out.iter().any(|d| d.id == g.id) {
                continue;
            }
            let last = live(&g.id);
            if last.is_some_and(|at| now - at > STALE_SECS + grace) {
                continue;
            }
            let awake = fresh(&g.id);
            out.push(Device {
                id: g.id.clone(),
                name: g.name.clone(),
                platform: g.platform.clone(),
                account: g.account,
                nearby: g.nearby && awake,
                awake,
                asleep: !awake,
                wakeable: false,
                last_seen: last,
                waking: None,
                same_library: g.same_library,
                // What it was doing is not what it is doing now.
                state: awake.then(|| g.state.clone()).flatten(),
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
            if !d.awake {
                d.waking = s.waking.get(&d.id).cloned();
            }
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

/// When `list` next changes on its own: a device crossing into asleep, or
/// past the grace period.
fn next_change(now: i64, grace: i64) -> Option<i64> {
    with(|s| {
        let reachable = |id: &str| {
            s.nearby.iter().any(|n| n.hello.id == id)
                || s.account.iter().any(|(d, _)| d.id == id && d.linked)
        };
        s.live
            .iter()
            .filter(|(id, _)| !reachable(id))
            .flat_map(|(_, at)| [at + STALE_SECS, at + STALE_SECS + grace])
            .filter(|t| *t > now)
            .min()
    })
}

/// Whether the app may choose `id`: not when it is asleep with no way to wake
/// it, where a command would go nowhere.
pub fn choosable(id: &str) -> Result<(), String> {
    match list().into_iter().find(|d| d.id == id) {
        Some(d) if d.asleep && !d.wakeable && d.problem.is_none() => Err(format!(
            "{} is asleep and cannot be woken from here.",
            d.name
        )),
        _ => Ok(()),
    }
}

/// A device seen on the local network this recently is dialled there first.
const NETWORK_RECENT: i64 = 60;
/// How long each stage is given to bring the device in before the next.
const NETWORK_WAIT: Duration = Duration::from_secs(3);
const PUSH_WAIT: Duration = Duration::from_secs(6);
const NOTIFICATION_WAIT: Duration = Duration::from_secs(45);

/// The stages to try, each with how long to wait on it.
fn wake_plan(seen_nearby_recently: bool) -> Vec<(Waking, Duration)> {
    let mut plan = Vec::new();
    if seen_nearby_recently {
        plan.push((Waking::Network, NETWORK_WAIT));
    }
    plan.push((Waking::Push, PUSH_WAIT));
    plan.push((Waking::Notification, NOTIFICATION_WAIT));
    plan
}

/// Walk `plan`: `act` starts each stage, `wait` blocks until the device is
/// reachable or the stage's time is up, saying which. The stage it was
/// reached in, or the failure to show.
fn run_wake(
    plan: &[(Waking, Duration)],
    mut act: impl FnMut(&Waking) -> Result<(), String>,
    mut wait: impl FnMut(Duration) -> bool,
) -> Result<Waking, Waking> {
    let mut last = None;
    for (stage, time) in plan {
        if let Err(why) = act(stage) {
            return Err(Waking::Failed(why));
        }
        last = Some(stage);
        if wait(*time) {
            return Ok(stage.clone());
        }
    }
    Err(Waking::Failed(match last {
        Some(Waking::Notification) => {
            "Did not wake. A notification is waiting on it to be tapped.".into()
        }
        _ => "Did not wake.".into(),
    }))
}

fn reachable(id: &str) -> bool {
    with(|s| {
        s.nearby.iter().any(|n| n.hello.id == id)
            || s.account.iter().any(|(d, _)| d.id == id && d.linked)
    })
}

/// Wake the asleep device `id`, in the background: on the local network if it
/// was there a moment ago, then a background push, then a notification on it
/// to tap. The device's row says which stage it is at, and why it failed if it
/// did. Each step is logged with its time from the start, so a wake that did
/// not happen shows where it stopped.
pub fn wake(id: &str) {
    let id = id.to_string();
    // Only what this device may wake: its account's, through the server. A
    // stranger on the network has nothing to answer a push with.
    let Some(device) = list()
        .into_iter()
        .find(|d| d.id == id && d.asleep && d.wakeable)
    else {
        return;
    };
    let name = device.name;
    let now = chrono::Utc::now().timestamp();
    let recent = remembered_nearby()
        .iter()
        .any(|n| n.id == id && now - n.at <= NETWORK_RECENT);
    let _ = std::thread::Builder::new()
        .name("koan-wake".into())
        .spawn(move || {
            let started = Instant::now();
            let ms = || started.elapsed().as_millis();
            let plan = wake_plan(recent);
            let outcome = run_wake(
                &plan,
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
                        if left.is_zero() {
                            return false;
                        }
                        seen = signal.wait_until(seen, left);
                    }
                    true
                },
            );
            match outcome {
                Ok(stage) => {
                    log::info!("wake: {name}: reached at +{}ms, after {stage:?}", ms());
                    changed(|s| s.waking.remove(&id));
                }
                Err(failed) => {
                    log::warn!("wake: {name}: did not wake in {}ms ({failed:?})", ms());
                    changed(|s| s.waking.insert(id.clone(), failed));
                }
            }
        });
}

/// The device the app is controlling; `None` for this one.
pub fn target() -> Option<String> {
    with(|s| s.target.as_ref().map(|t| t.id.clone()))
}

pub fn set_target(id: Option<String>) {
    let listed = id
        .as_ref()
        .and_then(|id| list().into_iter().find(|d| d.id == *id));
    changed(|s| {
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

/// The target as `list` would give it.
pub fn target_device() -> Option<Device> {
    let id = target()?;
    list().into_iter().find(|d| d.id == id)
}

/// Get `cmd` to the device `id`: over the local network if connected there,
/// else up the link, else in one request to the server, which is what a Live
/// Activity's button has while iOS keeps the app's link down.
pub fn send(id: &str, cmd: LinkCommand) -> Result<(), String> {
    choosable(id)?;
    let (nearby, account) = with(|s| {
        (
            s.nearby.iter().any(|n| n.hello.id == id),
            s.account.iter().any(|(d, _)| d.id == id),
        )
    });
    // The network path proves nothing about who is asking, so a device on it
    // refuses what only the account may send: that goes through the server,
    // and only to the account's own devices.
    if !cmd.allowed_nearby() && !account {
        return Err("Only your own devices can be asked that.".into());
    }
    if nearby && cmd.allowed_nearby() && crate::remote::nearby::send(id, cmd.clone()) {
        return Ok(());
    }
    if link::report(LinkReport::Command {
        to: id.to_string(),
        command: cmd.clone(),
    }) {
        return Ok(());
    }
    if !account && !nearby {
        // Not heard of here, but perhaps by the server: the target of a Live
        // Activity outlives this process's list of devices.
        log::info!("devices: {id} is not listed; asking the server");
    }
    let cfg = Config::load().unwrap_or_default();
    let client = crate::helpers::subsonic_client(&cfg).ok_or("not signed in to a server")?;
    let json = serde_json::to_string(&cmd).map_err(|e| e.to_string())?;
    client.koan_command(id, &json).map_err(|e| e.to_string())
}

/// This device's id, as other devices know it.
pub fn this_id() -> Option<String> {
    local().map(|l| l.identity.device_id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        }
    }

    #[test]
    fn a_device_that_stops_answering_sleeps_after_a_heartbeat_and_goes_after_the_grace() {
        let _held = STORE_LOCK.lock();
        crate::config::isolate_config_for_tests();
        with(|s| *s = Store::default());
        let grace = 30 * 60;

        nearby_hello(hello("stranger"), "10.0.0.5:5626");
        nearby_gone("stranger");
        let now = with(|s| s.live["stranger"]);
        let at = |t: i64| list_at(t, grace).into_iter().find(|d| d.id == "stranger");

        let missed_one = at(now + STALE_SECS - 1).expect("still listed");
        assert!(
            missed_one.awake && !missed_one.asleep,
            "one missed heartbeat changes nothing"
        );
        let asleep = at(now + STALE_SECS + 1).expect("listed asleep");
        assert!(asleep.asleep && !asleep.awake && !asleep.wakeable);
        assert_eq!(asleep.last_seen, Some(now));
        assert!(asleep.state.is_none(), "what it was doing is not news");
        assert!(at(now + STALE_SECS + grace).is_some());
        assert!(
            at(now + STALE_SECS + grace + 1).is_none(),
            "dropped after the grace"
        );
        assert_eq!(
            next_change(now, grace),
            Some(now + STALE_SECS),
            "the clock wakes for the next crossing"
        );

        // Heard from again: back at once, with no wait.
        nearby_hello(hello("stranger"), "10.0.0.5:5626");
        let back = at(now + STALE_SECS + 5).unwrap();
        assert!(back.awake && back.nearby && !back.asleep);

        // An asleep stranger cannot be chosen or sent to.
        nearby_gone("stranger");
        with(|s| *s.live.get_mut("stranger").unwrap() -= STALE_SECS + 10);
        assert!(choosable("stranger").is_err());
        assert!(send("stranger", LinkCommand::Pause).is_err());

        // An account device a push can wake stays listed past the grace.
        let mut phone = device("phone", false);
        phone.linked = false;
        phone.last_seen = Some(now - 10 * grace);
        set_account(vec![phone]);
        let phone = list_at(now, grace)
            .into_iter()
            .find(|d| d.id == "phone")
            .unwrap();
        assert!(phone.asleep && phone.wakeable);
        assert_eq!(phone.last_seen, Some(now - 10 * grace));
        assert!(choosable("phone").is_ok());

        // An account device that unlinks without a push token: kept as it
        // was for a heartbeat, then asleep, then dropped.
        set_account(vec![device("mac", false)]);
        set_account(Vec::new());
        let left = with(|s| s.live["mac"]);
        let mac = |t: i64| list_at(t, grace).into_iter().find(|d| d.id == "mac");
        assert!(mac(left + 1).unwrap().awake);
        let asleep = mac(left + STALE_SECS + 1).unwrap();
        assert!(asleep.asleep && !asleep.wakeable && asleep.account);
        assert!(mac(left + STALE_SECS + grace + 1).is_none());
        with(|s| *s = Store::default());
    }

    /// Run a wake with a device that answers in stage `answers_in`, if any:
    /// the stages started, and how it ended.
    fn walk(recent: bool, answers_in: Option<Waking>) -> (Vec<Waking>, Result<Waking, Waking>) {
        let plan = wake_plan(recent);
        let mut started = Vec::new();
        let current = std::cell::RefCell::new(None);
        let outcome = run_wake(
            &plan,
            |stage| {
                started.push(stage.clone());
                *current.borrow_mut() = Some(stage.clone());
                Ok(())
            },
            |_| *current.borrow() == answers_in,
        );
        (started, outcome)
    }

    #[test]
    fn waking_tries_the_network_first_only_when_it_was_there_a_moment_ago() {
        let (tried, _) = walk(true, None);
        assert_eq!(tried, [Waking::Network, Waking::Push, Waking::Notification]);
        let (tried, _) = walk(false, None);
        assert_eq!(tried, [Waking::Push, Waking::Notification]);
    }

    #[test]
    fn a_wake_stops_at_the_stage_that_reached_it() {
        let (tried, outcome) = walk(true, Some(Waking::Network));
        assert_eq!(tried, [Waking::Network], "no push for a phone still awake");
        assert_eq!(outcome, Ok(Waking::Network));
        let (tried, outcome) = walk(false, Some(Waking::Push));
        assert_eq!(
            tried,
            [Waking::Push],
            "no notification once the push worked"
        );
        assert_eq!(outcome, Ok(Waking::Push));
    }

    #[test]
    fn a_push_that_does_not_wake_it_falls_back_to_a_notification_then_says_so() {
        let (tried, outcome) = walk(false, None);
        assert_eq!(tried.last(), Some(&Waking::Notification));
        assert!(matches!(outcome, Err(Waking::Failed(why)) if why.contains("notification")));
        // The stages' times: the notification comes after a short wait on the push.
        let plan = wake_plan(false);
        assert!(plan[0].1 <= Duration::from_secs(8) && plan[0].1 >= Duration::from_secs(5));
    }

    #[test]
    fn a_stage_that_cannot_start_ends_the_wake_with_why() {
        let outcome = run_wake(&wake_plan(false), |_| Err("Not connected".into()), |_| true);
        assert_eq!(outcome, Err(Waking::Failed("Not connected".into())));
    }

    #[test]
    fn a_stranger_asleep_on_the_network_is_never_woken() {
        let _held = STORE_LOCK.lock();
        crate::config::isolate_config_for_tests();
        with(|s| *s = Store::default());
        nearby_hello(hello("stranger"), "10.0.0.5:5626");
        nearby_gone("stranger");
        with(|s| *s.live.get_mut("stranger").unwrap() -= STALE_SECS + 10);
        wake("stranger");
        assert!(
            with(|s| s.waking.is_empty()),
            "not wakeable: no stage starts"
        );
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
            },
            "mac.local:5626",
        );
        nearby_hello(
            LinkHello {
                id: "tv".into(),
                name: "Living room".into(),
                platform: "macos".into(),
                library: Some("elsewhere".into()),
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
        // Past the heartbeat and the grace: out of every list but the target.
        with(|s| *s.live.get_mut("tv").unwrap() -= STALE_SECS + 31 * 60);
        assert_eq!(target(), Some("tv".into()), "kept while out of reach");
        let tv = list().into_iter().find(|d| d.id == "tv").unwrap();
        assert_eq!(tv.name, "Living room");
        assert!(!tv.awake && tv.problem.is_some());

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
