//! koan clients linked to this server, and the way to reach them.
//!
//! A client that syncs from this server holds a WebSocket open at
//! `/rest/koanLink` (see `koan_core::remote::link`). Each is registered here
//! under the account it signed in as, so GraphQL (and MCP through it) can hand
//! one a list of tracks to play: build a playlist on the server, hear it on a
//! phone.

use std::sync::LazyLock;

use koan_core::remote::link::LinkCommand;
use parking_lot::Mutex;
use tokio::sync::mpsc::UnboundedSender;

/// A linked client, as listed.
#[derive(Debug, Clone)]
pub struct ClientInfo {
    pub id: String,
    pub name: String,
    pub platform: String,
    pub username: String,
    /// Unix seconds.
    pub connected_at: i64,
}

struct Entry {
    info: ClientInfo,
    /// The client's own id for itself, so a reconnect replaces its entry.
    device: String,
    tx: UnboundedSender<LinkCommand>,
}

#[derive(Default)]
pub struct Registry {
    entries: Mutex<Vec<Entry>>,
}

/// One registry per process: the WebSocket route and the GraphQL schema are
/// built in different places and both need it.
pub fn registry() -> &'static Registry {
    static REGISTRY: LazyLock<Registry> = LazyLock::new(Registry::default);
    &REGISTRY
}

impl Registry {
    /// Add a client, replacing any earlier connection from the same device
    /// and account. Returns its id.
    pub fn register(
        &self,
        username: &str,
        name: &str,
        platform: &str,
        device: &str,
        tx: UnboundedSender<LinkCommand>,
    ) -> String {
        let id = uuid::Uuid::now_v7().to_string();
        let mut entries = self.entries.lock();
        entries.retain(|e| !(e.device == device && e.info.username == username));
        entries.push(Entry {
            info: ClientInfo {
                id: id.clone(),
                name: name.to_string(),
                platform: platform.to_string(),
                username: username.to_string(),
                connected_at: chrono::Utc::now().timestamp(),
            },
            device: device.to_string(),
            tx,
        });
        id
    }

    pub fn unregister(&self, id: &str) {
        self.entries.lock().retain(|e| e.info.id != id);
    }

    /// Clients `username` may command, newest first; every client for `None`.
    pub fn list(&self, username: Option<&str>) -> Vec<ClientInfo> {
        let mut out: Vec<ClientInfo> = self
            .entries
            .lock()
            .iter()
            .filter(|e| username.is_none_or(|u| e.info.username == u))
            .map(|e| e.info.clone())
            .collect();
        out.sort_by_key(|c| std::cmp::Reverse(c.connected_at));
        out
    }

    /// Send to `id`, or with no id to the newest client `username` may
    /// command. The client that received it, or `None` when there was none to
    /// send to.
    pub fn send(
        &self,
        username: Option<&str>,
        id: Option<&str>,
        cmd: LinkCommand,
    ) -> Option<ClientInfo> {
        let target = match id {
            Some(id) => self
                .list(username)
                .into_iter()
                .find(|c| c.id == id || c.name.eq_ignore_ascii_case(id))?,
            None => self.list(username).into_iter().next()?,
        };
        let entries = self.entries.lock();
        let entry = entries.iter().find(|e| e.info.id == target.id)?;
        entry.tx.send(cmd).ok()?;
        Some(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reconnect_replaces_the_device_and_commands_reach_it() {
        let reg = Registry::default();
        let (tx1, _rx1) = tokio::sync::mpsc::unbounded_channel();
        let (tx2, mut rx2) = tokio::sync::mpsc::unbounded_channel();
        let (tx3, _rx3) = tokio::sync::mpsc::unbounded_channel();
        reg.register("j", "phone", "ios", "dev-1", tx1);
        let id = reg.register("j", "phone", "ios", "dev-1", tx2);
        reg.register("someone", "laptop", "macos", "dev-2", tx3);

        assert_eq!(reg.list(Some("j")).len(), 1);
        assert_eq!(reg.list(None).len(), 2);

        let sent = reg.send(Some("j"), None, LinkCommand::Pause).unwrap();
        assert_eq!(sent.id, id);
        assert_eq!(rx2.try_recv().unwrap(), LinkCommand::Pause);

        // Another account's device is not this account's to command.
        assert!(
            reg.send(Some("j"), Some("laptop"), LinkCommand::Pause)
                .is_none()
        );

        reg.unregister(&id);
        assert!(reg.send(Some("j"), None, LinkCommand::Pause).is_none());
    }
}
