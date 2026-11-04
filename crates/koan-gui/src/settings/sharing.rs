//! This device serving its own library to the person's other devices, as a
//! kōan server: Settings → Server → Shares its library.
//!
//! Whether sharing is on is a setting (`share.enabled`), written through the
//! settings model like every other; this holds what is running. Starts,
//! stops and changes to where invites go reach the engine one at a time, in
//! the order they were asked: turning sharing off while launch is still
//! starting it ends with it off.

use std::collections::VecDeque;
use std::sync::Arc;

use koan_kit::app::{App, Context, SharedString, Task};
use koan_runtime::{InviteTo, KoanEngine, ShareStatus, Sharing as Serving};

use super::model::describe;

/// Told whether an op failed, once it has landed.
pub type Landed = Box<dyn FnOnce(bool, &mut App)>;

enum Op {
    Start(u16, Option<Landed>),
    Stop,
    InviteTo(InviteTo, Option<String>, Option<Landed>),
    /// The addresses read again.
    Refresh,
}

pub struct Sharing {
    engine: Arc<KoanEngine>,
    /// Where the library is served, while it is.
    pub serving: Option<Serving>,
    /// Why the last start failed, until the next start or stop.
    pub failure: Option<SharedString>,
    /// The account the library is shared under, its first admin. `None`
    /// until one is made; sharing cannot start before.
    pub account: Option<String>,
    /// Where invites go, Tailscale, and the addresses Settings shows, as the
    /// engine last worked them out. `None` until first asked.
    pub status: Option<ShareStatus>,
    /// Why the last change to where invites go failed.
    pub funnel_error: Option<SharedString>,
    ops: VecDeque<Op>,
    running: Option<Task<()>>,
}

impl Sharing {
    pub fn new(engine: Arc<KoanEngine>) -> Self {
        Self {
            engine,
            serving: None,
            failure: None,
            account: None,
            status: None,
            funnel_error: None,
            ops: VecDeque::new(),
            running: None,
        }
    }

    /// A start, stop or change is waiting or under way.
    pub fn busy(&self) -> bool {
        self.running.is_some()
    }

    /// Reads the account, and serves again if sharing was left on. What the
    /// app calls as it opens.
    pub fn resume(&mut self, cx: &mut Context<Self>) {
        let engine = self.engine.clone();
        cx.spawn(async move |this, cx| {
            let account = engine.clone().server_admin().await.ok().flatten();
            let settings = engine.settings().await;
            this.update(cx, |sharing, cx| {
                sharing.account = account;
                if settings.share_enabled && sharing.account.is_some() {
                    sharing.start(settings.share_port, None, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Serves on `port`, stopping a server already running first, so a
    /// start that fails leaves nothing serving. `started` hears whether it
    /// failed.
    pub fn start(&mut self, port: u16, started: Option<Landed>, cx: &mut Context<Self>) {
        self.push(Op::Start(port, started), cx);
    }

    pub fn stop(&mut self, cx: &mut Context<Self>) {
        self.push(Op::Stop, cx);
    }

    /// Where invites send phones. `Funnel` publishes through Tailscale
    /// Funnel; `Custom` takes `address`, or keeps the one set. The engine
    /// writes `share.invite_to` and `server.url` itself; `landed` hears
    /// whether it failed.
    pub fn set_invite_to(
        &mut self,
        to: InviteTo,
        address: Option<String>,
        landed: Option<Landed>,
        cx: &mut Context<Self>,
    ) {
        self.push(Op::InviteTo(to, address, landed), cx);
    }

    fn push(&mut self, op: Op, cx: &mut Context<Self>) {
        self.ops.push_back(op);
        if self.running.is_none() {
            self.next(cx);
        }
        cx.notify();
    }

    /// Runs the next op, and the one after when it lands. The task is held
    /// here, so the chain ends with the entity; the app quitting mid-start
    /// leaves the server to the process's end, as `stop_server` allows.
    fn next(&mut self, cx: &mut Context<Self>) {
        let Some(op) = self.ops.pop_front() else {
            self.running = None;
            return;
        };
        let engine = self.engine.clone();
        self.running = Some(cx.spawn(async move |this, cx| {
            match op {
                Op::Start(port, started) => {
                    // The host's name can take a lookup to find.
                    let names = cx.background_executor().spawn(async { names() }).await;
                    let result = engine.clone().start_server(port, names).await;
                    let failed = result.is_err();
                    this.update(cx, |sharing, _| match result {
                        Ok(serving) => {
                            sharing.serving = Some(serving);
                            sharing.failure = None;
                        }
                        Err(e) => {
                            sharing.serving = None;
                            sharing.failure = Some(describe(&e).into());
                        }
                    })
                    .ok();
                    if let Some(started) = started {
                        cx.update(|cx| started(failed, cx));
                    }
                }
                Op::Refresh => {
                    let serving = this.update(cx, |sharing, _| sharing.serving.is_some());
                    if serving.unwrap_or(false) {
                        let now = engine.clone().server_sharing().await;
                        this.update(cx, |sharing, _| sharing.serving = now).ok();
                    }
                }
                Op::Stop => {
                    engine.clone().stop_server().await;
                    this.update(cx, |sharing, _| {
                        sharing.serving = None;
                        sharing.failure = None;
                    })
                    .ok();
                }
                Op::InviteTo(to, address, landed) => {
                    let result = engine.clone().set_invite_to(to, address).await;
                    let failed = result.is_err();
                    this.update(cx, |sharing, _| {
                        sharing.funnel_error = result.err().map(|e| describe(&e).into());
                    })
                    .ok();
                    if let Some(landed) = landed {
                        cx.update(|cx| landed(failed, cx));
                    }
                }
            }
            // A start publishes the Funnel again where it was left on, and
            // may move its address.
            let status = engine.share_status().await;
            this.update(cx, |sharing, cx| {
                sharing.status = Some(status);
                sharing.next(cx);
                cx.notify();
            })
            .ok();
        }));
    }

    /// Reads the addresses again: they move with the network, so the page
    /// asks each time it opens.
    /// In turn with the starts, stops and changes, so it never reads before
    /// one and lands after it.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.push(Op::Refresh, cx);
    }

    /// Makes the account the library is shared under. `landed` hears why
    /// not, if it failed, once this entity is no longer being updated, so it
    /// may start sharing.
    pub fn create_account(
        &mut self,
        username: String,
        password: String,
        cx: &mut Context<Self>,
        landed: impl FnOnce(Result<(), String>, &mut App) + 'static,
    ) {
        let engine = self.engine.clone();
        cx.spawn(async move |this, cx| {
            let made = engine
                .clone()
                .create_first_server_account(username, password)
                .await;
            let account = match &made {
                Ok(()) => engine.server_admin().await.ok().flatten(),
                Err(_) => None,
            };
            let updated = this.update(cx, |sharing, cx| {
                if made.is_ok() {
                    sharing.account = account;
                }
                cx.notify();
            });
            if updated.is_ok() {
                cx.update(|cx| landed(made.map_err(|e| describe(&e)), cx));
            }
        })
        .detach();
    }
}

/// What the server answers to besides IP addresses: this device's name on
/// the local network, which a phone there can reach it by. On macOS that is
/// the Bonjour name (`scutil --get LocalHostName`, what Sharing settings
/// show); on Linux the host name, which Avahi publishes under `.local`.
/// Blocking: called off the main thread.
fn names() -> Vec<String> {
    #[cfg(target_os = "macos")]
    let name = std::process::Command::new("/usr/sbin/scutil")
        .args(["--get", "LocalHostName"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok());
    #[cfg(not(target_os = "macos"))]
    let name = std::fs::read_to_string("/proc/sys/kernel/hostname").ok();
    // Avahi publishes the first label of a host name that has a domain.
    name.and_then(|name| name.trim().split('.').next().map(str::to_owned))
        .filter(|name| !name.is_empty())
        .map(|name| vec![format!("{name}.local")])
        .unwrap_or_default()
}
