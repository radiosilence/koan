//! The devices this one can play on, and which of them it is controlling.
//!
//! Two sources: the account's other devices, which the server sends down the
//! link, and whatever answers on the local network (`remote::nearby`), which
//! may belong to anyone. A device found both ways is one device, reached over
//! the local network while that connection is up: it is the shorter path.

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
    target: Option<String>,
    version: u64,
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
    /// A target that has gone from both lists is no longer something to
    /// control; the app is back to playing here. Asked only of a fresh list:
    /// a link that dropped says nothing about the devices on it.
    fn let_go_of_lost_target(&mut self) {
        if let Some(t) = &self.target
            && !self.account.iter().any(|(d, _)| d.id == *t)
            && !self.nearby.iter().any(|n| n.hello.id == *t)
        {
            log::info!("devices: {t} has gone; controlling this device again");
            self.target = None;
        }
    }
}

/// Link to the server and open this device to the local network, serving
/// `local` to both. Once per process.
pub fn start(local: Local) {
    if LOCAL.set(local.clone()).is_err() {
        return;
    }
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
pub fn set_linked(linked: bool) {
    changed(|s| s.linked = linked);
}

pub fn set_account(devices: Vec<LinkDevice>) {
    let now = Instant::now();
    changed(|s| {
        s.account = devices.into_iter().map(|d| (d, now)).collect();
        s.let_go_of_lost_target();
    });
}

pub fn nearby_hello(hello: LinkHello) {
    changed(|s| {
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
    changed(|s| {
        s.nearby.retain(|n| n.hello.id != id);
        s.let_go_of_lost_target();
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
    with(|s| s.target.clone())
}

pub fn set_target(id: Option<String>) {
    changed(|s| s.target = id);
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
    fn a_device_found_both_ways_is_one_and_a_lost_target_lets_go() {
        set_account(vec![device("mac", false), device("phone", true)]);
        let listed = list();
        assert_eq!(listed[0].id, "phone", "playing first");
        assert_eq!(listed.len(), 2);

        nearby_hello(LinkHello {
            id: "mac".into(),
            name: "Mac".into(),
            platform: "macos".into(),
            library: None,
        });
        nearby_hello(LinkHello {
            id: "tv".into(),
            name: "Living room".into(),
            platform: "macos".into(),
            library: Some("elsewhere".into()),
        });
        let listed = list();
        assert_eq!(listed.len(), 3);
        let mac = listed.iter().find(|d| d.id == "mac").unwrap();
        assert!(mac.account && mac.nearby && mac.same_library);
        let tv = listed.iter().find(|d| d.id == "tv").unwrap();
        assert!(!tv.account && !tv.same_library);

        set_target(Some("tv".into()));
        assert_eq!(target(), Some("tv".into()));
        nearby_gone("tv");
        assert_eq!(target(), None, "a target that has gone is let go");
        set_account(Vec::new());
        nearby_gone("mac");
        assert!(list().is_empty());
    }
}
