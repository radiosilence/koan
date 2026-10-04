//! Playing to UPnP AV MediaRenderers: network amplifiers and streamers that
//! fetch a URL they are given and decode it themselves.
//!
//! A renderer is this koan's output, the way a USB DAC is. The queue, cursor,
//! history and everything else stay in the `Player`; only the audio goes
//! elsewhere, as the original file, so playback stays bit-perfect up to the
//! renderer's own DAC. Nothing here involves the server.
//!
//! Hand-rolled on std threads and the blocking reqwest koan already uses:
//! eight AVTransport actions, two RenderingControl ones, SSDP and GENA.

pub mod description;
pub mod didl;
pub mod discovery;
pub mod serve;
pub mod session;
pub mod soap;
mod xml;

#[cfg(test)]
pub(crate) mod fake;

pub use description::Renderer;

/// The renderer this koan is playing to, as the front ends show it.
#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub udn: String,
    pub name: String,
    /// 0–100, when the renderer has a volume control.
    pub volume: Option<u8>,
    /// Why the current track is not playing there, when koan skipped it.
    pub problem: Option<String>,
}

/// An open session with a renderer, and what it hears, for the player.
///
/// The events come on a channel of their own rather than the command
/// channel: a session the player holds must not keep the player's own
/// channel open, or the player would never see its senders go.
#[derive(Debug)]
pub struct Connection {
    pub session: session::Session,
    pub events: crossbeam_channel::Receiver<session::Event>,
}

/// Each choice of output, in the order they were made. Opening a session
/// takes a few round trips; one that finishes after a later choice is
/// dropped, so the last one picked is where the music goes.
static CHOICE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Record a choice of output, made now, and return its place. Called when the
/// choice is made, before any work towards it: every output change (a
/// renderer, a local device, the system default) takes one.
pub fn choose() -> u64 {
    CHOICE.fetch_add(1, std::sync::atomic::Ordering::AcqRel) + 1
}

/// Make the renderer `udn` this koan's output, carrying on from where the
/// music is, unless a later choice was made while the session opened.
/// `choice` is what `choose` returned when this one was picked. Blocks for
/// the few round trips a session takes to open, so call it from a thread
/// nothing is waiting on.
pub fn connect(
    udn: &str,
    choice: u64,
    player: &crossbeam_channel::Sender<crate::player::commands::PlayerCommand>,
) -> Result<(), String> {
    let renderer = discovery::find(udn)
        .ok_or_else(|| "That renderer is no longer on the network.".to_string())?;
    let (tx, events) = crossbeam_channel::unbounded();
    let session = session::Session::open(renderer, move |event| {
        let _ = tx.send(event);
    })?;
    if CHOICE.load(std::sync::atomic::Ordering::Acquire) != choice {
        log::info!(
            "upnp: {} opened after a later choice of output; not used",
            session.renderer().name
        );
        return Ok(());
    }
    player
        .send(crate::player::commands::PlayerCommand::UseRenderer(Some(
            Box::new(Connection { session, events }),
        )))
        .map_err(|_| "The player has stopped.".to_string())
}

/// Bring the music back to this device's own output.
pub fn disconnect(player: &crossbeam_channel::Sender<crate::player::commands::PlayerCommand>) {
    choose();
    let _ = player.send(crate::player::commands::PlayerCommand::UseRenderer(None));
}
