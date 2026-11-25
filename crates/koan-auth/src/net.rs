//! Where a request came from, and who an authenticating proxy says sent it.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use http::{HeaderMap, HeaderName};
use ipnet::IpNet;

/// Whose `X-Forwarded-For` a server believes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ForwardedFrom {
    /// Any private address: a server behind an ingress, where only the proxy
    /// shares its private network.
    #[default]
    Private,
    /// Loopback alone: a server on a home network, where every device has a
    /// private address and only a tunnel client on the same machine forwards.
    /// A loopback connection that names no client comes from nowhere known,
    /// so it neither counts as this network nor shares a limit with it.
    Loopback,
}

/// The address a request came from.
///
/// Behind a reverse proxy the TCP peer is the proxy, for every client, so a
/// limit keyed on it is one bucket for everyone. When the peer is one `trust`
/// believes (a proxy in the cluster, or a tunnel on the host), the last
/// `X-Forwarded-For` entry is the one that proxy appended, and is the client.
/// Earlier entries are whatever the client sent, and are ignored, even when
/// the last entry is not an address: a proxy that appends `unknown` would
/// otherwise hand the choice to the client. Such a request comes from an
/// unknown address (`0.0.0.0`), as one through a trusted peer that sent no
/// header comes from that peer under [`ForwardedFrom::Private`] and from an
/// unknown address under [`ForwardedFrom::Loopback`]. Any other peer is the client, and
/// its header is not believed. Nor is it when the peer is unknown (`None`): a
/// listener served without its connection info would otherwise let every
/// client pick its own address.
///
/// IPv4-mapped IPv6 addresses come back as IPv4, so one client is one address
/// whichever way a dual-stack listener or a proxy spelled it.
pub fn client_address(
    peer: Option<SocketAddr>,
    trust: ForwardedFrom,
    headers: &HeaderMap,
) -> IpAddr {
    let unknown = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
    let Some(peer) = peer else {
        return unknown;
    };
    let peer = peer.ip().to_canonical();
    let believed = match trust {
        ForwardedFrom::Private => is_internal(peer),
        ForwardedFrom::Loopback => peer.is_loopback(),
    };
    if !believed {
        return peer;
    }
    // The proxy's entry is split from the raw bytes, so a client's own
    // entries that are not text cannot spoil the line the proxy appended to.
    let Some(line) = headers.get_all("x-forwarded-for").iter().next_back() else {
        return match trust {
            ForwardedFrom::Private => peer,
            ForwardedFrom::Loopback => unknown,
        };
    };
    // A header whose last entry is no address names nobody known: not the
    // proxy, whose private address would make the client look local.
    line.as_bytes()
        .rsplit(|&b| b == b',')
        .next()
        .and_then(|entry| std::str::from_utf8(entry).ok())
        .and_then(forwarded_entry)
        .map_or(unknown, |ip| ip.to_canonical())
}

/// An `X-Forwarded-For` entry as an address: bare, or with the port some
/// proxies add (`203.0.113.9:51234`, `[2001:db8::1]:443`).
fn forwarded_entry(entry: &str) -> Option<IpAddr> {
    let entry = entry.trim();
    entry
        .parse::<IpAddr>()
        .ok()
        .or_else(|| entry.parse::<SocketAddr>().ok().map(|s| s.ip()))
        .or_else(|| entry.strip_prefix('[')?.strip_suffix(']')?.parse().ok())
}

fn is_internal(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(v4) => v4.is_private() || v4.is_loopback(),
        IpAddr::V6(v6) => v6.is_loopback() || (v6.segments()[0] & 0xfe00) == 0xfc00,
    }
}

/// An authenticating reverse proxy in front of a server, whose header names
/// the account signed in to it.
#[derive(Clone, Debug)]
pub struct ProxyAuth {
    header: HeaderName,
    from: Arc<Vec<IpNet>>,
}

/// What a request's proxy says about who is signed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vouch<'a> {
    /// Proxy sign-in is off, the request did not come from the proxy, or the
    /// proxy sent no header.
    Absent,
    /// The proxy sent the header, but not as one account's name. Never to be
    /// read as absent: the browser's own session would then stand in for
    /// whoever the proxy signed in.
    Unusable,
    Named(&'a str),
}

impl ProxyAuth {
    /// From the header the proxy sets and the addresses it connects from,
    /// named by the settings `settings` (for the errors). Off when neither is
    /// set; on when both are. Anything between is refused rather than read as
    /// either: a header with no proxy to believe it from, a proxy with no
    /// header, a header name or entry that does not parse, or a range covering
    /// every address, which would let any client name any account.
    pub fn from_config(
        header: &str,
        from: &[String],
        settings: (&str, &str),
    ) -> Result<Option<Self>, String> {
        let (header_setting, from_setting) = settings;
        let header = header.trim();
        match (header.is_empty(), from.is_empty()) {
            (true, true) => return Ok(None),
            (false, true) => {
                return Err(format!(
                    "{header_setting} is set but {from_setting} is empty; \
                     name the addresses the authenticating proxy connects from"
                ));
            }
            (true, false) => {
                return Err(format!(
                    "{from_setting} is set but {header_setting} is empty; \
                     name the header the authenticating proxy sets"
                ));
            }
            (false, false) => {}
        }
        let header = HeaderName::from_bytes(header.as_bytes())
            .map_err(|_| format!("{header_setting} {header:?} is not a header name"))?;
        let from = from
            .iter()
            .map(|entry| {
                let entry = entry.trim();
                let net = entry
                    .parse::<IpNet>()
                    .or_else(|_| entry.parse::<IpAddr>().map(IpNet::from))
                    .map_err(|_| format!("{from_setting}: {entry:?} is not an address or range"))?;
                let net = canonical(net);
                if net.prefix_len() == 0 {
                    return Err(format!(
                        "{from_setting}: {entry:?} covers every address, so any client could \
                         name any account; name the proxy's own address"
                    ));
                }
                Ok(net)
            })
            .collect::<Result<Vec<_>, String>>()?;
        let ranges = from.iter().map(ToString::to_string).collect::<Vec<_>>();
        log::info!(
            "proxy sign-in on, believing {header} from {}",
            ranges.join(", ")
        );
        Ok(Some(Self {
            header,
            from: Arc::new(from),
        }))
    }

    /// What the proxy says about who is signed in. Only a connection from the
    /// proxy itself (`peer`) is heard; a header from anywhere else is absent.
    /// From the proxy, one value naming one account is believed, and anything
    /// else it sent is unusable: a proxy that appended to or merged with a
    /// client's own header would otherwise let the client name the account.
    pub fn user<'a>(&self, peer: Option<IpAddr>, headers: &'a HeaderMap) -> Vouch<'a> {
        let Some(peer) = peer else {
            return Vouch::Absent;
        };
        let peer = peer.to_canonical();
        if !self.from.iter().any(|net| net.contains(&peer)) {
            return Vouch::Absent;
        }
        let mut values = headers.get_all(&self.header).iter();
        let Some(first) = values.next() else {
            return Vouch::Absent;
        };
        match std::str::from_utf8(first.as_bytes()).map(str::trim) {
            Ok(name) if values.next().is_none() && !name.is_empty() && !name.contains(',') => {
                Vouch::Named(name)
            }
            _ => Vouch::Unusable,
        }
    }
}

/// An IPv4-mapped IPv6 range as the IPv4 range it maps, since a peer's
/// address is compared in that form.
fn canonical(net: IpNet) -> IpNet {
    match net {
        IpNet::V6(v6) if v6.prefix_len() >= 96 => match v6.addr().to_ipv4_mapped() {
            Some(v4) => ipnet::Ipv4Net::new(v4, v6.prefix_len() - 96)
                .map_or(net, IpNet::V4)
                .trunc(),
            None => net,
        },
        _ => net,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn forwarded(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", value.parse().unwrap());
        h
    }

    fn peer(ip: &str) -> Option<SocketAddr> {
        Some(SocketAddr::new(ip.parse().unwrap(), 4000))
    }

    #[test]
    fn a_trusted_proxys_last_entry_is_the_client() {
        let h = forwarded("6.6.6.6, 203.0.113.9");
        assert_eq!(
            client_address(peer("10.0.0.2"), ForwardedFrom::Private, &h),
            "203.0.113.9".parse::<IpAddr>().unwrap()
        );
        // A public peer is the client, whatever it claims.
        assert_eq!(
            client_address(peer("198.51.100.1"), ForwardedFrom::Private, &h),
            "198.51.100.1".parse::<IpAddr>().unwrap()
        );
        // On a home network only loopback forwards.
        assert_eq!(
            client_address(peer("192.168.1.5"), ForwardedFrom::Loopback, &h),
            "192.168.1.5".parse::<IpAddr>().unwrap()
        );
        assert_eq!(
            client_address(
                peer("127.0.0.1"),
                ForwardedFrom::Loopback,
                &HeaderMap::new()
            ),
            IpAddr::V4(Ipv4Addr::UNSPECIFIED)
        );
        assert_eq!(
            client_address(None, ForwardedFrom::Private, &h),
            IpAddr::V4(Ipv4Addr::UNSPECIFIED)
        );
        assert_eq!(
            client_address(
                peer("::ffff:10.0.0.2"),
                ForwardedFrom::Private,
                &forwarded("::ffff:203.0.113.9")
            ),
            "203.0.113.9".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn only_the_proxys_own_entry_is_believed_whatever_it_looks_like() {
        let at = |value: &str, trust| client_address(peer("10.0.0.2"), trust, &forwarded(value));
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        // A port, as some proxies append one.
        assert_eq!(
            at("6.6.6.6, 203.0.113.9:51234", ForwardedFrom::Private),
            ip("203.0.113.9")
        );
        assert_eq!(
            at("6.6.6.6, [2001:db8::1]:443", ForwardedFrom::Private),
            ip("2001:db8::1")
        );
        assert_eq!(
            at("6.6.6.6, [2001:db8::1]", ForwardedFrom::Private),
            ip("2001:db8::1")
        );
        // An entry that is no address never hands the choice to the client's
        // own entries before it.
        let nobody = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
        for last in ["unknown", "", "_hidden"] {
            let value = format!("6.6.6.6, {last}");
            assert_eq!(at(&value, ForwardedFrom::Private), nobody, "{value}");
            assert_eq!(
                client_address(
                    peer("127.0.0.1"),
                    ForwardedFrom::Loopback,
                    &forwarded(&value)
                ),
                IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                "{value}"
            );
        }
        // A proxy that adds a header of its own rather than appending: its
        // line is the last.
        let mut h = forwarded("6.6.6.6");
        h.append("x-forwarded-for", "203.0.113.9".parse().unwrap());
        assert_eq!(
            client_address(peer("10.0.0.2"), ForwardedFrom::Private, &h),
            ip("203.0.113.9")
        );
        h.append("x-forwarded-for", "unknown".parse().unwrap());
        assert_eq!(
            client_address(peer("10.0.0.2"), ForwardedFrom::Private, &h),
            nobody
        );
        // A client's bytes that are not text, on the line the proxy appended
        // to, leave the proxy's entry readable.
        let mut h = HeaderMap::new();
        h.insert(
            "x-forwarded-for",
            http::HeaderValue::from_bytes(b"\xff, 203.0.113.9").unwrap(),
        );
        assert_eq!(
            client_address(peer("10.0.0.2"), ForwardedFrom::Private, &h),
            ip("203.0.113.9")
        );
    }

    const SETTINGS: (&str, &str) = ("proxy_header", "proxy_from");

    #[test]
    fn the_proxy_is_heard_only_from_its_own_address_and_naming_one_account() {
        let proxy = ProxyAuth::from_config("x-user", &["10.0.0.0/8".into()], SETTINGS)
            .unwrap()
            .unwrap();
        let mut h = HeaderMap::new();
        h.insert("x-user", "ann".parse().unwrap());
        let from = |ip: &str| Some(ip.parse::<IpAddr>().unwrap());
        assert_eq!(proxy.user(from("10.1.2.3"), &h), Vouch::Named("ann"));
        assert_eq!(proxy.user(from("::ffff:10.1.2.3"), &h), Vouch::Named("ann"));
        assert_eq!(proxy.user(from("203.0.113.9"), &h), Vouch::Absent);
        assert_eq!(proxy.user(None, &h), Vouch::Absent);
        assert_eq!(
            proxy.user(from("10.1.2.3"), &HeaderMap::new()),
            Vouch::Absent
        );
        h.append("x-user", "bob".parse().unwrap());
        assert_eq!(proxy.user(from("10.1.2.3"), &h), Vouch::Unusable);
        let mut merged = HeaderMap::new();
        merged.insert("x-user", "ann, bob".parse().unwrap());
        assert_eq!(proxy.user(from("10.1.2.3"), &merged), Vouch::Unusable);
    }

    #[test]
    fn half_a_proxy_or_one_believed_from_everywhere_is_refused() {
        assert!(ProxyAuth::from_config("", &[], SETTINGS).unwrap().is_none());
        assert!(ProxyAuth::from_config("x-user", &[], SETTINGS).is_err());
        assert!(ProxyAuth::from_config("", &["10.0.0.1".into()], SETTINGS).is_err());
        assert!(ProxyAuth::from_config("x user", &["10.0.0.1".into()], SETTINGS).is_err());
        assert!(ProxyAuth::from_config("x-user", &["nowhere".into()], SETTINGS).is_err());
        assert!(ProxyAuth::from_config("x-user", &["0.0.0.0/0".into()], SETTINGS).is_err());
        assert!(ProxyAuth::from_config("x-user", &["::ffff:0.0.0.0/96".into()], SETTINGS).is_err());
    }
}
