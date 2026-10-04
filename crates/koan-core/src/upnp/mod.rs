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
pub mod stream;
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
        let cmd = crate::player::commands::PlayerCommand::Renderer {
            session: tagged.load(std::sync::atomic::Ordering::Acquire),
            event,
        };
        match &cmd {
            // Only a wake-up: the player reads the loss from the session. It
            // can be raised on the player's own thread, or under the
            // discovery lock, where waiting for room in the channel could
            // never end.
            crate::player::commands::PlayerCommand::Renderer {
                event: session::Event::Gone,
                ..
            } => {
                let _ = tx.try_send(cmd);
            }
            _ => {
                let _ = tx.send(cmd);
            }
        }
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

/// How long after launch the renderer used last time is looked for.
const RESUME_WINDOW: std::time::Duration = std::time::Duration::from_secs(6);

/// Go back to the renderer `udn`, used last time, on a thread of its own:
/// look for it for `RESUME_WINDOW`, waking on each change discovery
/// announces, and hand the player an open session as
/// `PlayerCommand::ResumeRenderer` if it turns up idle. The player takes it
/// only if nobody has played anything or picked an output meanwhile.
pub fn resume(
    udn: String,
    player: &crossbeam_channel::Sender<crate::player::commands::PlayerCommand>,
) {
    let choice = CHOICE.load(std::sync::atomic::Ordering::Acquire);
    let player = player.clone();
    let _ = std::thread::Builder::new()
        .name("koan-upnp-resume".into())
        .spawn(move || {
            let _ = player.send(match resume_onto(&udn, RESUME_WINDOW, choice, &player) {
                Some(connection) => {
                    crate::player::commands::PlayerCommand::ResumeRenderer(Box::new(connection))
                }
                None => crate::player::commands::PlayerCommand::ResumeRendererMissed,
            });
        });
}

/// The session `resume` hands over, if the renderer is found within `window`,
/// nothing else has been picked since `choice`, and it is not playing or
/// paused for something else: whatever is driving it, a phone on the same
/// amplifier say, is not taken over by a launch nobody asked for. Still on a
/// track a kōan on this machine sent it, a run that ended without stopping
/// it, it is ours to take back.
fn resume_onto(
    udn: &str,
    window: std::time::Duration,
    choice: u64,
    player: &crossbeam_channel::Sender<crate::player::commands::PlayerCommand>,
) -> Option<Connection> {
    let Some(renderer) = await_renderer(udn, window) else {
        log::info!("upnp: {udn}, used last time, is not on the network; playing here");
        return None;
    };
    if CHOICE.load(std::sync::atomic::Ordering::Acquire) != choice {
        return None;
    }
    match discovery::in_use(&renderer) {
        Some(false) => {}
        Some(true)
            if discovery::playing_uri(&renderer)
                .is_some_and(|uri| served_from_here(&renderer, &uri) && !still_served(&uri)) =>
        {
            log::info!(
                "upnp: {} is still on what a kōan here sent it; taking it back",
                renderer.name
            );
        }
        _ => {
            log::info!(
                "upnp: {}, used last time, is busy or not answering; playing here",
                renderer.name
            );
            return None;
        }
    }
    open(renderer, player)
        .inspect_err(|e| log::info!("upnp: could not go back to {udn}: {e}"))
        .ok()
}

/// Whether `uri` is one a kōan on this machine served `renderer`: a track's
/// tokenised path (`serve`'s `/t/<token>`) on this machine's address towards
/// it. The port is not compared: every run listens on a new one.
fn served_from_here(renderer: &Renderer, uri: &str) -> bool {
    let Ok(url) = url::Url::parse(uri) else {
        return false;
    };
    let host = |u: &url::Url| {
        u.host_str()
            .and_then(|h| h.parse::<std::net::IpAddr>().ok())
    };
    let Some(here) = host(&renderer.location).and_then(serve::local_ip_towards) else {
        return false;
    };
    let token = url
        .path()
        .strip_prefix("/t/")
        .and_then(|t| t.split('.').next())
        .unwrap_or_default();
    host(&url) == Some(here) && token.len() == 32 && token.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Whether a kōan is still serving `uri`: another one on this machine, a
/// TUI beside the app say, is playing to the renderer, and it is not ours to
/// take. A run that has ended refuses the connection; a live one that has let
/// the track go answers 404.
fn still_served(uri: &str) -> bool {
    use std::io::{Read, Write};
    let Ok(url) = url::Url::parse(uri) else {
        return false;
    };
    let Some(addr) = url
        .socket_addrs(|| Some(80))
        .ok()
        .and_then(|a| a.into_iter().next())
    else {
        return false;
    };
    let timeout = std::time::Duration::from_millis(500);
    let Ok(mut stream) = std::net::TcpStream::connect_timeout(&addr, timeout) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(timeout));
    if write!(stream, "HEAD {} HTTP/1.1\r\nHost: x\r\n\r\n", url.path()).is_err() {
        return false;
    }
    let mut head = [0u8; 12];
    stream.read_exact(&mut head).is_ok() && head.starts_with(b"HTTP/1.1 2")
}

/// The renderer `udn` once discovery has found it, waiting at most `window`.
fn await_renderer(udn: &str, window: std::time::Duration) -> Option<Renderer> {
    let signal = crate::signal::engine_changed();
    let mut seen = signal.generation();
    let deadline = std::time::Instant::now() + window;
    discovery::search();
    loop {
        if let Some(renderer) = discovery::find(udn) {
            return Some(renderer);
        }
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return None;
        }
        seen = signal.wait_until(seen, left);
    }
}

/// Bring the music back to this device's own output.
pub fn disconnect(player: &crossbeam_channel::Sender<crate::player::commands::PlayerCommand>) {
    choose();
    let _ = player.send(crate::player::commands::PlayerCommand::UseRenderer(None));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A renderer discovery knows is found at once; one it never hears of
    /// is given up on when the window closes, and the music stays here.
    #[test]
    fn a_remembered_renderer_is_found_or_given_up_on() {
        let fake = fake::FakeRenderer::start("http-get:*:audio/wav:*", true, false);
        let renderer = fake.renderer();
        discovery::remember(renderer.clone(), std::time::Duration::from_secs(60));
        assert_eq!(
            await_renderer(&renderer.udn, std::time::Duration::from_secs(1)).map(|r| r.udn),
            Some(renderer.udn)
        );

        let start = std::time::Instant::now();
        let window = std::time::Duration::from_millis(300);
        assert!(await_renderer("uuid:nowhere", window).is_none());
        assert!(start.elapsed() >= window);
    }

    /// A renderer playing for something else when the app launches is left
    /// to it; an idle one is gone back to.
    #[test]
    fn a_busy_renderer_is_not_taken_at_launch() {
        let fake = fake::FakeRenderer::start("http-get:*:audio/wav:*", true, false);
        let renderer = fake.renderer();
        discovery::remember(renderer.clone(), std::time::Duration::from_secs(60));
        let (tx, _rx) = crossbeam_channel::unbounded();
        let window = std::time::Duration::from_secs(1);
        let now = CHOICE.load(std::sync::atomic::Ordering::Acquire);

        fake.play_foreign("http://phone/track.flac");
        assert!(resume_onto(&renderer.udn, window, now, &tx).is_none());

        // Still on a track a kōan here sent it, from a run that ended
        // without stopping it: ours, taken back.
        let gone = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let gone_port = gone.local_addr().unwrap().port();
        drop(gone);
        fake.play_foreign(&format!(
            "http://127.0.0.1:{gone_port}/t/{}.flac",
            "0a".repeat(16)
        ));
        assert!(resume_onto(&renderer.udn, window, now, &tx).is_some());

        // On a track a kōan here is still serving, a TUI beside the app say:
        // that one's, left alone.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("t.flac");
        std::fs::write(&file, b"fLaC").unwrap();
        let live = serve::Listener::start(Box::new(|_, _| {})).unwrap();
        let token = live.add(serve::Served::File {
            path: file,
            mime: "audio/flac".into(),
        });
        fake.play_foreign(&format!("http://127.0.0.1:{}/t/{token}.flac", live.port()));
        assert!(resume_onto(&renderer.udn, window, now, &tx).is_none());

        fake.press_stop(0);
        assert!(resume_onto(&renderer.udn, window, now, &tx).is_some());
    }
}
