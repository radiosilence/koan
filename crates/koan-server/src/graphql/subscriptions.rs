use std::sync::Arc;
use std::time::Duration;

use async_graphql::{Context, Subscription};
use tokio_stream::Stream;

use koan_core::audio::viz::VizSnapshot;
use koan_core::player::state::{PlaybackState, QueueEntryStatus, SharedPlayerState};

use super::types::*;

// ---------------------------------------------------------------------------
// Subscription root
// ---------------------------------------------------------------------------

pub struct SubscriptionRoot;

#[Subscription]
impl SubscriptionRoot {
    /// Playback state updates — pushes on state change and position at the given interval.
    /// Default interval: 200ms (5Hz). Override with `intervalMs` for faster/slower updates.
    async fn now_playing(
        &self,
        ctx: &Context<'_>,
        #[graphql(
            default = 200,
            desc = "Push interval in milliseconds. Default 200 (5Hz)."
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
                let playback_state = state.playback_state();
                let position_ms = state.position_ms();

                let state_u8 = playback_state as u8;
                let queue_item_id = state.track_info().map(|ti| ti.id.0.to_string());

                // Emit on any change: state, position, or track.
                let changed = state_u8 != last_state
                    || position_ms != last_position
                    || queue_item_id != last_queue_item;

                if changed {
                    last_state = state_u8;
                    last_position = position_ms;
                    last_queue_item = queue_item_id.clone();

                    let playback_enum = match playback_state {
                        PlaybackState::Stopped => PlaybackStateEnum::Stopped,
                        PlaybackState::Playing => PlaybackStateEnum::Playing,
                        PlaybackState::Paused => PlaybackStateEnum::Paused,
                    };

                    let (track, duration_ms) = if let Some(info) = state.track_info() {
                        let (items, _cursor) = state.snapshot_playlist();
                        let playlist_item = items.iter().find(|i| i.id == info.id);
                        let track = GqlNowPlayingTrack {
                            title: playlist_item.map(|i| i.title.clone()).unwrap_or_default(),
                            artist: playlist_item.map(|i| i.artist.clone()).unwrap_or_default(),
                            album: playlist_item.map(|i| i.album.clone()).unwrap_or_default(),
                            codec: info.codec.clone(),
                            sample_rate: info.sample_rate,
                            bit_depth: info.bit_depth,
                            bitrate_kbps: info.bitrate_kbps,
                            channels: info.channels,
                            duration_ms: info.duration_ms,
                        };
                        (Some(track), Some(info.duration_ms))
                    } else {
                        (None, None)
                    };

                    yield GqlNowPlaying {
                        state: playback_enum,
                        position_ms,
                        duration_ms,
                        track,
                        queue_item_id,
                    };
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

                    let snap = state.derive_visible_queue();
                    let entries = snap
                        .entries
                        .iter()
                        .map(|entry| {
                            let status = match entry.status {
                                QueueEntryStatus::Queued => GqlQueueEntryStatus::Queued,
                                QueueEntryStatus::Playing => GqlQueueEntryStatus::Playing,
                                QueueEntryStatus::Played => GqlQueueEntryStatus::Played,
                                QueueEntryStatus::Downloading => GqlQueueEntryStatus::Downloading,
                                QueueEntryStatus::PriorityPending => GqlQueueEntryStatus::PriorityPending,
                                QueueEntryStatus::Failed => GqlQueueEntryStatus::Failed,
                            };

                            let download_progress =
                                entry.download_progress.map(|(downloaded, total)| {
                                    GqlDownloadProgress { downloaded, total }
                                });

                            GqlQueueEntry {
                                queue_item_id: entry.id.0.to_string(),
                                title: entry.title.clone(),
                                artist: entry.artist.clone(),
                                album: entry.album.clone(),
                                codec: entry.codec.clone(),
                                track_number: entry.track_number,
                                disc: entry.disc,
                                duration_ms: entry.duration_ms,
                                is_current: entry.status == QueueEntryStatus::Playing,
                                status,
                                download_progress,
                            }
                        })
                        .collect();

                    yield GqlQueueSnapshot {
                        version,
                        entries,
                        finished_count: snap.finished_count as i32,
                        has_playing: snap.has_playing,
                        queue_count: snap.queue_count as i32,
                    };
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
