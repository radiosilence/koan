use std::sync::Arc;
use std::time::Duration;

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
    /// Playback state updates — pushes on state change and position at the given interval.
    /// Default interval: 200ms (5Hz). Override with `intervalMs` for faster/slower updates.
    /// Minimum 16ms (~60fps) — values below this are clamped to prevent CPU waste.
    async fn now_playing(
        &self,
        ctx: &Context<'_>,
        #[graphql(
            default = 200,
            desc = "Push interval in milliseconds (minimum 16). Default 200 (5Hz)."
        )]
        interval_ms: u64,
    ) -> impl Stream<Item = GqlNowPlaying> {
        let state = ctx.data_unchecked::<Arc<SharedPlayerState>>().clone();
        let interval = Duration::from_millis(interval_ms.max(16)); // floor at ~60fps

        async_stream::stream! {
            let mut last_state = 255u8; // impossible value to force first emit
            let mut last_position = u64::MAX;
            let mut last_queue_item: Option<String> = None;

            loop {
                let playback_state = state.playback_state() as u8;
                let position_ms = state.position_ms();
                let queue_item_id = state.track_info().map(|ti| ti.id.0.to_string());

                // Emit on any change: state, position, or track.
                let changed = playback_state != last_state
                    || position_ms != last_position
                    || queue_item_id != last_queue_item;

                if changed {
                    last_state = playback_state;
                    last_position = position_ms;
                    last_queue_item = queue_item_id;

                    yield GqlNowPlaying::capture(&state);
                }

                tokio::time::sleep(interval).await;
            }
        }
    }

    /// Queue updates — pushes the full queue snapshot whenever the playlist version changes.
    async fn queue_updated(
        &self,
        ctx: &Context<'_>,
        #[graphql(default = 500, desc = "Poll interval in milliseconds. Default 500.")]
        interval_ms: u64,
    ) -> impl Stream<Item = GqlQueueSnapshot> {
        let state = ctx.data_unchecked::<Arc<SharedPlayerState>>().clone();
        let interval = Duration::from_millis(interval_ms.max(50));

        async_stream::stream! {
            let mut last_version = u64::MAX; // force first emit

            loop {
                let version = state.playlist_version();

                if version != last_version {
                    last_version = version;
                    yield GqlQueueSnapshot::capture(&state);
                }

                tokio::time::sleep(interval).await;
            }
        }
    }

    /// Visualizer frames — spectrum, peaks, VU, beat energy, optional waveform.
    /// Pushes at `fps` rate (default 30). Set `includeWaveform` for oscilloscope modes.
    async fn viz_frame(
        &self,
        ctx: &Context<'_>,
        #[graphql(default = 30, desc = "Target frames per second. Default 30.")] fps: u32,
        #[graphql(default = false, desc = "Include raw waveform samples. Default false.")]
        include_waveform: bool,
    ) -> impl Stream<Item = GqlVizFrame> {
        let viz = ctx.data_opt::<Arc<VizSnapshot>>().cloned();
        let interval = Duration::from_millis((1000 / fps.clamp(1, 120)) as u64);

        async_stream::stream! {
            let Some(viz) = viz else {
                // No VizSnapshot — nothing to push.
                return;
            };

            loop {
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

                tokio::time::sleep(interval).await;
            }
        }
    }
}
