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

use std::sync::OnceLock;
use std::time::Instant;

use parking_lot::Mutex;

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
}

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
    out
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
    changed(|s| {
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
        }
    });
}

pub fn nearby_gone(id: &str) {
    changed(|s| s.nearby.retain(|n| n.hello.id != id));
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
    let ours = link::library_fingerprint(&cfg);
    with(|s| {
        let mut out: Vec<Device> = s
            .account
            .iter()
            .map(|(d, at)| {
                let near = s.nearby.iter().find(|n| n.hello.id == d.id);
                Device {
                    id: d.id.clone(),
                    name: d.name.clone(),
                    platform: d.platform.clone(),
                    account: true,
                    nearby: near.is_some(),
                    awake: d.linked || near.is_some(),
                    same_library: true,
                    state: match near {
                        Some(n) => n.state.clone(),
                        None => d.state.clone(),
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
                same_library: ours.is_some() && n.hello.library == ours,
                state: n.state.clone(),
                heard: n.at,
                problem: None,
            });
        }
        // Announced on this network but not connected: listed with the reason.
        for f in crate::remote::nearby::found() {
            let id =
                f.id.clone()
                    .unwrap_or_else(|| format!("bonjour:{}", f.name));
            if out.iter().any(|d| d.id == id) || f.id.is_some() && f.id == this_id() {
                continue;
            }
            out.push(Device {
                id,
                name: f.name,
                platform: f.platform.unwrap_or_default(),
                account: false,
                nearby: false,
                awake: false,
                same_library: false,
                state: None,
                heard: Instant::now(),
                problem: Some(f.problem.unwrap_or_else(|| "Connecting…".into())),
            });
        }
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
                same_library: true,
                state: None,
                heard: Instant::now(),
                problem: Some("Out of reach".into()),
            });
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
    let (nearby, account) = with(|s| {
        (
            s.nearby.iter().any(|n| n.hello.id == id),
            s.account.iter().any(|(d, _)| d.id == id),
        )
    });
    if nearby && crate::remote::nearby::send(id, cmd.clone()) {
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
        }
    }

    // The store is process-wide; one test walks it through its states rather
    // than several racing each other over it.
    #[test]
    fn a_device_found_both_ways_is_one_and_a_lost_target_is_kept() {
        crate::config::isolate_config_for_tests();
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
        assert!(list().is_empty());
    }
}
