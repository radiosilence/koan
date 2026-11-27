//! koan's own extensions to the Subsonic API: the `koan*` endpoints its apps
//! use beyond what OpenSubsonic defines, specified in `docs/reference/protocol/`.

use super::*;

mod accounts;
mod dsp;
mod history;
mod link;
mod pairing;
mod scrobbling;
mod sharing;

pub(super) use accounts::*;
pub(super) use dsp::*;
pub(super) use history::*;
pub(super) use link::*;
pub(super) use pairing::*;
pub(super) use scrobbling::*;
pub(super) use sharing::*;
