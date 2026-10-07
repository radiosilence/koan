//! Who is connected to this device, and whom it is connected to: the link to
//! the server, the account's devices that can control this one through it,
//! and connections on the local network either way. What Settings lists, so
//! that a device using more power than its time on screen explains can be
//! seen to have been driven from elsewhere, and stopped.

use crate::config::Config;
use crate::remote::proof::Peer;
use crate::remote::{devices, nearby};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    Server,
    Network,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Connection {
    /// What `nearby::end` takes: a connection on the network to this device.
    /// The account's devices are ended by revoking their keys on the server.
    pub key: Option<u64>,
    pub via: Via,
    /// The other end can control this device.
    pub inbound: bool,
    /// The other device's id, when it has said.
    pub id: Option<String>,
    /// The device's name, or the server's or a stranger's address.
    pub name: String,
    /// Where on the network it is.
    pub addr: Option<String>,
    /// Who it is: one of the account's devices, one shared with it, or
    /// (`None`) a device that proved nothing.
    pub peer: Option<Peer>,
    /// Unix seconds. `None` where this device cannot know.
    pub since: Option<i64>,
}

/// A device disconnected from Settings and held off: its address, and its
/// name when it proved who it is.
#[derive(Debug, Clone, PartialEq)]
pub struct Held {
    pub addr: String,
    pub name: Option<String>,
}

pub fn held() -> Vec<Held> {
    nearby::held()
        .into_iter()
        .map(|(addr, id)| Held {
            name: id.as_deref().and_then(devices::name_of),
            addr,
        })
        .collect()
}

pub fn list() -> Vec<Connection> {
    let mut out = Vec::new();
    if let Some(since) = devices::linked_since() {
        out.push(Connection {
            key: None,
            via: Via::Server,
            inbound: false,
            id: None,
            name: server_name(&Config::cached().remote.url),
            addr: None,
            peer: None,
            since: Some(since),
        });
        out.extend(
            devices::linked_controllers()
                .into_iter()
                .map(|d| Connection {
                    key: None,
                    via: Via::Server,
                    inbound: true,
                    id: Some(d.id),
                    name: d.name,
                    addr: None,
                    peer: Some(Peer::Own),
                    since: None,
                }),
        );
    }
    out.extend(nearby::sessions().into_iter().map(|s| {
        Connection {
            key: s.key,
            via: Via::Network,
            inbound: s.inbound,
            // A device's id is its own word until it proves it: one that has
            // not is named by its address, never by the device it claims to be.
            name: s
                .id
                .as_deref()
                .filter(|_| s.proven.is_some())
                .and_then(devices::name_of)
                .unwrap_or_else(|| s.addr.clone()),
            id: s.id,
            addr: Some(s.addr),
            peer: s.proven,
            since: Some(s.since),
        }
    }));
    out
}

/// The server as a person would name it: its host.
fn server_name(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split(['/', '?']).next().unwrap_or(rest).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_server_is_named_by_its_host() {
        assert_eq!(
            server_name("https://music.example.com/"),
            "music.example.com"
        );
        assert_eq!(server_name("http://10.0.0.2:4533"), "10.0.0.2:4533");
        assert_eq!(server_name("music.example.com"), "music.example.com");
    }
}
