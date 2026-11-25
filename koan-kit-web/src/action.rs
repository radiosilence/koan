//! What a control asks of the server, and the signal that says it is asking.
//!
//! A control that sends a request shows busy for exactly as long as its own request runs. Datastar's
//! `data-indicator` counts only requests raised by the element that carries it, so the signal can be named for the
//! request: two controls posting the same thing share it, and any other two never do. The name is a hash of the
//! method and the URL, in lowercase hex, since HTML lowercases attribute names and a key with capitals would declare
//! one signal and read another. Its leading `_` keeps it out of every request Datastar sends.

use std::fmt;

/// How a request goes: a read, or a change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// A read, which changes nothing (§8: nothing changes state on GET).
    Get,
    /// A change, carrying Datastar's request header, which `require_datastar_on_post` checks.
    Post,
}

impl Method {
    /// The Datastar action: `@get` or `@post`.
    pub(crate) fn action(self) -> &'static str {
        match self {
            Self::Get => "@get",
            Self::Post => "@post",
        }
    }

    /// The form method.
    pub(crate) fn form(self) -> &'static str {
        match self {
            Self::Get => "get",
            Self::Post => "post",
        }
    }
}

/// The signal a request's control is busy by: `_busy` and twelve hex digits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Busy(u64);

impl Busy {
    /// The signal for a request by `method` to `url`.
    pub(crate) fn of(method: Method, url: &str) -> Self {
        let hash = fnv1a(
            method
                .action()
                .as_bytes()
                .iter()
                .chain(b" ")
                .chain(url.as_bytes()),
        );
        Self(hash & 0xffff_ffff_ffff)
    }
}

/// `_busy` and the hash, lowercase.
impl fmt::Display for Busy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "_busy{:012x}", self.0)
    }
}

/// FNV-1a, 64-bit: stable across builds and platforms, so a fragment patched in later names the signal the page
/// already has.
pub(crate) fn fnv1a<'b>(bytes: impl IntoIterator<Item = &'b u8>) -> u64 {
    bytes.into_iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_actions_never_share_a_busy_signal() {
        let delete = Busy::of(Method::Post, "/albums/1/delete");
        assert_ne!(delete, Busy::of(Method::Post, "/albums/2/delete"));
        assert_ne!(delete, Busy::of(Method::Get, "/albums/1/delete"));
    }

    #[test]
    fn one_action_shares_its_busy_signal() {
        assert_eq!(
            Busy::of(Method::Post, "/scan"),
            Busy::of(Method::Post, "/scan")
        );
    }

    #[test]
    fn a_busy_signal_survives_html_lowercasing() {
        let name = Busy::of(Method::Post, "/Albums/ABC").to_string();
        assert_eq!(
            name,
            name.to_lowercase(),
            "{name} would be declared under another name"
        );
        assert!(
            name.starts_with("_busy"),
            "{name} would be sent with every request"
        );
    }
}
