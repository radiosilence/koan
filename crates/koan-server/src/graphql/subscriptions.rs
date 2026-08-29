use std::sync::Arc;

use async_graphql::{Context, Subscription};
use tokio_stream::Stream;

use koan_core::audio::viz::VizSnapshot;
use koan_core::player::state::SharedPlayerState;

use super::types::*;

// ---------------------------------------------------------------------------
// Subscription root
// ---------------------------------------------------------------------------

pub struct SubscriptionRoot;

#[Subscription]
impl SubscriptionRoot {
    /// Playback updates, pushed when something changes.
    ///
    /// `positionMs` is an anchor rather than a reading. A playhead advancing at
    /// one second per second is the one thing a client can work out for itself,
    /// so this pushes when that stops being true — a seek, a pause, a track
    /// boundary, a stall — and not on a clock. Derive the current position from
    /// the last message and `state`; that is what the native client does, and
    /// what lets a paused koan send nothing at all.
    async fn now_playing(
        &self,
        ctx: &Context<'_>,
        #[graphql(
            default = 200,
            desc = "Ignored. Kept so existing queries still parse — this stream \
                    pushes on change rather than on an interval."
        )]
        interval_ms: u64,
    ) -> impl Stream<Item = GqlNowPlaying> {
        let _ = interval_ms; // Kept in the schema, not used — see above.
        let state = ctx.data_unchecked::<Arc<SharedPlayerState>>().clone();
        let mut wake = koan_core::signal::engine_changed().subscribe();

        async_stream::stream! {
            let mut last_state = 255u8; // impossible value to force first emit
            let mut last_position = u64::MAX;
            let mut last_queue_item: Option<String> = None;

            loop {
                let playback_state = state.playback_state() as u8;
                let position_ms = state.position_ms();
                let queue_item_id = state.track_info().map(|ti| ti.id.0.to_string());

                let changed = playback_state != last_state
                    || position_ms != last_position
                    || queue_item_id != last_queue_item;

                if changed {
                    last_state = playback_state;
                    last_position = position_ms;
                    last_queue_item = queue_item_id;

                    yield GqlNowPlaying::capture(&state);
                }

                // Marked seen before the wait, so a change landing between the
                // read above and this cannot be slept through.
                wake.borrow_and_update();
                if wake.changed().await.is_err() {
                    return; // The engine is gone; so is this stream.
                }
            }
        }
    }

    /// Queue updates — the full snapshot whenever the playlist changes, and
    /// while anything is downloading, since progress moves without the
    /// playlist version moving.
    ///
    /// Woken rather than polled: the download store takes a rate reading as the
    /// bytes land and says so, which is the same signal a queue edit sends.
    async fn queue_updated(
        &self,
        ctx: &Context<'_>,
        #[graphql(
            default = 500,
            desc = "Ignored. Kept so existing queries still parse — this stream \
                    pushes on change rather than on an interval."
        )]
        interval_ms: u64,
    ) -> impl Stream<Item = GqlQueueSnapshot> {
        let _ = interval_ms; // Kept in the schema, not used — see above.
        let state = ctx.data_unchecked::<Arc<SharedPlayerState>>().clone();
        let mut wake = koan_core::signal::engine_changed().subscribe();

        async_stream::stream! {
            let mut last_version = u64::MAX; // force first emit

            loop {
                let version = state.playlist_version();
                let downloading = !state.downloads_in_flight().is_empty();

                if version != last_version || downloading {
                    last_version = version;
                    yield GqlQueueSnapshot::capture(&state);
                }

                wake.borrow_and_update();
                if wake.changed().await.is_err() {
                    return;
                }
            }
        }
    }

    /// Visualizer frames — spectrum, peaks, VU, beat energy, optional waveform.
    /// Set `includeWaveform` for oscilloscope modes.
    ///
    /// One message per analysed frame: `fps` sets the rate the analyser itself
    /// runs at rather than a rate to resample it at, so no frame is sent twice
    /// and none is skipped. The analyser stops publishing when the play head
    /// does, so a paused koan sends nothing and this stream costs nothing.
    ///
    /// The rate is the analyser's, and it is one analyser: a second client
    /// asking for a different `fps` moves it for both.
    async fn viz_frame(
        &self,
        ctx: &Context<'_>,
        #[graphql(
            default = 30,
            desc = "Frames per second to run the analyser at. Default 30."
        )]
        fps: u32,
        #[graphql(default = false, desc = "Include raw waveform samples. Default false.")]
        include_waveform: bool,
    ) -> impl Stream<Item = GqlVizFrame> {
        let viz = ctx.data_opt::<Arc<VizSnapshot>>().cloned();

        async_stream::stream! {
            let Some(viz) = viz else {
                // No VizSnapshot — nothing to push.
                return;
            };
            viz.set_fps(fps.clamp(1, 240) as u8);
            // Counted as a reader before the first wait: an analyser parked for
            // want of one would otherwise never publish the frame this waits
            // for. Every read below keeps it counted.
            viz.touch();
            let mut published = viz.subscribe();

            loop {
                published.borrow_and_update();
                if published.changed().await.is_err() {
                    return;
                }
                let frame = viz.read();
                yield GqlVizFrame {
                    spectrum: frame.spectrum.to_vec(),
                    peaks: frame.peaks.to_vec(),
                    vu_levels: frame.vu_levels.to_vec(),
                    beat_energy: frame.beat_energy,
                    waveform: if include_waveform {
                        frame.waveform.clone()
                    } else {
                        Vec::new()
                    },
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    /// The arguments stay in the schema even though the streams no longer run
    /// on them: a query that named one still has to parse.
    #[test]
    fn the_interval_arguments_are_still_in_the_schema() {
        let (state, _timeline, _viz, cmd_tx) = koan_core::player::Player::spawn();
        let schema = crate::graphql::build_schema(
            state,
            cmd_tx,
            std::path::PathBuf::from("/nonexistent/koan-test.db"),
            None,
        );
        let sdl = schema.sdl();
        let subscription = sdl
            .split("type SubscriptionRoot")
            .nth(1)
            .expect("subscription root in SDL");
        assert!(subscription.contains("intervalMs"), "{subscription}");
        assert!(subscription.contains("fps"), "{subscription}");
    }
}
