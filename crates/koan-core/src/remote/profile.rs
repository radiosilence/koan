//! What the server this client signs in to turns out to be.
//!
//! koan's own additions to Subsonic are listed as OpenSubsonic extensions, and
//! a client decides what to offer from that list rather than from the server
//! calling itself koan: a koan server older than an extension does not list it,
//! and a server that is not koan can list one (the hub puts devices in front of
//! Navidrome). Probed once per sign-in and kept.

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::remote::client::SubsonicAuth;

/// The standing connection a server can command a client over:
/// `/rest/koanLink`. See `remote::link`.
pub const LINK: &str = "koanLink";
/// A person's devices seeing and commanding each other through the server:
/// the account's other devices sent down the link, commands relayed between
/// them, handoff, and `/rest/koanCommand` for a device whose link is down.
pub const DEVICES: &str = "koanDevices";
/// Invite links: `/rest/koanJoin` trades the token one carries for an API
/// key. See `crate::invite`.
pub const INVITE: &str = "koanInvite";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerProfile {
    /// OpenSubsonic's `type`: `koan`, `navidrome`. `None` from a server that
    /// predates OpenSubsonic.
    pub kind: Option<String>,
    pub version: Option<String>,
    pub open_subsonic: bool,
    /// Each extension's name and the versions offered.
    pub extensions: Vec<(String, Vec<i64>)>,
}

impl ServerProfile {
    pub fn offers(&self, extension: &str) -> bool {
        self.extensions.iter().any(|(name, _)| name == extension)
    }

    /// Whether a client should link. koan servers linked before the link was
    /// listed as an extension, so the name still counts.
    pub fn links(&self) -> bool {
        self.offers(LINK) || self.kind.as_deref() == Some("koan")
    }
}

/// The last profile probed, whose credentials it was for, and whether it is
/// still to be trusted without asking again.
static PROFILE: Mutex<Option<(SubsonicAuth, ServerProfile, bool)>> = Mutex::new(None);

/// The profile of the server `auth` signs in to, probing it on first ask.
/// `None` while it cannot be reached; asked again next time.
pub fn for_auth(auth: &SubsonicAuth) -> Option<ServerProfile> {
    if let Some((held, profile, true)) = PROFILE.lock().as_ref()
        && held == auth
    {
        return Some(profile.clone());
    }
    let client = crate::remote::client::SubsonicClient::from_auth(auth.clone());
    let profile = match client.profile() {
        Ok(p) => p,
        Err(e) => {
            log::info!("profile: {} did not answer: {e}", auth.base_url);
            return None;
        }
    };
    log::info!(
        "profile: {} is {} {} ({} extensions)",
        auth.base_url,
        profile.kind.as_deref().unwrap_or("a Subsonic server"),
        profile.version.as_deref().unwrap_or(""),
        profile.extensions.len()
    );
    *PROFILE.lock() = Some((auth.clone(), profile.clone(), true));
    crate::remote::devices::touch();
    Some(profile)
}

/// Probe again on next ask: the server may have changed under a dropped link.
/// What was found stays on show until then.
pub fn forget() {
    if let Some(held) = PROFILE.lock().as_mut() {
        held.2 = false;
    }
}

/// The configured server's profile as last probed, without asking it.
pub fn current() -> Option<ServerProfile> {
    let cfg = crate::config::Config::load().unwrap_or_default();
    let auth = crate::helpers::subsonic_auth(&cfg)?;
    PROFILE
        .lock()
        .as_ref()
        .filter(|(held, _, _)| *held == auth)
        .map(|(_, p, _)| p.clone())
}

/// Whether the configured server is koan, probing it if it has not been.
pub fn is_koan(auth: &SubsonicAuth) -> bool {
    for_auth(auth).is_some_and(|p| p.kind.as_deref() == Some("koan"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_older_koan_server_still_links() {
        let old = ServerProfile {
            kind: Some("koan".into()),
            open_subsonic: true,
            ..Default::default()
        };
        assert!(old.links());
        assert!(!old.offers(DEVICES));
        let hub = ServerProfile {
            kind: Some("navidrome".into()),
            extensions: vec![(LINK.into(), vec![1]), (DEVICES.into(), vec![1])],
            ..Default::default()
        };
        assert!(hub.links() && hub.offers(DEVICES));
        assert!(!ServerProfile::default().links());
    }
}
