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

/// An open session with a renderer, for the player to play to.
///
/// What the renderer is heard to do reaches the player as
/// `PlayerCommand::Renderer`, tagged with the number of the player session it
/// was heard in: `tag`, which the player keeps up to date. An event from a
/// session already over is recognised and dropped.
#[derive(Debug)]
pub struct Connection {
    pub session: session::Session,
    pub tag: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

/// Open a session with `renderer` whose events go to `player`.
pub fn open(
    renderer: Renderer,
    player: &crossbeam_channel::Sender<crate::player::commands::PlayerCommand>,
) -> Result<Connection, String> {
    let tag = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let tx = player.clone();
    let tagged = tag.clone();
    let session = session::Session::open(renderer, move |event| {
        let _ = tx.send(crate::player::commands::PlayerCommand::Renderer {
            session: tagged.load(std::sync::atomic::Ordering::Acquire),
            event,
        });
    })?;
    Ok(Connection { session, tag })
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
    let connection = open(renderer, player)?;
    if CHOICE.load(std::sync::atomic::Ordering::Acquire) != choice {
        log::info!(
            "upnp: {} opened after a later choice of output; not used",
            connection.session.renderer().name
        );
        return Ok(());
    }
    player
        .send(crate::player::commands::PlayerCommand::UseRenderer(Some(
            Box::new(connection),
        )))
        .map_err(|_| "The player has stopped.".to_string())
}

/// Bring the music back to this device's own output.
pub fn disconnect(player: &crossbeam_channel::Sender<crate::player::commands::PlayerCommand>) {
    choose();
    let _ = player.send(crate::player::commands::PlayerCommand::UseRenderer(None));
}
