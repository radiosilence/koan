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

use std::collections::HashMap;
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::config::{Config, DEVICES_PORT};
use crate::remote::devices;
use crate::remote::link::{LinkCommand, LinkHello, LinkReport, LinkState, Local};
use crate::remote::wire::{self, Waker};

pub const SERVICE: &str = "_koan._tcp";

/// A connection this app made to another, once it has said who it is.
struct Conn {
    outbox: Vec<LinkCommand>,
    waker: Arc<Waker>,
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
    cfg!(target_os = "ios")
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
    });
    reconfigure();
    dial_remembered();
    #[cfg(target_vendor = "apple")]
    std::thread::Builder::new()
        .name("koan-bonjour".into())
        .spawn(bonjour::browse_forever)
        .expect("failed to spawn the Bonjour thread");
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
    reconfigure();
    dial_remembered();
    #[cfg(target_vendor = "apple")]
    bonjour::restart();
}

/// Dial the devices reached before, at the addresses they were reached at,
/// which answers before Bonjour has said anything.
fn dial_remembered() {
    if !crate::quiet::awake() {
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
}

/// The port this device is listening on, while it is.
pub fn listening_port() -> Option<u16> {
    *PORT.lock()
}

/// Queue `cmd` for the device `id` on its local connection. False when there
/// is none.
pub fn send(id: &str, cmd: LinkCommand) -> bool {
    let mut conns = CONNS.lock();
    let Some(conn) = conns.as_mut().and_then(|c| c.get_mut(id)) else {
        return false;
    };
    conn.outbox.push(cmd);
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
    let _advert = bonjour::advertise(port, &local.identity);

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
            }));
            self.greeted = true;
        }
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
        out.iter()
            .filter_map(|r| serde_json::to_string(r).ok())
            .collect()
    }

    fn incoming(&mut self, text: &str) {
        match serde_json::from_str::<LinkCommand>(text) {
            Ok(LinkCommand::WatchLevels { on }) => {
                self.levels = on.then(|| crate::remote::levels::feed().watch(&self.waker));
            }
            Ok(cmd) => match cmd.from_the_network(full_control()) {
                Some(source) => (self.local.on_command)(cmd, source),
                None => log::warn!("nearby: refused {cmd:?}"),
            },
            Err(e) => log::warn!("nearby: not a command ({e}): {text}"),
        }
    }

    fn done(&self) -> bool {
        self.stop.stopped()
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
}

impl wire::Session for Controlling<'_> {
    fn outgoing(&mut self) -> Vec<String> {
        let Some(id) = &self.id else {
            return Vec::new();
        };
        let mut conns = CONNS.lock();
        let Some(conn) = conns.as_mut().and_then(|c| c.get_mut(id)) else {
            return Vec::new();
        };
        std::mem::take(&mut conn.outbox)
            .iter()
            .filter_map(|c| serde_json::to_string(c).ok())
            .collect()
    }

    fn incoming(&mut self, text: &str) {
        match serde_json::from_str::<LinkReport>(text) {
            Ok(LinkReport::Hello(hello)) => {
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
                    },
                );
                self.id = Some(hello.id.clone());
                log::info!("nearby: found {} ({})", hello.name, hello.platform);
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
            Ok(_) => {}
            Err(e) => log::debug!("nearby: not a report ({e})"),
        }
    }

    fn done(&self) -> bool {
        self.this_device || self.duplicate || self.stop.stopped()
    }
}

#[cfg(target_vendor = "apple")]
fn announced(name: String, host: String, port: u16, id: Option<String>, platform: Option<String>) {
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
mod bonjour {
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

    pub fn advertise(port: u16, identity: &LinkIdentity) -> Option<Advert> {
        let name = CString::new(identity.name.as_str()).ok()?;
        let regtype = CString::new(super::SERVICE).ok()?;
        let txt = txt_record(&[
            ("id", &identity.device_id),
            ("platform", &identity.platform),
        ]);
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
                    if let Some((host, port, id, platform)) = resolve(&s) {
                        super::announced(name, host, port, id, platform);
                    }
                });
            }
        }
    }

    type Resolved = Option<(String, u16, Option<String>, Option<String>)>;

    extern "C" fn on_resolve(
        _: Ref,
        _: u32,
        _: u32,
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
        // SAFETY: the context is `resolve`'s `out`, alive for the call that
        // runs this; the host and TXT record are the responder's, valid here.
        unsafe {
            let out = &mut *(context as *mut Resolved);
            let txt = std::slice::from_raw_parts(txt, txt_len as usize);
            *out = Some((
                CStr::from_ptr(host).to_string_lossy().into_owned(),
                u16::from_be(port),
                txt_value(txt, "id"),
                txt_value(txt, "platform"),
            ));
        }
    }

    fn resolve(s: &Seen) -> Resolved {
        let mut out: Resolved = None;
        let mut sd: Ref = std::ptr::null_mut();
        // SAFETY: every pointer is live until the reference is deallocated
        // below, before `out` goes out of scope.
        unsafe {
            if DNSServiceResolve(
                &mut sd,
                0,
                s.interface,
                s.name.as_ptr(),
                s.regtype.as_ptr(),
                s.domain.as_ptr(),
                on_resolve,
                (&mut out as *mut Resolved).cast(),
            ) != 0
            {
                return None;
            }
            let mut fds = [libc::pollfd {
                fd: DNSServiceRefSockFD(sd),
                events: libc::POLLIN,
                revents: 0,
            }];
            if libc::poll(fds.as_mut_ptr(), 1, 5000) > 0 {
                DNSServiceProcessResult(sd);
            }
            DNSServiceRefDeallocate(sd);
        }
        out
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
}
