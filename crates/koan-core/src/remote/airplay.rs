//! Waking an Apple TV that has gone to sleep, so that the push that wakes
//! koan on it can arrive.
//!
//! A sleeping Apple TV hands its Bonjour services to a Bonjour Sleep Proxy,
//! which answers for them while it sleeps and sends it a magic packet when a
//! TCP connection is opened to one of their ports (mDNSResponder's
//! `mDNSCoreReceiveRawTransportPacket`; pyatv's `knock` does the same). koan's
//! own `_koan._tcp` announcement is no use for this: it is gone once tvOS
//! suspends the app. So while the TV is awake, the system's `_airplay._tcp`
//! announcement from the same host is recorded beside the koan device
//! (`devices::SeenNearby::tv`), and knocked on to wake it. A connection that
//! is accepted means the box is awake; what wakes koan there is the push,
//! sent as for any device asleep.
//!
//! Only an ordinary connection to a resolved service is made: a magic packet
//! of this app's own would need the multicast entitlement. The responder is
//! also asked to send one (`kDNSServiceFlagsWakeOnResolve`), which it does
//! itself, from the MAC address the AirPlay announcement carries.

use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

/// The services an Apple TV announces that are knocked on to wake it.
pub const SERVICES: &[&str] = &["_airplay._tcp", "_companion-link._tcp"];

/// An Apple TV as its AirPlay announcement had it while it was awake.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Tv {
    /// The `_airplay._tcp` instance name, which is the name set on the TV.
    pub name: String,
    /// The host both announcements resolve to, as `Living-Room.local.`.
    pub host: String,
    /// The `deviceid` in the announcement: its MAC address.
    #[serde(default)]
    pub mac: Option<String>,
    /// Its IPv4 address when recorded.
    #[serde(default)]
    pub ip: Option<String>,
}

/// One `_airplay._tcp` instance, resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Announced {
    pub name: String,
    pub host: String,
    pub mac: Option<String>,
}

/// The host of a koan device's address, `host:port` as dialled.
pub fn host_of(addr: &str) -> &str {
    addr.rsplit_once(':').map_or(addr, |(h, _)| h)
}

fn same_host(a: &str, b: &str) -> bool {
    let a = a.trim_end_matches('.');
    let b = b.trim_end_matches('.');
    !a.is_empty() && a.eq_ignore_ascii_case(b)
}

/// Which of `announced` is the koan device at `host` named `name`: the one on
/// the same host, else the only one of the same name. Two of the same name
/// and no host to tell them apart is a guess not worth waking a stranger's
/// TV for.
pub fn pick<'a>(announced: &'a [Announced], host: &str, name: &str) -> Option<&'a Announced> {
    if let Some(a) = announced.iter().find(|a| same_host(&a.host, host)) {
        return Some(a);
    }
    let mut named = announced
        .iter()
        .filter(|a| a.name.trim().eq_ignore_ascii_case(name.trim()));
    match (named.next(), named.next()) {
        (Some(a), None) => Some(a),
        _ => None,
    }
}

/// Whether a record from before still describes the TV at `host`, so it need
/// not be looked for again.
pub fn current(tv: Option<&Tv>, host: &str) -> bool {
    tv.is_some_and(|t| same_host(&t.host, host))
}

/// Connect to each of `addrs` in turn until one accepts, trying again until
/// `within` runs out. Every attempt is a SYN, which is what a sleep proxy
/// wakes the host for; the first that is accepted means it is awake.
/// `addrs` is asked afresh each round, since a TV waking may come back on
/// other ports. A first round with nothing to knock on gives up at once:
/// no sleep proxy answers for the TV here (away from home, unplugged), and
/// the push should not wait on it.
pub fn knock_until_awake(
    mut addrs: impl FnMut() -> Vec<SocketAddr>,
    within: Duration,
    mut give_up: impl FnMut() -> bool,
) -> bool {
    let until = Instant::now() + within;
    let mut first = true;
    loop {
        let round = addrs();
        if first && round.is_empty() {
            return false;
        }
        first = false;
        for addr in round {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() || give_up() {
                return false;
            }
            if TcpStream::connect_timeout(&addr, KNOCK.min(left)).is_ok() {
                return true;
            }
        }
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() || give_up() {
            return false;
        }
        // A refused connection comes back at once; a sleeping host's after
        // `KNOCK`. Either way the next round waits a moment.
        std::thread::sleep(ROUND.min(left));
    }
}

/// How long one connection attempt is given.
const KNOCK: Duration = Duration::from_millis(1500);
/// The pause between rounds of knocking.
const ROUND: Duration = Duration::from_millis(500);
/// How long to look for the TV's AirPlay announcement when recording it.
#[cfg(target_vendor = "apple")]
const LOOK: Duration = Duration::from_secs(3);

/// The AirPlay announcement of the koan device at `addr` named `name`, from
/// this network now. Blocks for a few seconds.
#[cfg(target_vendor = "apple")]
pub fn record(addr: &str, name: &str) -> Option<Tv> {
    use crate::remote::nearby::bonjour;
    let instances = bonjour::instances(SERVICES[0], LOOK);
    let announced: Vec<Announced> = instances
        .iter()
        .map(|i| Announced {
            name: i.name.clone(),
            host: i.host.clone(),
            mac: i.txt("deviceid"),
        })
        .collect();
    let a = pick(&announced, host_of(addr), name)?;
    Some(Tv {
        name: a.name.clone(),
        host: a.host.clone(),
        mac: a.mac.clone(),
        ip: ipv4(&a.host),
    })
}

#[cfg(not(target_vendor = "apple"))]
pub fn record(_addr: &str, _name: &str) -> Option<Tv> {
    None
}

#[cfg(target_vendor = "apple")]
fn ipv4(host: &str) -> Option<String> {
    (host.trim_end_matches('.'), 0)
        .to_socket_addrs()
        .ok()?
        .find(|a| a.is_ipv4())
        .map(|a| a.ip().to_string())
}

/// Wake `tv` and wait up to `within` for it to accept a connection, unless
/// `give_up` says to stop. Each step is logged with its time from the start.
#[cfg(target_vendor = "apple")]
pub fn wake(tv: &Tv, within: Duration, give_up: impl FnMut() -> bool) -> bool {
    use crate::remote::nearby::bonjour;
    let started = Instant::now();
    let ms = || started.elapsed().as_millis();
    let mut asked = false;
    let addrs = || {
        let mut out = Vec::new();
        for service in SERVICES {
            let Some(i) = bonjour::resolve_named(&tv.name, service, 0, Duration::from_secs(1))
            else {
                continue;
            };
            if !asked && let (Some(mac), Some(ip)) = (&tv.mac, &tv.ip) {
                bonjour::wake_on_resolve(mac, ip, service, i.interface);
                asked = true;
            }
            // Port 0 is how a sleep proxy announces a service that does not
            // wake the host.
            if i.port == 0 {
                continue;
            }
            let found = (i.host.trim_end_matches('.'), i.port)
                .to_socket_addrs()
                .map(|a| a.collect::<Vec<_>>())
                .unwrap_or_default();
            if found.is_empty() {
                out.extend(
                    tv.ip
                        .as_deref()
                        .and_then(|ip| format!("{ip}:{}", i.port).parse::<SocketAddr>().ok()),
                );
            } else {
                out.extend(found);
            }
        }
        log::info!("airplay: {}: knocking on {out:?} at +{}ms", tv.name, ms());
        out
    };
    let awake = knock_until_awake(addrs, within, give_up);
    if awake {
        log::info!("airplay: {}: awake at +{}ms", tv.name, ms());
    } else {
        log::warn!("airplay: {}: did not answer in {}ms", tv.name, ms());
    }
    awake
}

#[cfg(not(target_vendor = "apple"))]
pub fn wake(_tv: &Tv, _within: Duration, _give_up: impl FnMut() -> bool) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn announced(name: &str, host: &str) -> Announced {
        Announced {
            name: name.into(),
            host: host.into(),
            mac: Some("AA:BB:CC:DD:EE:FF".into()),
        }
    }

    #[test]
    fn the_same_host_is_the_tv() {
        let all = [
            announced("Bedroom", "Bedroom.local."),
            announced("Living Room", "Living-Room.local."),
        ];
        let tv = pick(&all, "living-room.local", "Somewhere else").unwrap();
        assert_eq!(tv.name, "Living Room");
    }

    #[test]
    fn the_name_decides_when_no_host_matches() {
        let all = [
            announced("Bedroom", "Bedroom-2.local."),
            announced("Living Room", "Apple-TV.local."),
        ];
        let tv = pick(&all, "10.0.0.4", "living room").unwrap();
        assert_eq!(tv.host, "Apple-TV.local.");
    }

    #[test]
    fn two_of_the_same_name_are_not_guessed_between() {
        let all = [
            announced("Apple TV", "Apple-TV.local."),
            announced("Apple TV", "Apple-TV-2.local."),
        ];
        assert_eq!(pick(&all, "10.0.0.4", "Apple TV"), None);
    }

    #[test]
    fn nothing_on_the_network_is_nothing() {
        assert_eq!(pick(&[], "Living-Room.local", "Living Room"), None);
    }

    #[test]
    fn an_address_gives_its_host() {
        assert_eq!(host_of("Living-Room.local:51234"), "Living-Room.local");
        assert_eq!(host_of("Living-Room.local"), "Living-Room.local");
    }

    #[test]
    fn a_record_on_the_same_host_is_kept() {
        let tv = Tv {
            name: "Living Room".into(),
            host: "Living-Room.local.".into(),
            mac: None,
            ip: None,
        };
        assert!(current(Some(&tv), "living-room.local"));
        assert!(!current(Some(&tv), "Bedroom.local"));
        assert!(!current(None, "Living-Room.local"));
    }

    #[test]
    fn a_record_reads_back_without_the_newer_fields() {
        let tv: Tv = serde_json::from_str(r#"{"name":"TV","host":"TV.local."}"#).unwrap();
        assert_eq!(tv.mac, None);
        assert_eq!(tv.ip, None);
    }

    /// A port nothing listens on: bound, then closed.
    fn closed_port() -> SocketAddr {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap()
    }

    #[test]
    fn a_host_that_answers_is_awake_at_once() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let started = Instant::now();
        assert!(knock_until_awake(
            || vec![closed_port(), addr],
            Duration::from_secs(5),
            || false
        ));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn a_host_that_wakes_part_way_is_found_awake() {
        // Refused until the "TV" wakes, a second in, on a port of its own.
        let addr = closed_port();
        let woke = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(1));
            let l = TcpListener::bind(addr).unwrap();
            let _ = l.accept();
        });
        assert!(knock_until_awake(
            || vec![addr],
            Duration::from_secs(10),
            || false
        ));
        woke.join().unwrap();
    }

    #[test]
    fn a_host_that_never_answers_is_given_up_on_in_time() {
        let addr = closed_port();
        let started = Instant::now();
        assert!(!knock_until_awake(
            || vec![addr],
            Duration::from_millis(1200),
            || false
        ));
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn nothing_to_knock_on_gives_up_at_once() {
        let started = Instant::now();
        let mut rounds = 0;
        assert!(!knock_until_awake(
            || {
                rounds += 1;
                Vec::new()
            },
            Duration::from_secs(20),
            || false
        ));
        assert_eq!(rounds, 1);
        assert!(started.elapsed() < Duration::from_millis(200));
    }

    #[test]
    fn a_wake_abandoned_stops_knocking() {
        let addr = closed_port();
        let started = Instant::now();
        assert!(!knock_until_awake(
            || vec![addr],
            Duration::from_secs(30),
            || true
        ));
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
