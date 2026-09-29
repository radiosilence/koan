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
    /// One per address being dialled, keyed by the address or the Bonjour
    /// name it came from.
    dialers: HashMap<String, Arc<Stop>>,
}

static RUNNING: Mutex<Option<Running>> = Mutex::new(None);
static PORT: Mutex<Option<u16>> = Mutex::new(None);

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

/// Open this device to the network as the config says, and look for others.
pub fn start(local: Local) {
    *RUNNING.lock() = Some(Running {
        local,
        listener: None,
        dialers: HashMap::new(),
    });
    reconfigure();
    #[cfg(target_vendor = "apple")]
    std::thread::Builder::new()
        .name("koan-bonjour".into())
        .spawn(bonjour::browse_forever)
        .expect("failed to spawn the Bonjour thread");
}

/// Apply `devices.*` from the config as it is now: listen or stop, and dial
/// the addresses listed. For a settings screen that has just written them.
pub fn reconfigure() {
    let cfg = Config::load().unwrap_or_default();
    let mut running = RUNNING.lock();
    let Some(r) = running.as_mut() else { return };
    match (&r.listener, cfg.devices.discoverable) {
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
    r.dialers.retain(|key, stop| {
        let keep = key.starts_with("bonjour:") || wanted.contains(key);
        if !keep {
            stop.stop();
        }
        keep
    });
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
    let bind = |port: u16| {
        TcpListener::bind(("::", port)).or_else(|_| TcpListener::bind(("0.0.0.0", port)))
    };
    // Another process on the port: an ephemeral one still works on this
    // network, where it is announced; only a typed address would miss it.
    let listener = match bind(port).or_else(|e| {
        log::warn!("nearby: port {port} is taken ({e}); listening elsewhere");
        bind(0)
    }) {
        Ok(l) => l,
        Err(e) => {
            log::warn!("nearby: cannot listen: {e}");
            return;
        }
    };
    let Ok(port) = listener.local_addr().map(|a| a.port()) else {
        return;
    };
    if listener.set_nonblocking(true).is_err() {
        return;
    }
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
            Err(e) => {
                log::warn!("nearby: accept: {e}");
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    }
    *PORT.lock() = None;
    devices::touch();
    log::info!("nearby: stopped listening");
}

impl Stop {
    fn waker_fd(&self) -> i32 {
        self.waker.read_fd()
    }
}

/// Serve one device that connected to this one.
fn serve(stream: TcpStream, local: &Local, stop: &Arc<Stop>) -> Result<(), String> {
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
    };
    wire::drive(&mut socket, fd, &waker, &mut session)
}

struct Serving<'a> {
    local: &'a Local,
    stop: &'a Arc<Stop>,
    greeted: bool,
    sent: Option<(LinkState, Instant)>,
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
        let now = (self.local.state)();
        if self
            .sent
            .as_ref()
            .is_none_or(|(s, at)| now.differs(s, at.elapsed()))
        {
            out.push(LinkReport::State(now.clone()));
            self.sent = Some((now, Instant::now()));
        }
        out.iter()
            .filter_map(|r| serde_json::to_string(r).ok())
            .collect()
    }

    fn incoming(&mut self, text: &str) {
        match serde_json::from_str::<LinkCommand>(text) {
            Ok(cmd) if cmd.allowed_nearby() => (self.local.on_command)(cmd),
            Ok(cmd) => log::warn!("nearby: refused {cmd:?}"),
            Err(e) => log::warn!("nearby: not a command ({e}): {text}"),
        }
    }

    fn done(&self) -> bool {
        self.stop.stopped()
    }
}

// --- The connecting end -----------------------------------------------------

const RETRY_MIN: Duration = Duration::from_secs(2);
const RETRY_MAX: Duration = Duration::from_secs(60);

fn spawn_dialer(r: &mut Running, key: String, addr: String) {
    let Some(stop) = Stop::new() else { return };
    r.dialers.insert(key, stop.clone());
    let _ = std::thread::Builder::new()
        .name("koan-nearby-dial".into())
        .spawn(move || dial(addr, stop));
}

/// Stay connected to `addr` until stopped, or until it turns out to be this
/// device.
fn dial(addr: String, stop: Arc<Stop>) {
    let mut wait = RETRY_MIN;
    while !stop.stopped() {
        match connect(&addr) {
            Ok(mut socket) => {
                wait = RETRY_MIN;
                let fd = socket.get_ref().as_raw_fd();
                let Ok(waker) = Waker::new() else { return };
                stop.also_wake(&waker);
                let mut session = Controlling {
                    stop: &stop,
                    waker: waker.clone(),
                    id: None,
                    this_device: false,
                    duplicate: false,
                };
                let result = wire::drive(&mut socket, fd, &waker, &mut session);
                if let Some(id) = session.id.take() {
                    if let Some(conns) = CONNS.lock().as_mut() {
                        conns.remove(&id);
                    }
                    devices::nearby_gone(&id);
                }
                if session.this_device {
                    return;
                }
                if let Err(e) = result {
                    log::info!("nearby: {addr}: {e}");
                }
            }
            Err(e) => log::debug!("nearby: {addr}: {e}"),
        }
        let mut fds = [libc::pollfd {
            fd: stop.waker_fd(),
            events: libc::POLLIN,
            revents: 0,
        }];
        // SAFETY: a live array of the length given.
        unsafe { libc::poll(fds.as_mut_ptr(), 1, wait.as_millis() as i32) };
        wait = (wait * 2).min(RETRY_MAX);
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
                devices::nearby_hello(hello);
            }
            Ok(LinkReport::State(state)) => {
                if let Some(id) = &self.id {
                    devices::nearby_state(id, state);
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
fn found(name: String, host: String, port: u16, id: Option<String>) {
    if id.is_some() && id == devices::this_id() {
        return;
    }
    let mut running = RUNNING.lock();
    let Some(r) = running.as_mut() else { return };
    let key = format!("bonjour:{name}");
    if r.dialers.contains_key(&key) {
        return;
    }
    let host = host.trim_end_matches('.');
    spawn_dialer(r, key, format!("{host}:{port}"));
}

#[cfg(target_vendor = "apple")]
fn lost(name: &str) {
    if let Some(r) = RUNNING.lock().as_mut()
        && let Some(stop) = r.dialers.remove(&format!("bonjour:{name}"))
    {
        stop.stop();
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
            return;
        }
        // SAFETY: the context is the Vec `browse_forever` passed, alive for
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

    pub fn browse_forever() {
        let Ok(regtype) = CString::new(super::SERVICE) else {
            return;
        };
        let mut seen: Vec<Seen> = Vec::new();
        let mut interfaces: std::collections::HashMap<String, usize> = Default::default();
        let mut sd: Ref = std::ptr::null_mut();
        // SAFETY: `seen` outlives the reference, which is never deallocated:
        // this browses for the life of the process.
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
            log::warn!("nearby: cannot browse ({err})");
            return;
        }
        // SAFETY: a live reference.
        let fd = unsafe { DNSServiceRefSockFD(sd) };
        loop {
            let mut fds = [libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            }];
            // SAFETY: a live array of the length given.
            if unsafe { libc::poll(fds.as_mut_ptr(), 1, -1) } < 0 {
                continue;
            }
            // SAFETY: a live reference with a reply waiting.
            if unsafe { DNSServiceProcessResult(sd) } != 0 {
                log::warn!("nearby: browsing stopped");
                return;
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
                        super::lost(&name);
                    }
                    continue;
                }
                *count += 1;
                if *count > 1 {
                    continue;
                }
                std::thread::spawn(move || {
                    if let Some((host, port, id)) = resolve(&s) {
                        super::found(name, host, port, id);
                    }
                });
            }
        }
    }

    type Resolved = Option<(String, u16, Option<String>)>;

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
