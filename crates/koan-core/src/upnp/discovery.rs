//! Finding renderers: SSDP search, then the network's own announcements.
//!
//! One `M-SEARCH` when something first asks, and again whenever a picker
//! opens. After that the list follows `ssdp:alive` and `ssdp:byebye` as
//! renderers send them, and an entry whose `max-age` runs out without being
//! renewed is left out of the list when it is next read. Nothing here runs on
//! a timer.

use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use url::Url;

use super::description::{self, Renderer};

const GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 250);
const PORT: u16 = 1900;
const SEARCH_TARGET: &str = "urn:schemas-upnp-org:device:MediaRenderer:1";

/// What an announcement without a `max-age` is assumed to last.
const DEFAULT_MAX_AGE: Duration = Duration::from_secs(1800);

struct Known {
    renderer: Renderer,
    expires: Instant,
}

#[derive(Default)]
struct Store {
    renderers: HashMap<String, Known>,
    describing: HashSet<String>,
}

static STORE: Mutex<Option<Store>> = Mutex::new(None);
static SEARCH: OnceLock<Option<UdpSocket>> = OnceLock::new();

fn with<R>(f: impl FnOnce(&mut Store) -> R) -> R {
    f(STORE.lock().get_or_insert_with(Store::default))
}

/// The renderers on the network, by name.
pub fn renderers() -> Vec<Renderer> {
    let now = Instant::now();
    let mut list: Vec<Renderer> = with(|s| {
        s.renderers
            .values()
            .filter(|k| k.expires > now)
            .map(|k| k.renderer.clone())
            .collect()
    });
    list.sort_by_key(|r| r.name.to_lowercase());
    list
}

pub fn find(udn: &str) -> Option<Renderer> {
    with(|s| s.renderers.get(udn).map(|k| k.renderer.clone()))
}

/// Add or refresh a renderer, as if it had announced itself.
pub fn remember(renderer: Renderer, max_age: Duration) {
    let udn = renderer.udn.clone();
    let changed = with(|s| {
        let expires = Instant::now() + max_age;
        let changed = s.renderers.get(&udn).is_none_or(|k| k.renderer != renderer);
        s.renderers.insert(udn, Known { renderer, expires });
        changed
    });
    if changed {
        crate::signal::engine_changed().bump();
    }
}

fn forget(udn: &str) {
    if with(|s| s.renderers.remove(udn).is_some()) {
        log::info!("upnp: {udn} left");
        crate::signal::engine_changed().bump();
    }
}

/// Search the network for renderers, starting discovery if it has not
/// started. Answers arrive over the next couple of seconds.
pub fn search() {
    let Some(socket) = SEARCH.get_or_init(start) else {
        return;
    };
    let msg = format!(
        "M-SEARCH * HTTP/1.1\r\nHOST: {GROUP}:{PORT}\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nST: {SEARCH_TARGET}\r\nUSER-AGENT: koan/{} UPnP/1.0\r\n\r\n",
        env!("CARGO_PKG_VERSION")
    );
    // UDP may drop one; renderers ignore a repeat they have already answered.
    for _ in 0..2 {
        if let Err(e) = socket.send_to(msg.as_bytes(), (GROUP, PORT)) {
            log::warn!("upnp: search failed: {e}");
            return;
        }
    }
}

/// Open the search socket, and a listener for announcements. The search
/// socket alone is enough to find renderers, so failing to share port 1900
/// with whatever else holds it costs only the live updates.
fn start() -> Option<UdpSocket> {
    let search = match UdpSocket::bind(("0.0.0.0", 0)) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("upnp: cannot open a socket for discovery: {e}");
            return None;
        }
    };
    let _ = search.set_multicast_ttl_v4(2);
    if let Ok(reader) = search.try_clone() {
        spawn_reader("koan-ssdp-search", reader);
    }
    match notify_socket() {
        Ok(socket) => spawn_reader("koan-ssdp-notify", socket),
        Err(e) => log::info!("upnp: not listening for announcements: {e}"),
    }
    Some(search)
}

fn notify_socket() -> std::io::Result<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    socket.set_reuse_port(true)?;
    socket.bind(&SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, PORT)).into())?;
    socket.join_multicast_v4(&GROUP, &Ipv4Addr::UNSPECIFIED)?;
    Ok(socket.into())
}

fn spawn_reader(name: &str, socket: UdpSocket) {
    let spawned = thread::Builder::new().name(name.into()).spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match socket.recv_from(&mut buf) {
                Ok((n, _)) => {
                    if let Some(msg) = Message::parse(&String::from_utf8_lossy(&buf[..n])) {
                        heard(msg);
                    }
                }
                Err(e) => {
                    log::warn!("upnp: discovery socket closed: {e}");
                    return;
                }
            }
        }
    });
    if let Err(e) = spawned {
        log::warn!("upnp: could not start {name}: {e}");
    }
}

#[derive(Debug, PartialEq)]
pub(crate) struct Message {
    udn: String,
    kind: Kind,
}

#[derive(Debug, PartialEq)]
enum Kind {
    Alive { location: Url, max_age: Duration },
    ByeBye,
}

impl Message {
    /// A search response or announcement about a renderer. Anything else on
    /// the multicast group, and there is plenty, is `None`.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let mut lines = text.split("\r\n");
        let first = lines.next()?;
        let headers: Vec<(String, &str)> = lines
            .filter_map(|l| l.split_once(':'))
            .map(|(k, v)| (k.trim().to_ascii_uppercase(), v.trim()))
            .collect();
        let header = |name: &str| headers.iter().find(|(k, _)| k == name).map(|(_, v)| *v);
        let udn = header("USN")?.split("::").next()?.to_string();
        if !udn.starts_with("uuid:") {
            return None;
        }

        if first.starts_with("NOTIFY") {
            if header("NTS") == Some("ssdp:byebye") {
                return Some(Self {
                    udn,
                    kind: Kind::ByeBye,
                });
            }
            if !is_renderer(header("NT")?) {
                return None;
            }
        } else if first.starts_with("HTTP/") {
            if !is_renderer(header("ST")?) {
                return None;
            }
        } else {
            return None;
        }

        let max_age = header("CACHE-CONTROL")
            .and_then(|v| {
                v.split(',')
                    .filter_map(|d| d.trim().split_once('='))
                    .find(|(k, _)| k.trim().eq_ignore_ascii_case("max-age"))
                    .and_then(|(_, v)| v.trim().parse().ok())
            })
            .map(Duration::from_secs)
            .unwrap_or(DEFAULT_MAX_AGE);
        Some(Self {
            udn,
            kind: Kind::Alive {
                location: Url::parse(header("LOCATION")?).ok()?,
                max_age,
            },
        })
    }
}

fn is_renderer(target: &str) -> bool {
    target.contains(":device:MediaRenderer:") || target.contains(":service:AVTransport:")
}

fn heard(msg: Message) {
    match msg.kind {
        Kind::ByeBye => forget(&msg.udn),
        Kind::Alive { location, max_age } => {
            // Known at this address: the announcement only renews it.
            let fresh = with(|s| {
                if let Some(known) = s.renderers.get_mut(&msg.udn)
                    && known.renderer.location == location
                {
                    known.expires = Instant::now() + max_age;
                    return false;
                }
                s.describing.insert(msg.udn.clone())
            });
            if fresh {
                describe(msg.udn, location, max_age);
            }
        }
    }
}

fn describe(udn: String, location: Url, max_age: Duration) {
    let spawned = thread::Builder::new()
        .name("koan-upnp-describe".into())
        .spawn({
            let udn = udn.clone();
            move || {
                match fetch(&location) {
                    Ok(Some(renderer)) => {
                        log::info!(
                            "upnp: found {} ({} {}) at {location}{}",
                            renderer.name,
                            renderer.manufacturer,
                            renderer.model,
                            if renderer.gapless { ", gapless" } else { "" }
                        );
                        remember(renderer, max_age);
                    }
                    Ok(None) => log::debug!("upnp: {location} is not a renderer"),
                    Err(e) => log::info!("upnp: cannot describe {location}: {e}"),
                }
                with(|s| s.describing.remove(&udn));
            }
        });
    if spawned.is_err() {
        with(|s| s.describing.remove(&udn));
    }
}

/// Fetch and read a renderer's description, and its AVTransport SCPD for
/// whether it takes a next track.
pub fn fetch(location: &Url) -> Result<Option<Renderer>, String> {
    let http = super::soap::client();
    let get = |url: &Url| -> Result<String, String> {
        http.get(url.clone())
            .send()
            .and_then(|r| r.error_for_status())
            .and_then(|r| r.text())
            .map_err(|e| e.to_string())
    };
    let Some(mut renderer) = description::parse_device(&get(location)?, location)? else {
        return Ok(None);
    };
    renderer.gapless = get(&renderer.av_transport.scpd)
        .and_then(|doc| description::parse_actions(&doc))
        .map(|actions| actions.iter().any(|a| a == "SetNextAVTransportURI"))
        .unwrap_or(false);
    Ok(Some(renderer))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_search_response_names_the_renderer_and_its_description() {
        let msg = Message::parse(
            "HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age=100\r\nEXT:\r\nLOCATION: http://192.168.1.20:1597/\r\nSERVER: UPnP/1.0 Kodi\r\nST: urn:schemas-upnp-org:device:MediaRenderer:1\r\nUSN: uuid:bc9d::urn:schemas-upnp-org:device:MediaRenderer:1\r\n\r\n",
        )
        .unwrap();
        assert_eq!(msg.udn, "uuid:bc9d");
        assert_eq!(
            msg.kind,
            Kind::Alive {
                location: Url::parse("http://192.168.1.20:1597/").unwrap(),
                max_age: Duration::from_secs(100),
            }
        );
    }

    #[test]
    fn announcements_are_read_and_others_ignored() {
        let alive = "NOTIFY * HTTP/1.1\r\nHost: 239.255.255.250:1900\r\nNT: urn:schemas-upnp-org:service:AVTransport:1\r\nNTS: ssdp:alive\r\nLocation: http://10.0.0.5/d.xml\r\nUSN: uuid:amp::urn:schemas-upnp-org:service:AVTransport:1\r\n\r\n";
        assert!(matches!(
            Message::parse(alive).unwrap().kind,
            Kind::Alive { max_age, .. } if max_age == DEFAULT_MAX_AGE
        ));

        let bye = "NOTIFY * HTTP/1.1\r\nNT: upnp:rootdevice\r\nNTS: ssdp:byebye\r\nUSN: uuid:amp::upnp:rootdevice\r\n\r\n";
        assert_eq!(Message::parse(bye).unwrap().kind, Kind::ByeBye);

        let server = "NOTIFY * HTTP/1.1\r\nNT: urn:schemas-upnp-org:device:MediaServer:1\r\nNTS: ssdp:alive\r\nLocation: http://10.0.0.6/\r\nUSN: uuid:nas::urn:schemas-upnp-org:device:MediaServer:1\r\n\r\n";
        assert!(Message::parse(server).is_none());

        let search = "M-SEARCH * HTTP/1.1\r\nST: ssdp:all\r\n\r\n";
        assert!(Message::parse(search).is_none());
    }
}
