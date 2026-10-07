//! koan apps on the same network seeing and controlling each other, with no
//! server between them and whoever they are signed in as.
//!
//! Each app that is discoverable listens on `devices.port` and announces
//! itself over Bonjour as `_koan._tcp`. Each app browses for the others and
//! connects to every one it finds, and to any address in `devices.addresses`
//! (a tailnet carries no Bonjour). A connection speaks the link's language:
//! the listening end introduces itself (`LinkReport::Hello`) and reports its
//! state as it changes; the connecting end sends `LinkCommand`s, less the ones
//! that touch the library (`LinkCommand::allowed_nearby`), since anyone on the
//! network can connect.
//!
//! A peer that proves it is one of this account's devices, or one shared with
//! it, is trusted as that rather than by the connection it came over: the
//! listener's `Hello` carries a nonce, and the two ends sign each other's
//! (`remote::proof`). After that every command is signed, and every report
//! back when both ends know to. A peer that proves nothing is a stranger or,
//! under Full control, a nearby device, as before.

use std::collections::HashMap;
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::config::{Config, DEVICES_PORT};
use crate::remote::acks::{AckOutcome, Envelope};
use crate::remote::devices;
use crate::remote::link::{CommandSource, LinkCommand, LinkHello, LinkReport, LinkState, Local};
use crate::remote::proof::{self, Peer, Proven};
use crate::remote::wire::{self, Waker};

pub const SERVICE: &str = "_koan._tcp";

/// The proof's frames, beside the link's messages on a nearby connection.
/// Tagged apart from every `LinkCommand` and `LinkReport`, and sent only to a
/// peer whose part in the handshake shows it knows them: the listener's
/// `Hello` nonce, the dialler's `NearbyAuth`.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum ProofFrame {
    /// The dialler: who it is, its nonce, and its signature over both nonces.
    #[serde(rename = "nearbyAuth")]
    Auth {
        id: String,
        nonce: String,
        sig: String,
    },
    /// The listener's answer: whether it took the dialler for the account's
    /// or a shared device, and its own signature, when it has a key.
    #[serde(rename = "nearbyProof")]
    Proof {
        verified: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sig: Option<String>,
    },
    /// A command from a proven dialler, as JSON, signed for this session.
    #[serde(rename = "nearbySigned")]
    Signed {
        seq: u64,
        sig: String,
        command: String,
    },
    /// A report from a listener that signs them, as JSON, signed for this
    /// session.
    #[serde(rename = "nearbySignedReport")]
    SignedReport {
        seq: u64,
        sig: String,
        report: String,
    },
}

fn frame(f: &ProofFrame) -> Option<String> {
    serde_json::to_string(f).ok()
}

/// A connection this app made to another, once it has said who it is.
struct Conn {
    outbox: Vec<Envelope>,
    waker: Arc<Waker>,
    /// Who the listener proved to be, if it proved anything.
    proven: Option<proof::Peer>,
}

static CONNS: Mutex<Option<HashMap<String, Conn>>> = Mutex::new(None);

/// What is running, so a change of settings can stop and start it.
struct Running {
    local: Local,
    listener: Option<Arc<Stop>>,
    /// One per device being dialled: `id:<device id>` when its id is known
    /// (announced, or remembered from a previous run), `bonjour:<name>` when
    /// an announcement carried none, or the address as typed.
    dialers: HashMap<String, Dialer>,
    /// `devices.nearby` was off when last applied, so turning it on knows to
    /// start looking again.
    off: bool,
}

struct Dialer {
    stop: Arc<Stop>,
    /// Where it is dialled; moved by a fresh announcement, which only
    /// Bonjour makes.
    #[cfg_attr(not(target_vendor = "apple"), allow(dead_code))]
    addr: Arc<Mutex<String>>,
    /// Set to have this dialer try again at once, its backoff started over.
    redial: Arc<AtomicBool>,
}

impl Dialer {
    fn redial(&self) {
        self.redial.store(true, Ordering::Relaxed);
        self.stop.waker.wake();
    }
}

static RUNNING: Mutex<Option<Running>> = Mutex::new(None);
static PORT: Mutex<Option<u16>> = Mutex::new(None);

/// A device announced on this network, remembered from a previous run, or
/// listed by address, and why it is not connected when it is not.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    /// Its name, or the address as typed.
    pub name: String,
    /// The Bonjour service it was announced as, for when it is withdrawn.
    pub bonjour: Option<String>,
    /// Its device id, from the announcement.
    pub id: Option<String>,
    pub platform: Option<String>,
    /// The server it is signed in to, as it announces it: what a device that
    /// is not signed in yet offers to sign in to.
    pub server: Option<String>,
    pub problem: Option<String>,
}

/// Keyed as the dialers are.
static FOUND: Mutex<Vec<(String, Found)>> = Mutex::new(Vec::new());

/// The system refused this app the local network: the person has not allowed it.
static BLOCKED: AtomicBool = AtomicBool::new(false);

/// Every device announced here or listed by address that is not this one.
pub fn found() -> Vec<Found> {
    FOUND.lock().iter().map(|(_, f)| f.clone()).collect()
}

/// The servers devices on this network are signed in to, each with the names
/// of the devices that announced it, in the order they were found.
pub fn servers() -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for (_, f) in FOUND.lock().iter() {
        let Some(server) = f.server.as_deref().filter(|s| is_server(s)) else {
            continue;
        };
        match out.iter_mut().find(|(s, _)| s == server) {
            Some((_, names)) => names.push(f.name.clone()),
            None => out.push((server.to_string(), vec![f.name.clone()])),
        }
    }
    out
}

/// An address worth offering as a server: http or https, nothing more than an
/// address, and not this machine's own. Checked on both sides: anyone on the
/// network can announce anything, and what this device announces is heard by
/// everyone on it.
fn is_server(s: &str) -> bool {
    url::Url::parse(s).is_ok_and(|u| {
        matches!(u.scheme(), "http" | "https")
            && u.username().is_empty()
            && u.password().is_none()
            && u.query().is_none()
            && u.fragment().is_none()
            && match u.host() {
                Some(url::Host::Domain(d)) => {
                    let d = d.trim_end_matches('.').to_ascii_lowercase();
                    d != "localhost" && !d.ends_with(".localhost")
                }
                Some(url::Host::Ipv4(ip)) => !ip.is_loopback() && !ip.is_unspecified(),
                Some(url::Host::Ipv6(ip)) => !ip.is_loopback() && !ip.is_unspecified(),
                None => false,
            }
    })
}

/// The longest a TXT entry can be. An address that does not fit is not
/// announced: cut short, it could still read as a different, valid address.
#[cfg(any(target_vendor = "apple", test))]
const TXT_ENTRY_MAX: usize = 255;

/// What this device would announce for the server it is signed in to: the
/// address if it is one to offer and fits whole in the announcement, and
/// nothing otherwise.
#[cfg(any(target_vendor = "apple", test))]
fn announceable(url: &str) -> Option<String> {
    let url = url.trim().trim_end_matches('/');
    (is_server(url) && "server=".len() + url.len() <= TXT_ENTRY_MAX).then(|| url.to_string())
}

/// The server this device announces: the one it is signed in to, without the
/// account, or nothing while it is signed out.
#[cfg(target_vendor = "apple")]
fn announced_server() -> String {
    let cfg = Config::cached();
    if crate::helpers::remote_credential(&cfg).is_none() {
        return String::new();
    }
    announceable(&cfg.remote.url).unwrap_or_default()
}

/// This device's announcement, while it is listening.
#[cfg(target_vendor = "apple")]
static ADVERT: Mutex<Option<bonjour::Advert>> = Mutex::new(None);

/// Announce this device again, after it signed in or out: its announcement
/// names the server it is signed in to.
pub fn readvertise() {
    #[cfg(target_vendor = "apple")]
    {
        let Some(port) = *PORT.lock() else { return };
        let identity = match RUNNING.lock().as_ref() {
            Some(r) if r.listener.is_some() => r.local.identity.clone(),
            _ => return,
        };
        let mut advert = ADVERT.lock();
        if advert.is_none() {
            return;
        }
        // Withdrawn first: the responder would rename a second registration
        // under the same name rather than replace the first.
        *advert = None;
        let server = announced_server();
        *advert = bonjour::advertise(port, &identity, &server);
        // Silent until the next launch would be worse than announcing without
        // the server.
        if advert.is_none() && !server.is_empty() {
            log::warn!("nearby: cannot announce the server; announcing this device without it");
            *advert = bonjour::advertise(port, &identity, "");
        }
    }
}

/// Whether the system is keeping this app off the local network. The fix is
/// the person's: Privacy & Security → Local Network, in Settings on iOS and
/// System Settings on macOS.
pub fn local_network_blocked() -> bool {
    BLOCKED.load(Ordering::Relaxed)
}

fn set_blocked(blocked: bool) {
    if BLOCKED.swap(blocked, Ordering::Relaxed) != blocked {
        if blocked {
            log::warn!("nearby: the local network is blocked for this app");
        }
        devices::touch();
    }
}

fn note(key: &str, problem: Option<String>) {
    let mut found = FOUND.lock();
    if let Some((_, f)) = found.iter_mut().find(|(k, _)| k == key)
        && f.problem != problem
    {
        f.problem = problem;
        drop(found);
        devices::touch();
    }
}

/// Whether a failed connection is iOS keeping this app off the local network,
/// which it reports as an unreachable host. On macOS that error means only
/// that the address is not reachable from here: a sleeping phone, a stale
/// link-local address.
fn locally_refused(error: &str) -> bool {
    let e = error.to_lowercase();
    cfg!(any(target_os = "ios", target_os = "tvos"))
        && (e.contains("no route to host") || e.contains("network is unreachable"))
}

/// What a failed connection means to someone looking at the picker.
fn explain(error: &str) -> String {
    let e = error.to_lowercase();
    if locally_refused(error) {
        "Blocked: allow Local Network for kōan in Settings".into()
    } else if e.contains("no route to host") || e.contains("network is unreachable") {
        "Not reachable from this network".into()
    } else if e.contains("refused") {
        "Not accepting connections. Is kōan open there, and discoverable?".into()
    } else if e.contains("timed out") || e.contains("would block") {
        "Not answering on this network".into()
    } else if e.contains("lookup") || e.contains("nodename") || e.contains("not known") {
        "Address not found".into()
    } else {
        format!("Cannot connect: {error}")
    }
}

/// The app is in front again: whatever iOS closed while it was suspended is
/// opened again, and every device is dialled now rather than when its backoff
/// runs out.
pub fn refresh() {
    let listener = RUNNING.lock().as_mut().and_then(|r| r.listener.take());
    if let Some(stop) = listener {
        stop.stop();
    }
    reconfigure();
    redial_all();
    #[cfg(target_vendor = "apple")]
    bonjour::restart();
}

/// Dial every device now, rather than when its backoff runs out: the first
/// thing tried when waking one, which may not be suspended yet.
pub fn dial_now() {
    redial_all();
}

/// Dial every device now rather than when its backoff runs out.
fn redial_all() {
    if let Some(r) = RUNNING.lock().as_ref() {
        for d in r.dialers.values() {
            d.redial();
        }
    }
}

/// Dial `id` now by every other way it is known, `except` the dialer asking.
/// Only that device: one that comes and goes must not set every dialer here
/// going again, or it is dialled once for each time the other one flaps.
fn redial_device(id: &str, except: &str) {
    let keys: Vec<String> = FOUND
        .lock()
        .iter()
        .filter(|(k, f)| k != except && f.id.as_deref() == Some(id))
        .map(|(k, _)| k.clone())
        .collect();
    if let Some(r) = RUNNING.lock().as_ref() {
        for key in keys {
            if let Some(d) = r.dialers.get(&key) {
                d.redial();
            }
        }
    }
}

/// A flag, and a way to interrupt every thread that watches it.
struct Stop {
    flag: AtomicBool,
    waker: Arc<Waker>,
    /// The connections running under it, each waiting on its own waker.
    others: Mutex<Vec<Weak<Waker>>>,
}

impl Stop {
    fn new() -> Option<Arc<Self>> {
        Some(Arc::new(Self {
            flag: AtomicBool::new(false),
            waker: Waker::new().ok()?,
            others: Mutex::new(Vec::new()),
        }))
    }

    fn stop(&self) {
        self.flag.store(true, Ordering::Relaxed);
        self.waker.wake();
        for w in self.others.lock().iter().filter_map(Weak::upgrade) {
            w.wake();
        }
    }

    /// Ring `waker` too when stopped.
    fn also_wake(&self, waker: &Arc<Waker>) {
        let mut others = self.others.lock();
        others.retain(|w| w.strong_count() > 0);
        others.push(Arc::downgrade(waker));
    }

    fn stopped(&self) -> bool {
        self.flag.load(Ordering::Relaxed)
    }
}

/// Open this device to the network as the config says, and look for others:
/// first at the addresses devices were last reached at, which answers before
/// Bonjour has said anything, then wherever Bonjour finds them.
pub fn start(local: Local) {
    *RUNNING.lock() = Some(Running {
        local,
        listener: None,
        dialers: HashMap::new(),
        off: false,
    });
    // Paused before it starts when the network is off, so it never browses.
    #[cfg(target_vendor = "apple")]
    if !enabled() {
        bonjour::pause();
    }
    reconfigure();
    dial_remembered();
    #[cfg(target_vendor = "apple")]
    std::thread::Builder::new()
        .name("koan-bonjour".into())
        .spawn(bonjour::browse_forever)
        .expect("failed to spawn the Bonjour thread");
}

/// `devices.nearby`: whether this device takes part in the local network.
fn enabled() -> bool {
    Config::load().map(|c| c.devices.nearby).unwrap_or(true)
}

/// Stop looking for other devices, and hang up on them: a phone in the
/// background with nothing to control. What `wake` starts again. Listening is
/// `reconfigure`'s, and goes by `quiet::findable`.
pub fn suspend() {
    if let Some(r) = RUNNING.lock().as_mut() {
        for (_, d) in r.dialers.drain() {
            d.stop.stop();
        }
    }
    FOUND.lock().clear();
    devices::touch();
    #[cfg(target_vendor = "apple")]
    bonjour::pause();
}

/// Look for other devices again, as at startup.
pub fn wake() {
    if !enabled() {
        return;
    }
    reconfigure();
    dial_remembered();
    #[cfg(target_vendor = "apple")]
    bonjour::restart();
}

/// Dial the devices reached before, at the addresses they were reached at,
/// which answers before Bonjour has said anything.
fn dial_remembered() {
    if !crate::quiet::awake() || !enabled() {
        return;
    }
    if let Some(r) = RUNNING.lock().as_mut() {
        for seen in devices::remembered_nearby() {
            let key = format!("id:{}", seen.id);
            if r.dialers.contains_key(&key) {
                continue;
            }
            FOUND.lock().push((
                key.clone(),
                Found {
                    name: seen.name,
                    bonjour: None,
                    id: Some(seen.id),
                    platform: Some(seen.platform),
                    server: None,
                    problem: None,
                },
            ));
            spawn_dialer(r, key, seen.addr);
        }
    }
}

/// Apply `devices.*` from the config as it is now: listen or stop, and dial
/// the addresses listed. For a settings screen that has just written them.
pub fn reconfigure() {
    let cfg = Config::load().unwrap_or_default();
    let mut running = RUNNING.lock();
    let Some(r) = running.as_mut() else { return };
    // Off: no listener, no dialers, no browsing, until it is turned on again.
    if !cfg.devices.nearby {
        if let Some(stop) = r.listener.take() {
            stop.stop();
        }
        for (_, d) in r.dialers.drain() {
            d.stop.stop();
        }
        FOUND.lock().clear();
        if !r.off {
            log::info!("nearby: off (devices.nearby = false)");
            r.off = true;
            devices::touch();
            #[cfg(target_vendor = "apple")]
            bonjour::pause();
        }
        return;
    }
    let turned_on = std::mem::replace(&mut r.off, false);
    match (
        &r.listener,
        cfg.devices.discoverable && crate::quiet::findable(),
    ) {
        (None, true) => {
            if let Some(stop) = Stop::new() {
                let (local, port, s) = (r.local.clone(), cfg.devices.port, stop.clone());
                std::thread::Builder::new()
                    .name("koan-nearby".into())
                    .spawn(move || listen(local, port, s))
                    .expect("failed to spawn the listener");
                r.listener = Some(stop);
            }
        }
        (Some(stop), false) => {
            stop.stop();
            r.listener = None;
        }
        _ => {}
    }
    let wanted: Vec<String> = cfg
        .devices
        .addresses
        .iter()
        .map(|a| a.trim().to_string())
        .filter(|a| !a.is_empty())
        .collect();
    r.dialers.retain(|key, d| {
        let keep = key.starts_with("bonjour:") || key.starts_with("id:") || wanted.contains(key);
        if !keep {
            d.stop.stop();
            FOUND.lock().retain(|(k, _)| k != key);
        }
        keep
    });
    if !crate::quiet::awake() {
        return;
    }
    for addr in wanted {
        if !r.dialers.contains_key(&addr) {
            spawn_dialer(r, addr.clone(), addr);
        }
    }
    if turned_on {
        log::info!("nearby: on");
        drop(running);
        dial_remembered();
        #[cfg(target_vendor = "apple")]
        bonjour::restart();
    }
}

/// The port this device is listening on, while it is.
pub fn listening_port() -> Option<u16> {
    *PORT.lock()
}

/// Queue `cmd` for the device `id` on its local connection. False when there
/// is none, or when `id` is one of the account's devices or one shared with
/// it and the listener has not proved it is that device.
pub fn send(id: &str, cmd: LinkCommand) -> bool {
    send_envelope(id, cmd.into())
}

/// As `send`, asking `id` to answer under `ack`: see `remote::acks`.
pub fn send_acked(id: &str, cmd: LinkCommand, ack: u64) -> bool {
    send_envelope(
        id,
        Envelope {
            command: cmd,
            ack: Some(ack),
        },
    )
}

fn send_envelope(id: &str, envelope: Envelope) -> bool {
    // Asked before the connections are held: the device store is not to be
    // locked under them.
    queue(id, devices::listed_owner(id), envelope)
}

/// `send_envelope`, for a device the server lists as `listed`.
fn queue(id: &str, listed: Option<Option<String>>, envelope: Envelope) -> bool {
    let mut conns = CONNS.lock();
    let Some(conn) = conns.as_mut().and_then(|c| c.get_mut(id)) else {
        return false;
    };
    // Anyone on the network can say it is the account's phone. What is meant
    // for one of the account's devices, or one shared with it, goes only to a
    // listener that proved it is that device; otherwise by the link.
    if !proven_as_listed(listed, conn.proven.as_ref()) {
        log::info!("nearby: {id} has not proved it is that device; not sent here");
        return false;
    }
    conn.outbox.push(envelope);
    conn.waker.wake();
    true
}

// --- The listening end ------------------------------------------------------

fn listen(local: Local, port: u16, stop: Arc<Stop>) {
    // A listener that stops accepting (iOS closes it while the app is
    // suspended) is replaced rather than retried.
    while !stop.stopped() {
        if let Err(e) = listen_once(&local, port, &stop) {
            log::warn!("nearby: {e}; listening again");
            let mut fds = [libc::pollfd {
                fd: stop.waker_fd(),
                events: libc::POLLIN,
                revents: 0,
            }];
            // SAFETY: a live array of the length given.
            unsafe { libc::poll(fds.as_mut_ptr(), 1, 1000) };
        }
    }
    *PORT.lock() = None;
    devices::touch();
    log::info!("nearby: stopped listening");
}

fn bind(port: u16) -> std::io::Result<TcpListener> {
    TcpListener::bind(("::", port)).or_else(|_| TcpListener::bind(("0.0.0.0", port)))
}

/// Listen until stopped or until accepting fails.
fn listen_once(local: &Local, port: u16, stop: &Arc<Stop>) -> Result<(), String> {
    // The listener this replaces may take a moment to let the port go.
    let mut listener = bind(port);
    for _ in 0..20 {
        if listener.is_ok() || stop.stopped() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
        listener = bind(port);
    }
    // Another process on the port: an ephemeral one still works on this
    // network, where it is announced; only a typed address would miss it.
    let listener = listener
        .or_else(|e| {
            log::warn!("nearby: port {port} is taken ({e}); listening elsewhere");
            bind(0)
        })
        .map_err(|e| format!("cannot listen: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    *PORT.lock() = Some(port);
    devices::touch();
    log::info!("nearby: listening on {port}");
    #[cfg(target_vendor = "apple")]
    let _advert = Announced::new(port, &local.identity);

    while !stop.stopped() {
        match listener.accept() {
            Ok((stream, addr)) => {
                let (local, stop) = (local.clone(), stop.clone());
                let _ = std::thread::Builder::new()
                    .name("koan-nearby-peer".into())
                    .spawn(move || {
                        if let Err(e) = serve(stream, &local, &stop) {
                            log::info!("nearby: {addr}: {e}");
                        }
                    });
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                let mut fds = [
                    libc::pollfd {
                        fd: listener.as_raw_fd(),
                        events: libc::POLLIN,
                        revents: 0,
                    },
                    libc::pollfd {
                        fd: stop.waker_fd(),
                        events: libc::POLLIN,
                        revents: 0,
                    },
                ];
                // SAFETY: a live array of the length given.
                unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) };
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(format!("accept: {e}")),
        }
    }
    Ok(())
}

impl Stop {
    fn waker_fd(&self) -> i32 {
        self.waker.read_fd()
    }

    fn drain(&self) {
        self.waker.drain();
    }
}

/// Serve one device that connected to this one.
fn serve(stream: TcpStream, local: &Local, stop: &Arc<Stop>) -> Result<(), String> {
    // Apple's accept hands back the listener's non-blocking mode, under which
    // a request that arrives a moment after the connection fails the handshake.
    stream.set_nonblocking(false).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| e.to_string())?;
    let mut socket = tungstenite::accept(stream).map_err(|e| e.to_string())?;
    let fd = socket.get_ref().as_raw_fd();
    socket
        .get_ref()
        .set_nonblocking(true)
        .map_err(|e| e.to_string())?;
    let _ = socket.get_ref().set_nodelay(true);
    let waker = Waker::new().map_err(|e| e.to_string())?;
    wire::wake_on_engine_change(&waker);
    stop.also_wake(&waker);
    let mut session = Serving {
        local,
        stop,
        greeted: false,
        sent: None,
        waker: waker.clone(),
        levels: None,
        answers: Default::default(),
        nonce: proof::nonce(),
        answered: false,
        proven: None,
        reports: None,
        pending: Vec::new(),
    };
    wire::drive(&mut socket, fd, &waker, &mut session)
}

struct Serving<'a> {
    local: &'a Local,
    stop: &'a Arc<Stop>,
    greeted: bool,
    sent: Option<(LinkState, Instant)>,
    waker: Arc<Waker>,
    /// Set while the device at the other end has bars on screen for this
    /// one. Dropped with the connection, which stops the frames.
    levels: Option<crate::remote::levels::Watch>,
    /// Answers to commands sent with an id, waiting to go back down this
    /// connection; filled from whichever thread finishes each.
    answers: Arc<Mutex<Vec<LinkReport>>>,
    /// What the dialler is asked to sign over: `None` asks for no proof, as
    /// when the system has no random bytes to give.
    nonce: Option<String>,
    /// Its `NearbyAuth` has been answered; a second is ignored.
    answered: bool,
    /// The dialler, once proven, and the session its commands are signed for.
    proven: Option<(Proven, proof::Session)>,
    /// The session this end's reports are signed for, once the handshake
    /// says both ends sign them.
    reports: Option<proof::Session>,
    /// Proof frames to send.
    pending: Vec<String>,
}

impl Serving<'_> {
    fn proof(&mut self, frame_in: ProofFrame) {
        match frame_in {
            ProofFrame::Auth { id, nonce, sig } => {
                if std::mem::replace(&mut self.answered, true) {
                    return;
                }
                let Some(listen_nonce) = self.nonce.clone() else {
                    return;
                };
                let me = &self.local.identity.device_id;
                let proven = proof::verify_dial(me, &id, &listen_nonce, &nonce, &sig);
                let verified = proven.is_some();
                match &proven {
                    Some(p) => log::info!("nearby: {id} proved itself: {:?}", p.peer),
                    None => log::info!("nearby: {id} proved nothing; trusted as the network is"),
                }
                self.proven =
                    proven.map(|p| (p, proof::Session::new(me, &id, &listen_nonce, &nonce)));
                let sig = proof::sign_listen(&id, me, &nonce, &listen_nonce, verified);
                if sig.is_some() && proof::signs_reports(&listen_nonce, &nonce) {
                    self.reports = Some(proof::Session::reports(me, &id, &listen_nonce, &nonce));
                    // Sent again, signed: a state slipped in ahead of the
                    // handshake would otherwise stand until the next change.
                    self.sent = None;
                }
                self.pending
                    .extend(frame(&ProofFrame::Proof { verified, sig }));
            }
            ProofFrame::Signed { seq, sig, command } => {
                let Some((by, session)) = self.proven.as_mut() else {
                    log::warn!("nearby: a signed command from a peer that proved nothing; refused");
                    return;
                };
                if !session.accept(by, seq, &sig, &command) {
                    log::warn!("nearby: a command not signed for this connection; refused");
                    return;
                }
                let envelope = match serde_json::from_str::<Envelope>(&command) {
                    Ok(envelope) => envelope,
                    Err(e) => return log::warn!("nearby: not a command ({e}): {command}"),
                };
                let peer = by.peer.clone();
                self.run(envelope, &peer);
            }
            ProofFrame::Proof { .. } | ProofFrame::SignedReport { .. } => {}
        }
    }

    /// A command from a proven peer: from one of the account's devices,
    /// what one device may have another do, with the account's powers; from a
    /// shared device, the playback set, as that account's request.
    fn run(&mut self, envelope: Envelope, peer: &Peer) {
        let source_of = |cmd: &LinkCommand| match peer {
            Peer::Own if cmd.relayable() => Some(CommandSource::Account),
            Peer::Shared(_) if cmd.allowed_playback() => Some(CommandSource::Shared),
            _ => None,
        };
        let admitted = admit(envelope, source_of, self.answer());
        self.dispatch(admitted);
    }

    /// Where answers to commands sent with an id go: back down this
    /// connection, from whichever thread finishes each.
    fn answer(&self) -> impl FnOnce(u64, AckOutcome) + Send + 'static {
        let (answers, waker) = (self.answers.clone(), self.waker.clone());
        move |ack, outcome| {
            answers.lock().push(LinkReport::Ack { ack, outcome });
            waker.wake();
        }
    }

    fn dispatch(&mut self, admitted: Admitted) {
        match admitted {
            Admitted::Levels(on, pending) => {
                self.levels = on.then(|| crate::remote::levels::feed().watch(&self.waker));
                if let Some(pending) = pending {
                    pending.finish(AckOutcome::Done);
                }
            }
            Admitted::Command(cmd, source, pending) => {
                (self.local.on_command)(cmd, source, pending)
            }
            Admitted::Neither => {}
        }
    }
}

impl wire::Session for Serving<'_> {
    fn outgoing(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.greeted {
            let cfg = Config::load().unwrap_or_default();
            let id = &self.local.identity;
            out.push(LinkReport::Hello(LinkHello {
                id: id.device_id.clone(),
                name: id.name.clone(),
                platform: id.platform.clone(),
                library: crate::remote::link::library_fingerprint(&cfg),
                acks: true,
                nonce: self.nonce.clone(),
            }));
            self.greeted = true;
        }
        out.append(&mut self.answers.lock());
        let now = for_the_network((self.local.state)(), full_control());
        if self
            .sent
            .as_ref()
            .is_none_or(|(s, at)| now.differs(s, at.elapsed()))
        {
            out.push(LinkReport::State(now.clone()));
            self.sent = Some((now, Instant::now()));
        }
        if let Some(f) = self.levels.as_mut().and_then(|w| w.take()) {
            out.push(LinkReport::Levels { f });
        }
        // The proof goes first: the dialler checks what follows against it.
        let mut texts = std::mem::take(&mut self.pending);
        for report in out {
            let Ok(json) = serde_json::to_string(&report) else {
                continue;
            };
            match &mut self.reports {
                Some(session) => match session.sign(&json) {
                    Some((seq, sig)) => texts.extend(frame(&ProofFrame::SignedReport {
                        seq,
                        sig,
                        report: json,
                    })),
                    // Signed out since: the dialler refuses anything unsigned.
                    None => log::warn!("nearby: no key to sign a report with; dropped"),
                },
                None => texts.push(json),
            }
        }
        texts
    }

    fn incoming(&mut self, text: &str) {
        if let Ok(f) = serde_json::from_str::<ProofFrame>(text) {
            return self.proof(f);
        }
        let envelope = match serde_json::from_str::<Envelope>(text) {
            Ok(envelope) => envelope,
            Err(e) => {
                log::warn!("nearby: not a command ({e}): {text}");
                return;
            }
        };
        // A proven peer signs everything; an unsigned command on its
        // connection is one slipped into the stream.
        if self.proven.is_some() {
            log::warn!(
                "nearby: an unsigned command on a proven connection; refused: {:?}",
                envelope.command
            );
            return;
        }
        let full = full_control();
        let admitted = admit(envelope, |cmd| cmd.from_the_network(full), self.answer());
        self.dispatch(admitted);
    }

    fn done(&self) -> bool {
        self.stop.stopped()
    }
}

/// What a command from the network comes to here.
enum Admitted {
    Levels(bool, Option<crate::remote::acks::Pending>),
    Command(
        LinkCommand,
        crate::remote::link::CommandSource,
        Option<crate::remote::acks::Pending>,
    ),
    /// Refused, or a repeat already answered.
    Neither,
}

/// Admit a command from the network: checked against what this device lets
/// the network do (`full` control or a stranger's set) before its id is
/// taken, so a refused command answers `refused` and leaves the id free. A
/// stranger who has seen one of this person's ids then cannot spend it ahead
/// of the real command.
fn admit(
    envelope: Envelope,
    source_of: impl FnOnce(&LinkCommand) -> Option<crate::remote::link::CommandSource>,
    answer: impl FnOnce(u64, AckOutcome) + Send + 'static,
) -> Admitted {
    let source = match &envelope.command {
        LinkCommand::WatchLevels { .. } => None,
        cmd => match source_of(cmd) {
            Some(source) => Some(source),
            None => {
                log::warn!("nearby: refused {cmd:?}");
                if let Some(ack) = envelope.ack {
                    answer(
                        ack,
                        AckOutcome::Refused {
                            reason: "not allowed from this network".into(),
                        },
                    );
                }
                return Admitted::Neither;
            }
        },
    };
    let Some((command, pending)) = crate::remote::acks::take(envelope, answer) else {
        return Admitted::Neither;
    };
    match (command, source) {
        (LinkCommand::WatchLevels { on }, _) => Admitted::Levels(on, pending),
        (cmd, Some(source)) => Admitted::Command(cmd, source, pending),
        (_, None) => Admitted::Neither,
    }
}

/// Whether devices on the network may choose this one's output, preset and
/// volume and move its music anywhere, rather than only play: this device's
/// setting, Full control unless set otherwise.
fn full_control() -> bool {
    crate::config::Config::cached().devices.nearby_control == crate::config::NearbyControl::Full
}

/// What a device on the network, which may belong to anyone, is told: what is
/// playing and the queue, and under `full` control the outputs too, which it
/// may then choose from.
fn for_the_network(state: LinkState, full: bool) -> LinkState {
    if full {
        return state;
    }
    LinkState {
        outputs: None,
        ..state
    }
}

// --- The connecting end -----------------------------------------------------

const RETRY_MIN: Duration = Duration::from_secs(2);
const RETRY_MAX: Duration = Duration::from_secs(60);
const SERVED_LONG_ENOUGH: Duration = Duration::from_secs(30);

fn spawn_dialer(r: &mut Running, key: String, addr: String) {
    let Some(stop) = Stop::new() else { return };
    let addr = Arc::new(Mutex::new(addr));
    let redial = Arc::new(AtomicBool::new(false));
    r.dialers.insert(
        key.clone(),
        Dialer {
            stop: stop.clone(),
            addr: addr.clone(),
            redial: redial.clone(),
        },
    );
    {
        let mut found = FOUND.lock();
        if !found.iter().any(|(k, _)| *k == key) {
            // Typed in: listed under the address until it says who it is.
            found.push((
                key.clone(),
                Found {
                    name: addr.lock().clone(),
                    bonjour: None,
                    id: None,
                    platform: None,
                    server: None,
                    problem: None,
                },
            ));
        }
    }
    devices::touch();
    let _ = std::thread::Builder::new()
        .name("koan-nearby-dial".into())
        .spawn(move || dial(key, addr, stop, redial));
}

/// Stay connected to `addr` until stopped, or until it turns out to be this
/// device.
fn dial(key: String, at: Arc<Mutex<String>>, stop: Arc<Stop>, redial: Arc<AtomicBool>) {
    let mut wait = RETRY_MIN;
    while !stop.stopped() {
        let addr = at.lock().clone();
        let mut served = false;
        let started = Instant::now();
        match connect(&addr) {
            Ok(mut socket) => {
                let fd = socket.get_ref().as_raw_fd();
                let Ok(waker) = Waker::new() else { return };
                stop.also_wake(&waker);
                let mut session = Controlling {
                    stop: &stop,
                    waker: waker.clone(),
                    key: &key,
                    addr: &addr,
                    id: None,
                    this_device: false,
                    duplicate: false,
                    handshake: Handshake::Plain,
                    proven: None,
                    reports: None,
                    pending: Vec::new(),
                };
                let result = wire::drive(&mut socket, fd, &waker, &mut session);
                served = session
                    .id
                    .take()
                    .inspect(|id| {
                        if let Some(conns) = CONNS.lock().as_mut() {
                            conns.remove(id);
                        }
                        devices::nearby_gone(id);
                        // A dialer that found this device already served has been
                        // backing off; it is wanted now.
                        redial_device(id, &key);
                    })
                    .is_some()
                    && started.elapsed() >= SERVED_LONG_ENOUGH;
                if session.this_device {
                    FOUND.lock().retain(|(k, _)| *k != key);
                    devices::touch();
                    return;
                }
                if let Err(e) = result {
                    log::info!("nearby: {addr}: {e}");
                }
            }
            Err(e) => {
                log::debug!("nearby: {addr}: {e}");
                if locally_refused(&e) {
                    set_blocked(true);
                }
                note(&key, Some(explain(&e)));
            }
        }
        let mut fds = [libc::pollfd {
            fd: stop.waker_fd(),
            events: libc::POLLIN,
            revents: 0,
        }];
        // SAFETY: a live array of the length given.
        unsafe { libc::poll(fds.as_mut_ptr(), 1, wait.as_millis() as i32) };
        stop.drain();
        wait = next_wait(wait, served, redial.swap(false, Ordering::Relaxed));
    }
}

/// How long to wait before dialling again.
///
/// Only a connection that served, for `SERVED_LONG_ENOUGH`, starts the backoff
/// over. A device already connected another way answers every dial and is hung
/// up on, and one that keeps dropping out answers and is gone again; resetting
/// on either would dial it every few seconds for as long as both are awake.
fn next_wait(wait: Duration, served: bool, redialed: bool) -> Duration {
    if redialed {
        RETRY_MIN
    } else if served {
        RETRY_MIN * 2
    } else {
        (wait * 2).min(RETRY_MAX)
    }
}

fn connect(addr: &str) -> Result<tungstenite::WebSocket<TcpStream>, String> {
    let host_port = if addr
        .rsplit_once(':')
        .is_some_and(|(_, p)| p.parse::<u16>().is_ok())
        && !addr.ends_with(']')
    {
        addr.to_string()
    } else {
        format!("{addr}:{DEVICES_PORT}")
    };
    let targets = host_port.to_socket_addrs().map_err(|e| e.to_string())?;
    let mut last = "no address".to_string();
    for target in targets {
        match TcpStream::connect_timeout(&target, Duration::from_secs(3)) {
            Ok(stream) => {
                stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .map_err(|e| e.to_string())?;
                let url = format!("ws://{host_port}/");
                let (socket, _) =
                    tungstenite::client(url.as_str(), stream).map_err(|e| e.to_string())?;
                socket
                    .get_ref()
                    .set_nonblocking(true)
                    .map_err(|e| e.to_string())?;
                let _ = socket.get_ref().set_nodelay(true);
                return Ok(socket);
            }
            Err(e) => last = e.to_string(),
        }
    }
    Err(last)
}

struct Controlling<'a> {
    stop: &'a Arc<Stop>,
    waker: Arc<Waker>,
    /// Which entry of `FOUND` this connection is for.
    key: &'a str,
    /// Where it was reached, remembered for the next run.
    addr: &'a str,
    id: Option<String>,
    this_device: bool,
    /// Already connected to this device another way: announced and typed in.
    duplicate: bool,
    handshake: Handshake,
    /// Who the listener proved to be, if it proved anything.
    proven: Option<proof::Peer>,
    /// The listener, and the session its reports are signed for, once it has
    /// proved itself and the handshake says it signs them: from then on an
    /// unsigned report is one slipped into the stream.
    reports: Option<(Proven, proof::Session)>,
    /// Proof frames to send.
    pending: Vec<String>,
}

/// Where this end of a connection is in proving itself.
enum Handshake {
    /// Nothing proven, as with a listener that predates proofs or a device
    /// with no key: commands go as they are, and are trusted as the network
    /// is.
    Plain,
    /// Signed and sent; commands wait for the answer, so none goes out
    /// before the connection is settled.
    Awaiting {
        listener: String,
        me: String,
        listen_nonce: String,
        dial_nonce: String,
    },
    /// Proven: every command is signed for this session.
    Signed(proof::Session),
}

impl Controlling<'_> {
    /// Answer a listener that asked for proof, if this device has one to give.
    fn prove(&mut self, listener: &str, listen_nonce: String) {
        let Some(me) = devices::this_id() else { return };
        let Some(dial_nonce) = proof::nonce() else {
            return;
        };
        let Some(sig) = proof::sign_dial(listener, &me, &listen_nonce, &dial_nonce) else {
            return;
        };
        self.pending.extend(frame(&ProofFrame::Auth {
            id: me.clone(),
            nonce: dial_nonce.clone(),
            sig,
        }));
        self.handshake = Handshake::Awaiting {
            listener: listener.to_owned(),
            me,
            listen_nonce,
            dial_nonce,
        };
    }

    fn answered(&mut self, verified: bool, sig: Option<String>) {
        let Handshake::Awaiting {
            listener,
            me,
            listen_nonce,
            dial_nonce,
        } = &self.handshake
        else {
            return;
        };
        let listener_is = sig.and_then(|sig| {
            proof::verify_listen(me, listener, dial_nonce, listen_nonce, verified, &sig)
        });
        log::info!(
            "nearby: {listener} {} us; it proved itself: {:?}",
            if verified { "took" } else { "did not take" },
            listener_is.as_ref().map(|p| &p.peer)
        );
        self.reports = listener_is
            .clone()
            .filter(|_| proof::signs_reports(listen_nonce, dial_nonce))
            .map(|p| {
                let session = proof::Session::reports(listener, me, listen_nonce, dial_nonce);
                (p, session)
            });
        self.proven = listener_is.map(|p| p.peer);
        if let Some(id) = &self.id
            && let Some(conn) = CONNS.lock().as_mut().and_then(|c| c.get_mut(id))
        {
            conn.proven = self.proven.clone();
        }
        self.handshake = if verified {
            Handshake::Signed(proof::Session::new(listener, me, listen_nonce, dial_nonce))
        } else {
            Handshake::Plain
        };
    }
}

impl wire::Session for Controlling<'_> {
    fn outgoing(&mut self) -> Vec<String> {
        let mut out = std::mem::take(&mut self.pending);
        if matches!(self.handshake, Handshake::Awaiting { .. }) {
            return out;
        }
        let Some(id) = &self.id else {
            return out;
        };
        let commands = {
            let mut conns = CONNS.lock();
            let Some(conn) = conns.as_mut().and_then(|c| c.get_mut(id)) else {
                return out;
            };
            std::mem::take(&mut conn.outbox)
        };
        for command in commands {
            let Ok(json) = serde_json::to_string(&command) else {
                continue;
            };
            match &mut self.handshake {
                Handshake::Signed(session) => match session.sign(&json) {
                    Some((seq, sig)) => out.extend(frame(&ProofFrame::Signed {
                        seq,
                        sig,
                        command: json,
                    })),
                    // Signed out since: a proven listener refuses anything
                    // unsigned, so there is nothing to send it.
                    None => log::warn!("nearby: no key to sign with; {command:?} dropped"),
                },
                _ => out.push(json),
            }
        }
        out
    }

    fn incoming(&mut self, text: &str) {
        if let Ok(ProofFrame::Proof { verified, sig }) = serde_json::from_str(text) {
            return self.answered(verified, sig);
        }
        if let Some(report) = self.checked(text) {
            self.report(&report);
        }
    }

    fn done(&self) -> bool {
        self.this_device || self.duplicate || self.stop.stopped()
    }
}

impl Controlling<'_> {
    /// The report `text` is or carries, if it is one to read. Once the
    /// listener signs its reports, only those it signed for this connection,
    /// in order. Before then, or from a listener this end cannot check, a
    /// report signed or not is trusted as the network is.
    fn checked<'t>(&mut self, text: &'t str) -> Option<std::borrow::Cow<'t, str>> {
        if let Ok(ProofFrame::SignedReport { seq, sig, report }) = serde_json::from_str(text) {
            if let Some((by, session)) = self.reports.as_mut()
                && !session.accept(by, seq, &sig, &report)
            {
                log::warn!("nearby: a report not signed for this connection; refused");
                return None;
            }
            return Some(report.into());
        }
        if self.reports.is_some() {
            log::warn!("nearby: an unsigned report on a connection that signs them; refused");
            return None;
        }
        Some(text.into())
    }

    fn report(&mut self, text: &str) {
        match serde_json::from_str::<LinkReport>(text) {
            Ok(LinkReport::Hello(mut hello)) => {
                if devices::this_id().as_deref() == Some(hello.id.as_str()) {
                    self.this_device = true;
                    return;
                }
                let mut guard = CONNS.lock();
                let conns = guard.get_or_insert_with(HashMap::new);
                // Found twice: the first connection serves, and this one hangs
                // up and tries again later, in case that one has gone by then.
                if conns.contains_key(&hello.id) {
                    self.duplicate = true;
                    return;
                }
                conns.insert(
                    hello.id.clone(),
                    Conn {
                        outbox: Vec::new(),
                        waker: self.waker.clone(),
                        proven: None,
                    },
                );
                self.id = Some(hello.id.clone());
                log::info!("nearby: found {} ({})", hello.name, hello.platform);
                // The nonce is this connection's, and not to be remembered.
                if let Some(nonce) = hello.nonce.take() {
                    self.prove(&hello.id, nonce);
                }
                drop(guard);
                set_blocked(false);
                {
                    let mut found = FOUND.lock();
                    if let Some((_, f)) = found.iter_mut().find(|(k, _)| k == self.key) {
                        f.id = Some(hello.id.clone());
                        f.platform = Some(hello.platform.clone());
                        f.problem = None;
                    }
                }
                devices::nearby_hello(hello, self.addr);
            }
            Ok(LinkReport::State(state)) => {
                if let Some(id) = &self.id {
                    devices::nearby_state(id, state);
                }
            }
            Ok(LinkReport::Levels { f }) => {
                if let Some(id) = &self.id {
                    crate::remote::levels::remote().received(id, f);
                }
            }
            Ok(LinkReport::Ack { ack, outcome }) => {
                let Some(id) = &self.id else { return };
                if proven_as_listed(devices::listed_owner(id), self.proven.as_ref()) {
                    crate::remote::acks::resolve(ack, id, outcome);
                } else {
                    // The link, which the server vouches for, answers instead.
                    log::warn!("nearby: an answer from {id}, which did not prove it is; ignored");
                }
            }
            Ok(_) => {}
            Err(e) => log::debug!("nearby: not a report ({e})"),
        }
    }
}

/// Whether a listener is the device it said it is in its `Hello`, which
/// anyone can say, as far as commands to it and answers from it go. One of
/// the account's devices (`listed` is `Some(None)`) or one shared with it
/// (`Some(Some(owner))`) must have proved it is that device, under that
/// account; a stranger's word is all there is of a stranger.
fn proven_as_listed(listed: Option<Option<String>>, proven: Option<&proof::Peer>) -> bool {
    match listed {
        None => true,
        Some(None) => proven == Some(&proof::Peer::Own),
        Some(Some(owner)) => matches!(proven, Some(proof::Peer::Shared(o)) if *o == owner),
    }
}

/// This device's announcement for as long as the listener holds it.
#[cfg(target_vendor = "apple")]
struct Announced;

#[cfg(target_vendor = "apple")]
impl Announced {
    fn new(port: u16, identity: &crate::remote::link::LinkIdentity) -> Self {
        *ADVERT.lock() = bonjour::advertise(port, identity, &announced_server());
        Self
    }
}

#[cfg(target_vendor = "apple")]
impl Drop for Announced {
    fn drop(&mut self) {
        *ADVERT.lock() = None;
    }
}

#[cfg(target_vendor = "apple")]
fn announced(
    name: String,
    host: String,
    port: u16,
    id: Option<String>,
    platform: Option<String>,
    server: Option<String>,
) {
    // A resolve that finished after browsing was paused.
    if !crate::quiet::awake() || id.is_some() && id == devices::this_id() {
        return;
    }
    set_blocked(false);
    let key = match &id {
        Some(id) => format!("id:{id}"),
        None => format!("bonjour:{name}"),
    };
    let addr = format!("{}:{port}", host.trim_end_matches('.'));
    {
        let mut found = FOUND.lock();
        let problem = found
            .iter()
            .find(|(k, _)| *k == key)
            .and_then(|(_, f)| f.problem.clone());
        found.retain(|(k, _)| *k != key);
        found.push((
            key.clone(),
            Found {
                name: name.clone(),
                bonjour: Some(name),
                id,
                platform,
                server: server.filter(|s| !s.is_empty()),
                problem,
            },
        ));
    }
    devices::touch();
    let mut running = RUNNING.lock();
    let Some(r) = running.as_mut() else { return };
    match r.dialers.get(&key) {
        // Already dialled, from a previous run's address or an earlier
        // announcement: dial where it is now, and now rather than when the
        // backoff runs out.
        Some(d) => {
            *d.addr.lock() = addr;
            d.redial();
        }
        None => spawn_dialer(r, key, addr),
    }
}

#[cfg(target_vendor = "apple")]
fn lost(name: &str) {
    let key = {
        let mut found = FOUND.lock();
        let key = found
            .iter()
            .find(|(_, f)| f.bonjour.as_deref() == Some(name))
            .map(|(k, _)| k.clone());
        if let Some(k) = &key {
            found.retain(|(kk, _)| kk != k);
        }
        key
    };
    let Some(key) = key else { return };
    devices::touch();
    if let Some(r) = RUNNING.lock().as_mut()
        && let Some(d) = r.dialers.remove(&key)
    {
        d.stop.stop();
    }
}

/// Bonjour through the system's own responder (`dns_sd`), which is what iOS
/// allows an app without the multicast entitlement.
#[cfg(target_vendor = "apple")]
pub(crate) mod bonjour {
    use std::ffi::{CStr, CString, c_char, c_void};

    use crate::remote::link::LinkIdentity;

    type Ref = *mut c_void;
    type BrowseReply =
        extern "C" fn(Ref, u32, u32, i32, *const c_char, *const c_char, *const c_char, *mut c_void);
    type ResolveReply = extern "C" fn(
        Ref,
        u32,
        u32,
        i32,
        *const c_char,
        *const c_char,
        u16,
        u16,
        *const u8,
        *mut c_void,
    );

    unsafe extern "C" {
        fn DNSServiceRegister(
            sd: *mut Ref,
            flags: u32,
            interface: u32,
            name: *const c_char,
            regtype: *const c_char,
            domain: *const c_char,
            host: *const c_char,
            port: u16,
            txt_len: u16,
            txt: *const c_void,
            callback: *const c_void,
            context: *mut c_void,
        ) -> i32;
        fn DNSServiceBrowse(
            sd: *mut Ref,
            flags: u32,
            interface: u32,
            regtype: *const c_char,
            domain: *const c_char,
            callback: BrowseReply,
            context: *mut c_void,
        ) -> i32;
        fn DNSServiceResolve(
            sd: *mut Ref,
            flags: u32,
            interface: u32,
            name: *const c_char,
            regtype: *const c_char,
            domain: *const c_char,
            callback: ResolveReply,
            context: *mut c_void,
        ) -> i32;
        fn DNSServiceRefSockFD(sd: Ref) -> i32;
        fn DNSServiceProcessResult(sd: Ref) -> i32;
        fn DNSServiceRefDeallocate(sd: Ref);
    }

    const FLAG_ADD: u32 = 0x2;

    /// This device, announced for as long as it is held.
    pub struct Advert(Ref);

    // SAFETY: the reference is only deallocated, once, from whichever thread
    // drops it; dns_sd allows that.
    unsafe impl Send for Advert {}

    impl Drop for Advert {
        fn drop(&mut self) {
            // SAFETY: a reference DNSServiceRegister returned, freed once.
            unsafe { DNSServiceRefDeallocate(self.0) };
        }
    }

    /// `server` is the address it is signed in to, announced so that a device
    /// not signed in yet can offer it; empty, it is left out.
    pub fn advertise(port: u16, identity: &LinkIdentity, server: &str) -> Option<Advert> {
        let name = CString::new(identity.name.as_str()).ok()?;
        let regtype = CString::new(super::SERVICE).ok()?;
        let mut pairs = vec![
            ("id", identity.device_id.as_str()),
            ("platform", identity.platform.as_str()),
        ];
        if !server.is_empty() {
            pairs.push(("server", server));
        }
        let txt = txt_record(&pairs);
        let mut sd: Ref = std::ptr::null_mut();
        // SAFETY: every pointer is live for the call; a null callback is
        // allowed, and the responder renames on a conflict by itself.
        let err = unsafe {
            DNSServiceRegister(
                &mut sd,
                0,
                0,
                name.as_ptr(),
                regtype.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                port.to_be(),
                txt.len() as u16,
                txt.as_ptr().cast(),
                std::ptr::null(),
                std::ptr::null_mut(),
            )
        };
        if err != 0 {
            log::warn!("nearby: cannot announce this device ({err})");
            if err == POLICY_DENIED {
                super::set_blocked(true);
            }
            return None;
        }
        Some(Advert(sd))
    }

    fn txt_record(pairs: &[(&str, &str)]) -> Vec<u8> {
        let mut out = Vec::new();
        for (k, v) in pairs {
            let entry = format!("{k}={v}");
            let bytes = &entry.as_bytes()[..entry.len().min(255)];
            out.push(bytes.len() as u8);
            out.extend_from_slice(bytes);
        }
        out
    }

    fn txt_value(txt: &[u8], key: &str) -> Option<String> {
        let mut rest = txt;
        while let Some((&len, tail)) = rest.split_first() {
            let (entry, next) = tail.split_at((len as usize).min(tail.len()));
            if let Some(v) = std::str::from_utf8(entry)
                .ok()
                .and_then(|e| e.strip_prefix(key))
                .and_then(|e| e.strip_prefix('='))
            {
                return Some(v.to_string());
            }
            rest = next;
        }
        None
    }

    struct Seen {
        add: bool,
        interface: u32,
        name: CString,
        regtype: CString,
        domain: CString,
    }

    extern "C" fn on_browse(
        _: Ref,
        flags: u32,
        interface: u32,
        err: i32,
        name: *const c_char,
        regtype: *const c_char,
        domain: *const c_char,
        context: *mut c_void,
    ) {
        if err != 0 {
            BROWSE_ERR.store(err, std::sync::atomic::Ordering::Relaxed);
            return;
        }
        // SAFETY: the context is the Vec `browse_once` passed, alive for
        // the call to DNSServiceProcessResult that runs this; the strings are
        // the responder's, valid for this callback.
        unsafe {
            let seen = &mut *(context as *mut Vec<Seen>);
            seen.push(Seen {
                add: flags & FLAG_ADD != 0,
                interface,
                name: CStr::from_ptr(name).to_owned(),
                regtype: CStr::from_ptr(regtype).to_owned(),
                domain: CStr::from_ptr(domain).to_owned(),
            });
        }
    }

    /// What the responder answers when the system has not let this app onto
    /// the local network.
    const POLICY_DENIED: i32 = -65570;

    static BROWSE_ERR: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
    static RESTART: std::sync::OnceLock<std::sync::Arc<crate::remote::wire::Waker>> =
        std::sync::OnceLock::new();
    static RESTART_ASKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    static PAUSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    /// Browse afresh: after iOS suspended the app its connection to the
    /// responder may be gone, and a live one says nothing new until
    /// something changes. Ends a pause.
    pub fn restart() {
        PAUSED.store(false, std::sync::atomic::Ordering::Relaxed);
        RESTART_ASKED.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(w) = RESTART.get() {
            w.wake();
        }
    }

    /// Stop browsing until `restart`.
    pub fn pause() {
        PAUSED.store(true, std::sync::atomic::Ordering::Relaxed);
        RESTART_ASKED.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(w) = RESTART.get() {
            w.wake();
        }
    }

    /// Browse for the life of the process, starting again whenever the
    /// responder drops the browse or `restart` is asked for.
    pub fn browse_forever() {
        let Ok(waker) = crate::remote::wire::Waker::new() else {
            return;
        };
        let _ = RESTART.set(waker.clone());
        loop {
            while PAUSED.load(std::sync::atomic::Ordering::Relaxed) {
                let mut fds = [libc::pollfd {
                    fd: waker.read_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                }];
                // SAFETY: a live array of the length given.
                unsafe { libc::poll(fds.as_mut_ptr(), 1, -1) };
                waker.drain();
            }
            if let Err(e) = browse_once(&waker) {
                log::warn!("nearby: browsing stopped: {e}");
            }
            let mut fds = [libc::pollfd {
                fd: waker.read_fd(),
                events: libc::POLLIN,
                revents: 0,
            }];
            if !RESTART_ASKED.load(std::sync::atomic::Ordering::Relaxed) {
                // SAFETY: a live array of the length given.
                unsafe { libc::poll(fds.as_mut_ptr(), 1, 2000) };
            }
            waker.drain();
        }
    }

    fn browse_once(waker: &crate::remote::wire::Waker) -> Result<(), String> {
        use std::sync::atomic::Ordering;
        RESTART_ASKED.store(false, Ordering::Relaxed);
        let regtype = CString::new(super::SERVICE).map_err(|e| e.to_string())?;
        let mut seen: Vec<Seen> = Vec::new();
        let mut interfaces: std::collections::HashMap<String, usize> = Default::default();
        // What was announced before this browse began: anything not announced
        // again shortly is gone, withdrawn while this app was not looking.
        let mut unconfirmed: std::collections::HashSet<String> = super::FOUND
            .lock()
            .iter()
            .filter_map(|(_, f)| f.bonjour.clone())
            .collect();
        let settle = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut sd: Ref = std::ptr::null_mut();
        // SAFETY: `seen` outlives the reference, which is deallocated before
        // this returns.
        let err = unsafe {
            DNSServiceBrowse(
                &mut sd,
                0,
                0,
                regtype.as_ptr(),
                std::ptr::null(),
                on_browse,
                (&mut seen as *mut Vec<Seen>).cast(),
            )
        };
        if err != 0 {
            super::set_blocked(err == POLICY_DENIED);
            return Err(format!("cannot browse ({err})"));
        }
        let finish = |sd: Ref, result: Result<(), String>| {
            // SAFETY: the reference DNSServiceBrowse returned, freed once.
            unsafe { DNSServiceRefDeallocate(sd) };
            result
        };
        // SAFETY: a live reference.
        let fd = unsafe { DNSServiceRefSockFD(sd) };
        loop {
            let wait = if unconfirmed.is_empty() {
                -1
            } else {
                settle
                    .saturating_duration_since(std::time::Instant::now())
                    .as_millis() as i32
            };
            let mut fds = [
                libc::pollfd {
                    fd,
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: waker.read_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            // SAFETY: a live array of the length given.
            let ready = unsafe { libc::poll(fds.as_mut_ptr(), 2, wait) };
            if RESTART_ASKED.load(Ordering::Relaxed) {
                return finish(sd, Ok(()));
            }
            waker.drain();
            if !unconfirmed.is_empty() && std::time::Instant::now() >= settle {
                for name in unconfirmed.drain() {
                    super::lost(&name);
                }
            }
            if ready <= 0 || fds[0].revents == 0 {
                continue;
            }
            // SAFETY: a live reference with a reply waiting.
            if unsafe { DNSServiceProcessResult(sd) } != 0 {
                return finish(sd, Err("the responder went away".into()));
            }
            let err = BROWSE_ERR.swap(0, Ordering::Relaxed);
            if err != 0 {
                super::set_blocked(err == POLICY_DENIED);
                return finish(sd, Err(format!("browse error {err}")));
            }
            for s in seen.drain(..) {
                let name = s.name.to_string_lossy().into_owned();
                // Announced once per interface it is reached on: gone only
                // when gone from all of them, found on the first.
                let count = interfaces.entry(name.clone()).or_insert(0usize);
                if !s.add {
                    *count = count.saturating_sub(1);
                    if *count == 0 {
                        interfaces.remove(&name);
                        unconfirmed.remove(&name);
                        super::lost(&name);
                    }
                    continue;
                }
                unconfirmed.remove(&name);
                *count += 1;
                if *count > 1 {
                    continue;
                }
                std::thread::spawn(move || {
                    if let Some((host, port, id, platform, server)) = resolve(&s) {
                        super::announced(name, host, port, id, platform, server);
                    }
                });
            }
        }
    }

    /// A service instance resolved: where it is and what it says of itself.
    pub struct Instance {
        pub name: String,
        /// The interface it was resolved on.
        pub interface: u32,
        pub host: String,
        pub port: u16,
        pub txt: Vec<u8>,
    }

    impl Instance {
        pub fn txt(&self, key: &str) -> Option<String> {
            txt_value(&self.txt, key)
        }
    }

    extern "C" fn on_resolve(
        _: Ref,
        _: u32,
        interface: u32,
        err: i32,
        _: *const c_char,
        host: *const c_char,
        port: u16,
        txt_len: u16,
        txt: *const u8,
        context: *mut c_void,
    ) {
        if err != 0 {
            return;
        }
        // SAFETY: the context is `resolve_raw`'s `out`, alive for the call
        // that runs this; the host and TXT record are the responder's, valid
        // here.
        unsafe {
            let out = &mut *(context as *mut Option<(u32, String, u16, Vec<u8>)>);
            *out = Some((
                interface,
                CStr::from_ptr(host).to_string_lossy().into_owned(),
                u16::from_be(port),
                std::slice::from_raw_parts(txt, txt_len as usize).to_vec(),
            ));
        }
    }

    /// Resolve the instance `name` of `regtype`, waiting up to `within`.
    fn resolve_raw(
        flags: u32,
        interface: u32,
        name: &CStr,
        regtype: &CStr,
        domain: &CStr,
        within: std::time::Duration,
    ) -> Option<Instance> {
        let mut out: Option<(u32, String, u16, Vec<u8>)> = None;
        let mut sd: Ref = std::ptr::null_mut();
        // SAFETY: every pointer is live until the reference is deallocated
        // below, before `out` goes out of scope.
        unsafe {
            if DNSServiceResolve(
                &mut sd,
                flags,
                interface,
                name.as_ptr(),
                regtype.as_ptr(),
                domain.as_ptr(),
                on_resolve,
                (&mut out as *mut Option<(u32, String, u16, Vec<u8>)>).cast(),
            ) != 0
            {
                return None;
            }
            let mut fds = [libc::pollfd {
                fd: DNSServiceRefSockFD(sd),
                events: libc::POLLIN,
                revents: 0,
            }];
            if libc::poll(fds.as_mut_ptr(), 1, within.as_millis() as i32) > 0 {
                DNSServiceProcessResult(sd);
            }
            DNSServiceRefDeallocate(sd);
        }
        let (interface, host, port, txt) = out?;
        Some(Instance {
            name: name.to_string_lossy().into_owned(),
            interface,
            host,
            port,
            txt,
        })
    }

    type Resolved = Option<(String, u16, Option<String>, Option<String>, Option<String>)>;

    fn resolve(s: &Seen) -> Resolved {
        let i = resolve_raw(
            0,
            s.interface,
            &s.name,
            &s.regtype,
            &s.domain,
            std::time::Duration::from_secs(5),
        )?;
        Some((
            i.host.clone(),
            i.port,
            i.txt("id"),
            i.txt("platform"),
            i.txt("server"),
        ))
    }

    /// Asks the responder to send the sleeping host a magic packet through
    /// whichever Bonjour Sleep Proxy is answering for it.
    const FLAG_WAKE_ON_RESOLVE: u32 = 0x40000;

    /// Every instance of `regtype` announced within `within`, resolved.
    pub fn instances(regtype: &str, within: std::time::Duration) -> Vec<Instance> {
        let Ok(regtype) = CString::new(regtype) else {
            return Vec::new();
        };
        let mut seen: Vec<Seen> = Vec::new();
        let mut sd: Ref = std::ptr::null_mut();
        // SAFETY: `seen` outlives the reference, which is deallocated before
        // this returns.
        let err = unsafe {
            DNSServiceBrowse(
                &mut sd,
                0,
                0,
                regtype.as_ptr(),
                std::ptr::null(),
                on_browse,
                (&mut seen as *mut Vec<Seen>).cast(),
            )
        };
        if err != 0 {
            return Vec::new();
        }
        let until = std::time::Instant::now() + within;
        // SAFETY: a live reference.
        let fd = unsafe { DNSServiceRefSockFD(sd) };
        loop {
            let left = until.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                break;
            }
            let mut fds = [libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            }];
            // SAFETY: a live array of the length given.
            if unsafe { libc::poll(fds.as_mut_ptr(), 1, left.as_millis() as i32) } <= 0 {
                continue;
            }
            // SAFETY: a live reference with a reply waiting.
            if unsafe { DNSServiceProcessResult(sd) } != 0 {
                break;
            }
        }
        // SAFETY: the reference DNSServiceBrowse returned, freed once.
        unsafe { DNSServiceRefDeallocate(sd) };
        // `on_browse` reports a browse error through the shared slot; this
        // browse is not the one that watches it.
        BROWSE_ERR.store(0, std::sync::atomic::Ordering::Relaxed);
        let mut names = std::collections::HashSet::new();
        seen.into_iter()
            .filter(|s| s.add && names.insert(s.name.clone()))
            .filter_map(|s| {
                resolve_raw(
                    0,
                    s.interface,
                    &s.name,
                    &s.regtype,
                    &s.domain,
                    std::time::Duration::from_secs(2),
                )
            })
            .collect()
    }

    /// Resolve the instance `name` of `regtype` in `local.`, on `interface`
    /// (0 for any). A sleep proxy answers for a host asleep.
    pub fn resolve_named(
        name: &str,
        regtype: &str,
        interface: u32,
        within: std::time::Duration,
    ) -> Option<Instance> {
        let name = CString::new(name).ok()?;
        let regtype = CString::new(regtype).ok()?;
        resolve_raw(0, interface, &name, &regtype, c"local.", within)
    }

    /// Have the responder send a magic packet to `mac` at `ip` on `interface`.
    /// It reads the target from the instance name, `MAC@IP`, and only on a
    /// named interface; nothing answers the resolve itself.
    pub fn wake_on_resolve(mac: &str, ip: &str, regtype: &str, interface: u32) {
        let (Ok(name), Ok(regtype)) = (CString::new(format!("{mac}@{ip}")), CString::new(regtype))
        else {
            return;
        };
        if interface == 0 {
            return;
        }
        resolve_raw(
            FLAG_WAKE_ON_RESOLVE,
            interface,
            &name,
            &regtype,
            c"local.",
            std::time::Duration::from_millis(300),
        );
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_txt_record_reads_back() {
            let txt = txt_record(&[("id", "abc-123"), ("platform", "ios")]);
            assert_eq!(txt_value(&txt, "id").as_deref(), Some("abc-123"));
            assert_eq!(txt_value(&txt, "platform").as_deref(), Some("ios"));
            assert_eq!(txt_value(&txt, "name"), None);
        }

        #[test]
        fn a_txt_record_carries_the_server() {
            let txt = txt_record(&[
                ("id", "abc"),
                ("platform", "ios"),
                ("server", "https://music.example.com"),
            ]);
            assert_eq!(
                txt_value(&txt, "server").as_deref(),
                Some("https://music.example.com")
            );
        }
    }
}

#[cfg(test)]
mod dial_tests {
    use super::*;

    #[test]
    fn a_duplicate_backs_off_like_a_failure() {
        let mut wait = RETRY_MIN;
        for _ in 0..10 {
            wait = next_wait(wait, false, false);
        }
        assert_eq!(wait, RETRY_MAX);
    }

    #[test]
    fn a_connection_that_served_starts_over() {
        assert_eq!(next_wait(RETRY_MAX, true, false), RETRY_MIN * 2);
    }

    #[test]
    fn a_redial_skips_the_wait() {
        assert_eq!(next_wait(RETRY_MAX, false, true), RETRY_MIN);
    }
}

#[cfg(test)]
mod server_tests {
    use super::is_server;

    /// What another device announces is offered as a server to sign in to,
    /// so only a bare http(s) address is.
    #[test]
    fn a_device_announces_only_an_address_fit_to_offer() {
        use super::announceable;
        assert_eq!(
            announceable("https://music.example.com/").as_deref(),
            Some("https://music.example.com")
        );
        for url in [
            "",
            "https://user:secret@music.example.com",
            "https://music.example.com/?u=me&p=secret",
            "http://127.0.0.1:4799",
            "http://localhost:4799",
        ] {
            assert_eq!(announceable(url), None, "{url}");
        }
        // Too long to announce whole, so not announced at all.
        let long = format!("https://music.example.com/{}", "a".repeat(240));
        assert_eq!(announceable(&long), None);
    }

    #[test]
    fn only_a_bare_web_address_is_offered() {
        assert!(is_server("https://music.example.com"));
        assert!(is_server("http://192.168.1.20:4533/koan"));
        for s in [
            "",
            "music.example.com",
            "ftp://music.example.com",
            "javascript:alert(1)",
            "https://user:secret@music.example.com",
            "https://music.example.com/?u=me&p=secret",
            "https://music.example.com/#p=secret",
            "http://127.0.0.1:4799",
            "http://localhost:4799",
            "http://music.localhost",
            "http://[::1]:4799",
            "http://0.0.0.0:4799",
        ] {
            assert!(!is_server(s), "{s}");
        }
    }
}

#[cfg(test)]
mod tests {
    /// A device on the network sees what is playing, never where it plays.
    #[test]
    fn a_device_on_the_network_is_told_the_outputs_only_under_full_control() {
        let state = crate::remote::link::LinkState {
            playing: true,
            outputs: Some(Default::default()),
            ..Default::default()
        };
        let full = super::for_the_network(state.clone(), true);
        assert!(full.playing && full.outputs.is_some());
        let playback = super::for_the_network(state, false);
        assert!(playback.playing);
        assert_eq!(playback.outputs, None);
    }

    // --- Proof, from the listening end -------------------------------------

    use super::*;
    use crate::remote::link::LinkDeviceKey;
    use crate::remote::wire::Session as _;

    /// A listener "mac", signed in and holding a key list, with what it ran.
    struct Rig {
        _dir: tempfile::TempDir,
        local: Local,
        stop: Arc<Stop>,
        ran: Arc<Mutex<Vec<(LinkCommand, CommandSource)>>>,
    }

    /// This process's keypair stands for every device: `keys` lists who holds
    /// it, as the server would.
    fn rig(keys: &[(&str, Option<&str>)]) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        crate::config::set_config_dir(dir.path());
        Config::persist(|c| {
            c.remote.enabled = true;
            c.remote.url = "http://koan.test".into();
            c.remote.username = "jo".into();
            c.remote.api_key = "key".into();
            c.remote.device_key = proof::new_device_key().unwrap();
        })
        .unwrap();
        let public = proof::public_key().unwrap();
        proof::keep(
            keys.iter()
                .map(|(id, owner)| LinkDeviceKey {
                    id: (*id).into(),
                    key: public.clone(),
                    owner: owner.map(Into::into),
                })
                .collect(),
            proof::account_of(&Config::cached()),
        );
        let ran = Arc::new(Mutex::new(Vec::new()));
        let seen = ran.clone();
        Rig {
            _dir: dir,
            local: Local {
                identity: crate::remote::link::LinkIdentity {
                    name: "Mac".into(),
                    platform: "macos".into(),
                    device_id: "mac".into(),
                },
                state: Arc::new(LinkState::default),
                on_command: Arc::new(move |cmd, source, _| seen.lock().push((cmd, source))),
            },
            stop: Stop::new().unwrap(),
            ran,
        }
    }

    fn serving<'a>(r: &'a Rig) -> Serving<'a> {
        Serving {
            local: &r.local,
            stop: &r.stop,
            greeted: false,
            sent: None,
            waker: Waker::new().unwrap(),
            levels: None,
            answers: Default::default(),
            nonce: proof::nonce(),
            answered: false,
            proven: None,
            reports: None,
            pending: Vec::new(),
        }
    }

    fn json(f: &ProofFrame) -> String {
        frame(f).unwrap()
    }

    /// The dialler `dialer`'s half of the handshake, from this process's key.
    fn auth(s: &mut Serving, dialer: &str) -> (String, proof::Session) {
        auth_with(s, dialer, proof::nonce().unwrap())
    }

    /// The same, over the nonce `dial_nonce`.
    fn auth_with(s: &mut Serving, dialer: &str, dial_nonce: String) -> (String, proof::Session) {
        let listen_nonce = s.nonce.clone().unwrap();
        let sig = proof::sign_dial("mac", dialer, &listen_nonce, &dial_nonce).unwrap();
        let session = proof::Session::new("mac", dialer, &listen_nonce, &dial_nonce);
        s.incoming(&json(&ProofFrame::Auth {
            id: dialer.into(),
            nonce: dial_nonce.clone(),
            sig,
        }));
        (dial_nonce, session)
    }

    fn signed(session: &mut proof::Session, cmd: &LinkCommand) -> String {
        let command = serde_json::to_string(cmd).unwrap();
        let (seq, sig) = session.sign(&command).unwrap();
        json(&ProofFrame::Signed { seq, sig, command })
    }

    fn answer(s: &mut Serving) -> (bool, Option<String>) {
        let out = wire::Session::outgoing(s);
        out.iter()
            .find_map(|t| match serde_json::from_str::<ProofFrame>(t) {
                Ok(ProofFrame::Proof { verified, sig }) => Some((verified, sig)),
                _ => None,
            })
            .expect("an answer")
    }

    #[test]
    fn an_own_device_that_proves_itself_runs_with_the_accounts_powers() {
        let _guard = crate::config::tests::PERSIST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let r = rig(&[("phone", None), ("mac", None)]);
        let mut s = serving(&r);
        let hello = wire::Session::outgoing(&mut s);
        let listen_nonce = s.nonce.clone().unwrap();
        assert!(
            hello[0].contains(&listen_nonce),
            "the Hello carries the nonce"
        );

        let (dial_nonce, mut session) = auth(&mut s, "phone");
        let (verified, sig) = answer(&mut s);
        assert!(verified);
        // The listener proves itself back.
        let proven = proof::verify_listen(
            "phone",
            "mac",
            &dial_nonce,
            &listen_nonce,
            true,
            &sig.unwrap(),
        );
        assert_eq!(proven.map(|p| p.peer), Some(proof::Peer::Own));

        let sync = LinkCommand::Sync { full: true };
        let frame = signed(&mut session, &sync);
        s.incoming(&frame);
        // Replayed: refused.
        s.incoming(&frame);
        // Unsigned, on a proven connection: refused, whatever it is.
        s.incoming(&serde_json::to_string(&LinkCommand::Pause).unwrap());
        // The server's news, signed by the account's own device: refused.
        s.incoming(&signed(
            &mut session,
            &LinkCommand::DeviceKeys { keys: vec![] },
        ));
        s.incoming(&signed(&mut session, &LinkCommand::Next));

        let ran = r.ran.lock();
        assert_eq!(
            *ran,
            vec![
                (sync, CommandSource::Account),
                (LinkCommand::Next, CommandSource::Account),
            ]
        );
    }

    #[test]
    fn a_listener_signs_its_reports_when_both_ends_do() {
        let _guard = crate::config::tests::PERSIST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let r = rig(&[("phone", None), ("mac", None)]);
        let mut s = serving(&r);
        wire::Session::outgoing(&mut s);
        let listen_nonce = s.nonce.clone().unwrap();
        let (dial_nonce, _) = auth(&mut s, "phone");
        let out = wire::Session::outgoing(&mut s);
        let Ok(ProofFrame::Proof { sig, .. }) = serde_json::from_str(&out[0]) else {
            panic!("the proof goes first: {out:?}");
        };
        let mac = proof::verify_listen(
            "phone",
            "mac",
            &dial_nonce,
            &listen_nonce,
            true,
            &sig.unwrap(),
        )
        .unwrap();
        let mut session = proof::Session::reports("mac", "phone", &listen_nonce, &dial_nonce);
        let reports: Vec<String> = out[1..]
            .iter()
            .map(|t| match serde_json::from_str(t) {
                Ok(ProofFrame::SignedReport { seq, sig, report }) => {
                    assert!(session.accept(&mac, seq, &sig, &report));
                    report
                }
                _ => panic!("unsigned: {t}"),
            })
            .collect();
        assert!(
            reports.iter().any(|r| r.contains(r#""type":"state""#)),
            "the state is sent again, signed: {reports:?}"
        );
    }

    #[test]
    fn a_listener_reports_unsigned_to_an_older_dialler() {
        let _guard = crate::config::tests::PERSIST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let r = rig(&[("phone", None), ("mac", None)]);
        let mut s = serving(&r);
        wire::Session::outgoing(&mut s);
        // An older dialler's nonce carries no mark.
        auth_with(&mut s, "phone", "b2xkZXI=".into());
        assert!(answer(&mut s).0);
        s.sent = None;
        let out = wire::Session::outgoing(&mut s);
        assert!(!out.is_empty());
        assert!(
            out.iter()
                .all(|t| serde_json::from_str::<LinkReport>(t).is_ok()),
            "{out:?}"
        );
    }

    fn controlling<'a>(stop: &'a Arc<Stop>) -> Controlling<'a> {
        Controlling {
            stop,
            waker: Waker::new().unwrap(),
            key: "id:mac",
            addr: "10.0.0.2:7979",
            id: None,
            this_device: false,
            duplicate: false,
            handshake: Handshake::Plain,
            proven: None,
            reports: None,
            pending: Vec::new(),
        }
    }

    /// The dialler's side: the listener "mac" answers its proof, signed.
    fn answered(c: &mut Controlling, listen_nonce: &str, dial_nonce: &str) {
        c.handshake = Handshake::Awaiting {
            listener: "mac".into(),
            me: "phone".into(),
            listen_nonce: listen_nonce.into(),
            dial_nonce: dial_nonce.into(),
        };
        let sig = proof::sign_listen("phone", "mac", dial_nonce, listen_nonce, true);
        c.answered(true, sig);
    }

    #[test]
    fn a_dialler_reads_only_reports_signed_for_its_connection() {
        let _guard = crate::config::tests::PERSIST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _r = rig(&[("phone", None), ("mac", None)]);
        let stop = Stop::new().unwrap();
        let mut c = controlling(&stop);
        let (listen_nonce, dial_nonce) = (proof::nonce().unwrap(), proof::nonce().unwrap());
        answered(&mut c, &listen_nonce, &dial_nonce);
        assert_eq!(c.proven, Some(Peer::Own));

        let state = serde_json::to_string(&LinkReport::State(LinkState::default())).unwrap();
        assert!(c.checked(&state).is_none(), "unsigned");
        let mut session = proof::Session::reports("mac", "phone", &listen_nonce, &dial_nonce);
        let (seq, sig) = session.sign(&state).unwrap();
        let signed = json(&ProofFrame::SignedReport {
            seq,
            sig: sig.clone(),
            report: state.clone(),
        });
        assert_eq!(c.checked(&signed).as_deref(), Some(state.as_str()));
        assert!(c.checked(&signed).is_none(), "replayed");
        let forged = serde_json::to_string(&LinkReport::Ack {
            ack: 1,
            outcome: AckOutcome::Done,
        })
        .unwrap();
        let altered = json(&ProofFrame::SignedReport {
            seq: seq + 1,
            sig,
            report: forged,
        });
        assert!(c.checked(&altered).is_none(), "altered");
        // Signed for another connection.
        let mut other = proof::Session::reports("mac", "phone", &dial_nonce, &listen_nonce);
        let (seq, sig) = other.sign(&state).unwrap();
        let elsewhere = json(&ProofFrame::SignedReport {
            seq: seq + 5,
            sig,
            report: state.clone(),
        });
        assert!(c.checked(&elsewhere).is_none(), "another session");
    }

    #[test]
    fn a_dialler_reads_an_older_listeners_reports_unsigned() {
        let _guard = crate::config::tests::PERSIST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _r = rig(&[("phone", None), ("mac", None)]);
        let stop = Stop::new().unwrap();
        let mut c = controlling(&stop);
        answered(&mut c, "b2xkZXI=", &proof::nonce().unwrap());
        assert_eq!(c.proven, Some(Peer::Own));
        let state = serde_json::to_string(&LinkReport::State(LinkState::default())).unwrap();
        assert_eq!(c.checked(&state).as_deref(), Some(state.as_str()));
    }

    /// A connection is proven once. A second `nearbyAuth`, from a device
    /// that holds a key the first did not, changes nothing.
    #[test]
    fn a_second_proof_on_one_connection_is_ignored() {
        let _guard = crate::config::tests::PERSIST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let r = rig(&[("phone", Some("kim"))]);
        let mut s = serving(&r);
        let (_, _) = auth(&mut s, "stranger");
        assert!(!answer(&mut s).0);
        let (_, mut session) = auth(&mut s, "phone");
        assert!(
            wire::Session::outgoing(&mut s)
                .iter()
                .all(|t| !t.contains("nearbyProof")),
            "no second answer"
        );
        s.incoming(&signed(&mut session, &LinkCommand::Pause));
        assert!(r.ran.lock().is_empty(), "still unproven");
    }

    #[test]
    fn a_shared_device_gets_the_playback_set_as_its_account() {
        let _guard = crate::config::tests::PERSIST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let r = rig(&[("phone", Some("kim"))]);
        let mut s = serving(&r);
        let (_, mut session) = auth(&mut s, "phone");
        assert!(answer(&mut s).0);
        s.incoming(&signed(&mut session, &LinkCommand::Sync { full: false }));
        s.incoming(&signed(&mut session, &LinkCommand::Pause));
        assert_eq!(
            *r.ran.lock(),
            vec![(LinkCommand::Pause, CommandSource::Shared)]
        );
    }

    #[test]
    fn a_peer_that_proves_nothing_is_trusted_as_the_network_is() {
        let _guard = crate::config::tests::PERSIST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // The list does not hold the dialler's id.
        let r = rig(&[("mac", None)]);
        Config::persist(|c| c.devices.nearby_control = crate::config::NearbyControl::Playback)
            .unwrap();
        let mut s = serving(&r);
        let (_, mut session) = auth(&mut s, "phone");
        assert!(!answer(&mut s).0);
        // Its signed frames are refused; plain commands are a stranger's.
        s.incoming(&signed(&mut session, &LinkCommand::Pause));
        s.incoming(&serde_json::to_string(&LinkCommand::Sync { full: false }).unwrap());
        s.incoming(&serde_json::to_string(&LinkCommand::Pause).unwrap());
        assert_eq!(
            *r.ran.lock(),
            vec![(LinkCommand::Pause, CommandSource::Stranger)]
        );
    }

    #[test]
    fn a_peer_that_never_asks_is_trusted_as_before() {
        let _guard = crate::config::tests::PERSIST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let r = rig(&[("phone", None)]);
        let mut s = serving(&r);
        // An older dialler: no NearbyAuth, plain commands.
        s.incoming(&serde_json::to_string(&LinkCommand::Pause).unwrap());
        assert_eq!(r.ran.lock().len(), 1);
        assert_ne!(r.ran.lock()[0].1, CommandSource::Account);
    }
}

#[cfg(test)]
mod admit_tests {
    use super::*;
    use crate::remote::acks;

    /// A stranger who has seen one of this person's ids sends a command it may
    /// not, under that id. It is refused, and the id is still free for the
    /// person's own command when it comes.
    #[test]
    fn a_stranger_cannot_spend_an_id_ahead_of_the_real_command() {
        let id = acks::next_id();
        let refused = std::sync::Arc::new(parking_lot::Mutex::new(None));
        let got = refused.clone();
        let forged = Envelope {
            command: LinkCommand::Sync { full: true },
            ack: Some(id),
        };
        assert!(matches!(
            admit(
                forged,
                |cmd| cmd.from_the_network(false),
                move |_, o| *got.lock() = Some(o)
            ),
            Admitted::Neither
        ));
        assert!(matches!(*refused.lock(), Some(AckOutcome::Refused { .. })));

        // The real one, under the same id, is acted on.
        let real = Envelope {
            command: LinkCommand::Pause,
            ack: Some(id),
        };
        assert!(matches!(
            admit(real, |cmd| cmd.from_the_network(false), |_, _| {}),
            Admitted::Command(LinkCommand::Pause, _, Some(_))
        ));
    }
}

#[cfg(test)]
mod proof_tests {
    use super::*;
    use crate::remote::proof::Peer;

    #[test]
    fn a_listed_devices_name_needs_its_proof() {
        // A stranger is taken at its word: there is nothing else.
        assert!(proven_as_listed(None, None));
        // The account's own phone, proven, and an impostor using its id.
        assert!(proven_as_listed(Some(None), Some(&Peer::Own)));
        assert!(!proven_as_listed(Some(None), None));
        assert!(!proven_as_listed(
            Some(None),
            Some(&Peer::Shared("b".into()))
        ));
        // A device shared by account b, proven as b's, and not as another's.
        let shared = || Some(Some("b".to_string()));
        assert!(proven_as_listed(shared(), Some(&Peer::Shared("b".into()))));
        assert!(!proven_as_listed(shared(), Some(&Peer::Shared("c".into()))));
        assert!(!proven_as_listed(shared(), Some(&Peer::Own)));
        assert!(!proven_as_listed(shared(), None));
    }

    #[test]
    fn a_listed_device_is_sent_nothing_until_it_proves_it_is_that_device() {
        let id = "impostor-of-the-phone";
        let waker = Waker::new().unwrap();
        CONNS.lock().get_or_insert_with(HashMap::new).insert(
            id.into(),
            Conn {
                outbox: Vec::new(),
                waker,
                proven: None,
            },
        );
        let outbox = || CONNS.lock().as_ref().unwrap()[id].outbox.len();

        // Claiming the account's phone's id, unproven: nothing goes, so the
        // sender takes the link.
        assert!(!queue(id, Some(None), LinkCommand::Pause.into()));
        assert_eq!(outbox(), 0);
        // A stranger, as it always was.
        assert!(queue(id, None, LinkCommand::Pause.into()));
        assert_eq!(outbox(), 1);
        // Proven as the account's own.
        CONNS.lock().as_mut().unwrap().get_mut(id).unwrap().proven = Some(Peer::Own);
        assert!(queue(id, Some(None), LinkCommand::Pause.into()));
        assert_eq!(outbox(), 2);

        CONNS.lock().as_mut().unwrap().remove(id);
    }
}
