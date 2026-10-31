use std::sync::Arc;

use async_graphql::{Context, Subscription};
use tokio_stream::Stream;

use koan_core::audio::viz::VizSnapshot;
use koan_core::player::state::SharedPlayerState;

use super::jobs::JobRegistry;
use super::types::*;

// ---------------------------------------------------------------------------
// Subscription root
// ---------------------------------------------------------------------------

/// How often `queueUpdated` sends while only download progress moves.
const PROGRESS_EVERY: std::time::Duration = std::time::Duration::from_secs(1);

/// How far the playhead may stray from where a client counting on from the
/// last message puts it before the stream says so again.
const DRIFT_MS: u64 = 1000;

/// Whether `position_ms` is not where a client counting from `sent_ms`,
/// sent `since` ago, has it: a seek, a stall, a jump.
fn off_course(sent_ms: u64, playing: bool, since: std::time::Duration, position_ms: u64) -> bool {
    let expected = if playing {
        sent_ms.saturating_add(since.as_millis() as u64)
    } else {
        sent_ms
    };
    position_ms.abs_diff(expected) > if playing { DRIFT_MS } else { 0 }
}

pub struct SubscriptionRoot;

#[Subscription]
impl SubscriptionRoot {
    /// Playback updates, pushed when something changes.
    ///
    /// `positionMs` is an anchor rather than a reading. A playhead advancing at
    /// one second per second is the one thing a client can work out for itself,
    /// so this pushes when that stops being true — a seek, a pause, a track
    /// boundary, a stall — and not on a clock. `moving` says whether it is
    /// advancing at all. Derive the current position from
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
            let mut last_moving: Option<bool> = None;
            let mut sent_at = tokio::time::Instant::now();

            loop {
                let playback_state = state.playback_state() as u8;
                let position_ms = state.position_ms();
                let queue_item_id = state.track_info().map(|ti| ti.id.0.to_string());
                let moving = Some(state.playhead_moving());
                // A client counts on from the last message only while it said the playhead moved.
                let counting = last_moving == Some(true);

                let changed = playback_state != last_state
                    || off_course(last_position, counting, sent_at.elapsed(), position_ms)
                    || queue_item_id != last_queue_item
                    || moving != last_moving;

                if changed {
                    last_state = playback_state;
                    last_moving = moving;
                    last_position = position_ms;
                    last_queue_item = queue_item_id;
                    sent_at = tokio::time::Instant::now();

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
        // Download progress rings the store's own signal, not the engine's,
        // so only a stream that carries it wakes for it.
        let mut moved = state.downloads().moved().subscribe();

        async_stream::stream! {
            let (mut last_version, mut last_figures) = (u64::MAX, u64::MAX); // force first emit
            let mut sent_at = tokio::time::Instant::now() - PROGRESS_EVERY;

            loop {
                let version = state.playlist_version();
                let figures = state.downloads().figures();
                // An edit goes at once; progress alone at most every
                // `PROGRESS_EVERY`, since each sends the whole queue.
                let edited = version != last_version;
                let progressed = figures != last_figures;
                let held = !edited && progressed && sent_at.elapsed() < PROGRESS_EVERY;

                if (edited || progressed) && !held {
                    (last_version, last_figures) = (version, figures);
                    sent_at = tokio::time::Instant::now();
                    yield GqlQueueSnapshot::capture(&state);
                }

                wake.borrow_and_update();
                moved.borrow_and_update();
                let ended = if held {
                    tokio::select! {
                        r = wake.changed() => r.is_err(),
                        _ = tokio::time::sleep_until(sent_at + PROGRESS_EVERY) => false,
                    }
                } else {
                    tokio::select! {
                        r = wake.changed() => r.is_err(),
                        r = moved.changed() => r.is_err(),
                    }
                };
                if ended {
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

    /// Background jobs, the whole list as `jobs` returns it, again whenever
    /// one starts, moves or ends: the scans people start, the ones the server
    /// runs by itself (`watch`), and remote syncs. A running scan moves every
    /// fifty files read. Admins only.
    async fn jobs(
        &self,
        ctx: &Context<'_>,
    ) -> async_graphql::Result<impl Stream<Item = Vec<GqlJob>>> {
        super::require_role(ctx, koan_core::auth::Role::Admin)?;
        let registry = ctx.data::<JobRegistry>()?.clone();
        let mut changed = registry.subscribe();
        Ok(async_stream::stream! {
            loop {
                // Marked seen before the read, so a move between the two is
                // not slept through.
                changed.borrow_and_update();
                yield registry.list().into_iter().map(GqlJob::from).collect();
                if changed.changed().await.is_err() {
                    return;
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use futures_util::StreamExt;

    use crate::auth::AuthUser;
    use crate::graphql::jobs::{JobRegistry, WATCH, record_watch};
    use koan_core::helpers::WatchScan;

    /// An admin's `jobs` stream sends the list, then the list again as a
    /// scan moves; anyone else is refused.
    #[tokio::test]
    async fn admins_follow_jobs_as_they_move() {
        koan_core::config::isolate_config_for_tests();
        let (state, _timeline, _viz, cmd_tx) = koan_core::player::Player::spawn();
        let jobs = JobRegistry::default();
        let schema = crate::graphql::build_schema_sharing(
            state,
            cmd_tx,
            std::sync::Arc::new(koan_core::db::pool::Pool::new(
                "/nonexistent/koan-test.db".into(),
            )),
            None,
            jobs.clone(),
        );
        let query = "subscription { jobs { kind reason state done total } }";

        let user = |role| AuthUser {
            user_id: 2,
            username: "someone".into(),
            role,
        };
        let refused = schema
            .execute_stream(
                async_graphql::Request::new(query).data(user(koan_core::auth::Role::User)),
            )
            .next()
            .await
            .unwrap();
        assert!(!refused.errors.is_empty(), "{refused:?}");

        let mut stream = schema.execute_stream(
            async_graphql::Request::new(query).data(user(koan_core::auth::Role::Admin)),
        );
        let first = stream.next().await.unwrap();
        assert!(first.errors.is_empty(), "{first:?}");
        assert_eq!(first.data.to_string(), "{jobs: []}");

        let record = record_watch(&jobs);
        record(&WatchScan::Started("start-up"));
        let started = stream.next().await.unwrap().data.to_string();
        assert!(started.contains(&format!("kind: \"{WATCH}\"")), "{started}");
        assert!(started.contains("reason: \"start-up\""), "{started}");

        record(&WatchScan::Read {
            done: 50,
            total: 1200,
        });
        let moved = stream.next().await.unwrap().data.to_string();
        assert!(moved.contains("done: 50"), "{moved}");
        assert!(moved.contains("total: 1200"), "{moved}");
    }

    /// A playhead moving on time sends nothing; a seek or a stall does, and
    /// any move at all while paused.
    #[test]
    fn only_a_playhead_off_course_is_news() {
        use super::off_course;
        let s = std::time::Duration::from_secs;
        assert!(!off_course(10_000, true, s(5), 15_000));
        assert!(!off_course(10_000, true, s(5), 15_400));
        assert!(off_course(10_000, true, s(5), 10_000), "stalled");
        assert!(off_course(10_000, true, s(5), 60_000), "sought");
        assert!(!off_course(10_000, false, s(5), 10_000));
        assert!(off_course(10_000, false, s(5), 12_000));
    }

    /// The arguments stay in the schema, unused, so a query that names one
    /// still parses.
    #[test]
    fn the_interval_arguments_are_still_in_the_schema() {
        // The player's history writer opens the library in the config dir.
        koan_core::config::isolate_config_for_tests();
        let (state, _timeline, _viz, cmd_tx) = koan_core::player::Player::spawn();
        let schema = crate::graphql::build_schema(
            state,
            cmd_tx,
            std::sync::Arc::new(koan_core::db::pool::Pool::new(
                "/nonexistent/koan-test.db".into(),
            )),
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
