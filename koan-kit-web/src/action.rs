//! What a control asks of the server, and the signal that says it is asking.
//!
//! A control that sends a request shows busy for exactly as long as its own request runs. Datastar's
//! `data-indicator` counts only requests raised by the element that carries it, so the signal is named for the
//! control: a hash of what it is (its method, URL, form, group, id and label), in lowercase hex, since HTML lowercases
//! attribute names and a key with capitals would declare one signal and read another. The same control drawn twice
//! names the same signal, so a fragment a live stream draws again is byte for byte the one it drew before, and a redraw
//! landing while a request runs leaves its spinner alone. Its leading `_` keeps it out of every request Datastar sends.

use std::cell::Cell;
use std::collections::hash_map::RandomState;
use std::fmt;
use std::hash::{BuildHasher, Hasher};

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
///
/// Each control has a signal of its own, named by what it is. Datastar's indicator sets its signal false whenever an
/// element carrying it is bound or removed, so two controls sharing one would clear each other's busy state mid-request
/// when a patch adds or removes either; two controls on a page that are alike in every part (one label, one URL) are
/// told apart by an id. A key names a signal several controls share on purpose; those send their requests with
/// Datastar's cancellation off, since it keys running requests by method and URL, and one row's press would otherwise
/// abort another's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Busy {
    /// This control's alone.
    Own(u64),
    /// Shared by every control given the key.
    Keyed(u64),
}

impl Busy {
    /// The signal of the control made of `parts`: the same parts name the same signal on every render.
    pub(crate) fn own(parts: &[&str]) -> Self {
        let bytes = parts
            .iter()
            .flat_map(|part| part.as_bytes().iter().chain(std::iter::once(&0)));
        Self::Own(fnv1a(b"own ".iter().chain(bytes)) & 0xffff_ffff_ffff)
    }

    /// The signal named by `key`, shared by every control given it.
    pub(crate) fn keyed(key: &str) -> Self {
        Self::Keyed(fnv1a(b"key ".iter().chain(key.as_bytes())) & 0xffff_ffff_ffff)
    }

    /// Datastar's request options beyond the default: cancellation off for a shared signal.
    pub(crate) fn options(self) -> &'static str {
        match self {
            Self::Own(_) => "",
            Self::Keyed(_) => "requestCancellation: 'disabled'",
        }
    }
}

/// `_busy` and the hash, lowercase.
impl fmt::Display for Busy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (Self::Own(hash) | Self::Keyed(hash)) = self;
        write!(f, "_busy{hash:012x}")
    }
}

thread_local! {
    /// A per-thread random seed and a count, so a flash's id differs across renders, threads and server restarts, a
    /// tab kept open across a restart included.
    static SERIAL: Cell<(u64, u64)> = Cell::new((RandomState::new().build_hasher().finish(), 0));
}

/// A number no earlier call returned, for a name the page must not already hold.
pub(crate) fn serial() -> u64 {
    SERIAL.with(|serial| {
        let (seed, count) = serial.get();
        serial.set((seed, count + 1));
        fnv1a(seed.to_le_bytes().iter().chain(count.to_le_bytes().iter()))
    })
}

/// Makes this thread's serials repeat from the start: the storybook is written the same way every time.
#[cfg(any(test, feature = "storybook"))]
pub(crate) fn repeat_serials() {
    SERIAL.with(|serial| serial.set((0, 0)));
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
    fn a_control_is_named_by_what_it_is() {
        assert_eq!(Busy::own(&["post", "/scan"]), Busy::own(&["post", "/scan"]));
        assert_ne!(
            Busy::own(&["post", "/scan"]),
            Busy::own(&["post", "/scan", "again"])
        );
        assert_ne!(
            Busy::own(&["post", "/a", "b"]),
            Busy::own(&["post", "/a b"]),
            "parts are kept apart"
        );
        assert_eq!(Busy::keyed("card-1"), Busy::keyed("card-1"));
        assert_ne!(Busy::keyed("card-1"), Busy::keyed("card-2"));
    }

    #[test]
    fn a_busy_signal_survives_html_lowercasing() {
        for busy in [Busy::own(&["POST", "/A"]), Busy::keyed("Row-ABC")] {
            let name = busy.to_string();
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

    #[test]
    fn a_shared_signal_turns_cancellation_off() {
        assert_eq!(
            Busy::keyed("x").options(),
            "requestCancellation: 'disabled'"
        );
        assert_eq!(Busy::own(&[]).options(), "");
    }
}
