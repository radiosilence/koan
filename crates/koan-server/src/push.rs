//! Push to kōan's iOS app, through kōan's push relay.
//!
//! A linked app is reached over its WebSocket. iOS suspends an app in the
//! background that is not playing, and the socket dies with it; a push is the
//! supported way back. Apple accepts pushes to the app only under the
//! developer's key, so every server, this one included, sends them through
//! the relay at `push.koan.rocks` (`docs/reference/protocol/push-relay.md`),
//! which holds that key. The server holds handles the app gave it, never a
//! device token.
//!
//! Three kinds: a background push that wakes the app for half a minute to
//! link and take what waits for it in the outbox; a notification for
//! playback, which iOS will not let a suspended app start on its own, carrying
//! a claim the app redeems over the link once the person taps it; and Live
//! Activity updates for a device the phone follows.

use koan_relay_client::{ActivityState, Client, Error, Push as Relayed};

/// What to send.
#[derive(Debug, Clone, PartialEq)]
pub enum Push {
    Wake,
    WakeNow,
    /// "Someone wants to play here": tapping opens the app.
    Summon,
    /// A command to run once the person taps it: the claim it is kept under
    /// (see `crate::clients::claims`).
    Notify {
        claim: String,
    },
    Activity(ActivityState),
}

/// What became of a push.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    Sent,
    /// The handle no longer reaches the device: forget it.
    Gone,
    Failed(String),
}

pub struct Pusher {
    client: Client,
}

impl Pusher {
    /// Send `push` through `handle`. Blocks for the round trip, so never call
    /// it from async code.
    pub fn send(&self, handle: &str, push: &Push) -> Outcome {
        let sent = match push {
            Push::Wake => self.client.push(handle, Relayed::Wake),
            Push::WakeNow => self.client.push(handle, Relayed::WakeNow),
            Push::Summon => self.client.push(handle, Relayed::Summon { blob: None }),
            Push::Notify { claim } => self.client.push(
                handle,
                Relayed::Notify {
                    blob: Some(claim.as_str()),
                },
            ),
            Push::Activity(state) => self.client.update_activity(handle, state),
        };
        match sent {
            Ok(()) => Outcome::Sent,
            Err(Error::Gone) => Outcome::Gone,
            Err(e) => Outcome::Failed(e.to_string()),
        }
    }
}

/// What a Live Activity shows of the device it follows.
pub fn activity_state(info: &crate::clients::ClientInfo) -> ActivityState {
    ActivityState {
        device: info.name.clone(),
        linked: true,
        title: info.state.title.clone(),
        artist: info.state.artist.clone(),
        album: info.state.album.clone(),
        playing: info.state.playing,
        position_ms: info.position_ms(),
        duration_ms: info.state.duration_ms,
        at: chrono::Utc::now().timestamp_millis() as f64 / 1000.0,
        art: info
            .state
            .queue
            .iter()
            .find(|e| e.current)
            .and_then(|e| e.track_id.as_deref())
            .and_then(activity_art),
    }
}

/// Whether the activity would show something different from `sent`. A
/// playhead moving on time is drawn by the activity itself.
pub fn differs(state: &ActivityState, sent: &ActivityState) -> bool {
    let strip = |s: &ActivityState| ActivityState {
        position_ms: 0,
        at: 0.0,
        ..s.clone()
    };
    if strip(state) != strip(sent) {
        return true;
    }
    let expected = if sent.playing {
        sent.position_ms + ((state.at - sent.at).max(0.0) * 1000.0) as u64
    } else {
        sent.position_ms
    };
    state.position_ms.abs_diff(expected) > 3000
}

/// The relay takes art of at most this many base64 characters, which keeps a
/// Live Activity push within Apple's 4 KB.
const ART_BUDGET: usize = 2600;

/// The cover of `track` (a uid or a row id) small enough to ride in a Live
/// Activity push, as base64. Worked out once per track: the state it goes in
/// is reported far more often than the track changes.
fn activity_art(track: &str) -> Option<String> {
    use base64::Engine as _;
    static LAST: parking_lot::Mutex<Option<(String, Option<String>)>> =
        parking_lot::Mutex::new(None);
    static COVERS: std::sync::LazyLock<crate::covers::Covers> =
        std::sync::LazyLock::new(crate::covers::Covers::in_config_dir);
    if cfg!(test) {
        return None;
    }
    if let Some((held, art)) = LAST.lock().as_ref()
        && held == track
    {
        return art.clone();
    }
    let art = (|| {
        let db = koan_core::db::pool::shared().get().ok()?;
        let id = koan_core::db::queries::resolve_id(
            &db.conn,
            koan_core::db::queries::UidKind::Track,
            track,
        )
        .ok()??;
        let row = koan_core::db::queries::get_track_row(&db.conn, id).ok()??;
        let cover = COVERS.cover(std::slice::from_ref(&row), crate::covers::SIZES[0])?;
        let img = image::load_from_memory(&cover).ok()?;
        // Smaller and rougher until it fits.
        [(72, 60), (60, 50), (48, 40)]
            .into_iter()
            .find_map(|(side, quality)| {
                let mut out = Vec::new();
                let small = img.thumbnail(side, side).to_rgb8();
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
                    .encode_image(&small)
                    .ok()?;
                let b64 = base64::engine::general_purpose::STANDARD.encode(&out);
                (b64.len() <= ART_BUDGET).then_some(b64)
            })
    })();
    *LAST.lock() = Some((track.to_string(), art.clone()));
    art
}

/// The server's pusher, from the config it started with; `None` when push is
/// turned off, and pushes are simply not sent.
pub fn pusher() -> Option<&'static Pusher> {
    static PUSHER: std::sync::LazyLock<Option<Pusher>> = std::sync::LazyLock::new(|| {
        let cfg = koan_core::config::Config::load().unwrap_or_default();
        if !cfg.push.enabled {
            log::info!("push: off");
            return None;
        }
        match Client::new(&cfg.push.relay) {
            Ok(client) => {
                log::info!("push: through {}", cfg.push.relay);
                Some(Pusher { client })
            }
            Err(e) => {
                log::warn!("push: the relay {} cannot be used: {e}", cfg.push.relay);
                None
            }
        }
    });
    PUSHER.as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(position_ms: u64, at: f64, playing: bool) -> ActivityState {
        ActivityState {
            device: "Desk".into(),
            linked: true,
            title: Some("Blue in Green".into()),
            artist: None,
            album: None,
            playing,
            position_ms,
            duration_ms: 337_000,
            at,
            art: None,
        }
    }

    #[test]
    fn a_playhead_moving_on_time_is_no_change() {
        let sent = state(10_000, 100.0, true);
        assert!(!differs(&state(15_000, 105.0, true), &sent));
        assert!(differs(&state(60_000, 105.0, true), &sent), "a seek");
        assert!(differs(&state(15_000, 105.0, false), &sent), "a pause");
    }

    #[test]
    fn a_paused_playhead_stays_put() {
        let sent = state(10_000, 100.0, false);
        assert!(!differs(&state(10_000, 500.0, false), &sent));
    }
}
