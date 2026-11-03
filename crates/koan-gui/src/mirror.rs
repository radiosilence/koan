//! The engine's state as the views see it.
//!
//! The engine publishes whole slices and this holds the latest of each. GPUI
//! invalidates per entity, so the slices are cut into entities by rate of
//! change, as `EngineMirror.swift` cuts them into observed properties: the
//! playhead moves often and lives apart, so a view that does not draw it
//! never hears from it. A queue patch notifies the queue's observers, which
//! redraw only the rows on screen. Nothing here decides what a change means;
//! a slice arrives only when it differs, and the engine decides that.
//!
//! The main thread only swaps finished values in. Reading the stream (which
//! copies each slice out), indexing a queue sent whole and laying out its
//! rows happen on the background executor, and so does dropping the queue a
//! new one replaces.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use koan_ffi::{
    ConnectionInfo, CoverPreload, DeviceInfo, DeviceReach, KoanEngine, NowPlaying, OutputsInfo,
    QueueItem, QueueLock, RendererInfo, RendererOutput, StateSlice, SyncProgress, Transfer,
    TransferFigure,
};
use koan_kit::app::{App, AppContext, Entity, Global};

use crate::queue::rows::{self, Row};

/// Everything a transport bar shows other than the position.
pub struct Playback(pub NowPlaying);

/// Where the playhead was when the engine last said, and whether it has been
/// moving since.
///
/// An anchor rather than a reading: the engine publishes one when a client's
/// own reckoning would go wrong (a seek, a pause, a track boundary, a stall)
/// and not otherwise, so a playhead at rest costs nothing and one advancing is
/// worked out here.
#[derive(Clone, Copy)]
pub struct Playhead {
    pub position_ms: u64,
    pub seekable_ms: u64,
    pub playing: bool,
    pub at: Instant,
}

impl Playhead {
    /// Where the playhead is at `now`, capped at the track's length: the
    /// reckoning runs past the end for as long as the engine takes to say the
    /// next track has started.
    pub fn at(&self, now: Instant, duration_ms: u64) -> u64 {
        let position = if self.playing {
            self.position_ms + now.saturating_duration_since(self.at).as_millis() as u64
        } else {
            self.position_ms
        };
        if duration_ms > 0 {
            position.min(duration_ms)
        } else {
            position
        }
    }
}

/// The queue, as last sent whole and with the latest patch applied.
#[derive(Default)]
pub struct Queue {
    /// The rows as last sent whole, each with its status as it was then.
    /// Shared, so a regroup hands it to the background executor without
    /// copying fifty thousand rows on the main thread.
    pub base: Arc<Vec<QueueItem>>,
    /// `base` with the latest patch applied: what a row reads now.
    pub items: Vec<QueueItem>,
    pub version: u64,
    /// Moves each time the queue is sent whole. A view lays its rows out
    /// again only when this moves; a patch redraws the rows on screen.
    pub whole: u64,
    /// Where each queue item is in `items`.
    pub at: HashMap<String, usize>,
    /// The queue laid out as rows: record headings and tracks.
    pub rows: Vec<Row>,
    /// The track row of each queue item.
    pub track_rows: HashMap<String, usize>,
    /// The records whose rows number by disc and track.
    pub disc_records: std::collections::HashSet<i64>,
    /// Whether `rows` are laid out under record headings.
    pub grouped: bool,
    /// The rows the latest patch changed, put back from `base` before the
    /// next is applied, since each patch is relative to the base.
    patched: Vec<usize>,
}

impl Queue {
    /// Everything about a queue sent whole that takes time, done where it
    /// does not hold up a frame.
    fn prepare(items: Vec<QueueItem>, version: u64) -> Self {
        let at = items
            .iter()
            .enumerate()
            .rev()
            .map(|(row, item)| (item.queue_item_id.clone(), row))
            .collect();
        let grouped = crate::window_state::GROUPED.load(std::sync::atomic::Ordering::Relaxed);
        let rows = rows::build(&items, grouped);
        let disc_records = rows::disc_records(&items);
        let track_rows = track_rows(&rows, &items);
        Self {
            items: items.clone(),
            base: Arc::new(items),
            version,
            whole: 0,
            at,
            rows,
            track_rows,
            disc_records,
            grouped,
            patched: Vec::new(),
        }
    }

    /// Takes rows laid out off the main thread for this queue, as the queue
    /// sent whole would be: a view sees `whole` move and resets its list.
    fn take_rows(&mut self, grouped: bool, rows: Vec<Row>, track_rows: HashMap<String, usize>) {
        self.grouped = grouped;
        self.rows = rows;
        self.track_rows = track_rows;
        self.whole = self.whole.wrapping_add(1);
    }

    /// Takes the prepared queue's place, returning the one it replaces for
    /// dropping off the main thread.
    fn replace(&mut self, mut next: Queue) -> Queue {
        next.whole = self.whole.wrapping_add(1);
        std::mem::replace(self, next)
    }

    /// Whether the patch changed anything. A patch to a queue sent whole since
    /// is stale, and an empty one on an unpatched queue says nothing.
    fn patch(&mut self, base: u64, items: Vec<QueueItem>) -> bool {
        if base != self.version || (items.is_empty() && self.patched.is_empty()) {
            return false;
        }
        for row in self.patched.drain(..) {
            self.items[row] = self.base[row].clone();
        }
        for item in items {
            if let Some(&row) = self.at.get(&item.queue_item_id) {
                self.items[row] = item;
                self.patched.push(row);
            }
        }
        true
    }
}

/// Lays `queue` out again, grouped or flat, on the background executor: a
/// queue of 50,000 rows is no frame's work. The rows land only if the queue
/// was not sent whole meanwhile, which lays itself out, and if `grouped` is
/// still what is wanted.
pub fn regroup(queue: &Entity<Queue>, grouped: bool, cx: &mut App) {
    let (version, base) = {
        let q = queue.read(cx);
        (q.version, q.base.clone())
    };
    let queue = queue.downgrade();
    let laid = cx.background_spawn(async move {
        let rows = rows::build(&base, grouped);
        let track_rows = track_rows(&rows, &base);
        (rows, track_rows)
    });
    cx.spawn(async move |cx| {
        let (rows, track_rows) = laid.await;
        queue
            .update(cx, |q, cx| {
                let wanted =
                    crate::window_state::GROUPED.load(std::sync::atomic::Ordering::Relaxed);
                if q.version == version && wanted == grouped {
                    q.take_rows(grouped, rows, track_rows);
                    cx.notify();
                }
            })
            .ok();
    })
    .detach();
}

/// The track row of each queue item.
fn track_rows(rows: &[Row], items: &[QueueItem]) -> HashMap<String, usize> {
    rows.iter()
        .enumerate()
        .filter_map(|(row, r)| match r {
            Row::Track { ix, .. } => Some((items[*ix].queue_item_id.clone(), row)),
            Row::Heading { .. } => None,
        })
        .collect()
}

/// Moves whenever the library's rows change. Pages that read the library ask
/// again when it does. `known` once the engine's first report has landed, so
/// a page does not read the library twice at launch.
pub struct Library {
    pub version: u64,
    pub known: bool,
}

/// What the device in view plays through.
pub struct Outputs(pub Option<OutputsInfo>);

/// A slice held as the engine sent it, for those nothing here reshapes.
pub struct Slice<T>(pub T);

/// What koan is doing on its own: one entity, since the one reader draws a
/// row for each and would be woken by either.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Tasks {
    pub scanning: bool,
    pub syncing: bool,
}

/// The other devices, and which one this app controls (`None` for itself).
#[derive(Default)]
pub struct Devices {
    pub devices: Vec<DeviceInfo>,
    pub target: Option<String>,
    pub target_reach: Option<DeviceReach>,
}

/// UPnP renderers on the network, and the one this koan plays to.
#[derive(Default)]
pub struct Renderers {
    pub renderers: Vec<RendererInfo>,
    pub output: Option<RendererOutput>,
    /// macOS refused the last search the local network.
    pub blocked: bool,
}

#[derive(Clone)]
pub struct Mirror {
    pub playback: Entity<Playback>,
    pub playhead: Entity<Playhead>,
    pub queue: Entity<Queue>,
    pub library: Entity<Library>,
    pub outputs: Entity<Outputs>,
    pub lock: Entity<Slice<Option<QueueLock>>>,
    pub transfers: Entity<Slice<Vec<Transfer>>>,
    /// The transfers' byte counts: the fast half, apart from `transfers`.
    pub figures: Entity<Slice<Vec<TransferFigure>>>,
    pub history: Entity<Slice<u64>>,
    pub cache: Entity<Slice<u64>>,
    pub tasks: Entity<Slice<Tasks>>,
    pub sync: Entity<Slice<Option<SyncProgress>>>,
    pub covers: Entity<Slice<Option<CoverPreload>>>,
    pub devices: Entity<Slice<Devices>>,
    pub connection: Entity<Slice<Option<ConnectionInfo>>>,
    pub renderers: Entity<Slice<Renderers>>,
}

impl Global for Mirror {}

impl Mirror {
    /// Starts following the engine. The loop ends when the engine goes, since
    /// the stream holds it weakly and answers `None` then.
    pub fn follow(engine: &Arc<KoanEngine>, cx: &mut App) -> Self {
        let mirror = Self {
            playback: cx.new(|_| Playback(idle())),
            playhead: cx.new(|_| Playhead {
                position_ms: 0,
                seekable_ms: 0,
                playing: false,
                at: Instant::now(),
            }),
            queue: cx.new(|_| Queue::default()),
            library: cx.new(|_| Library {
                version: 0,
                known: false,
            }),
            outputs: cx.new(|_| Outputs(None)),
            lock: cx.new(|_| Slice(None)),
            transfers: cx.new(|_| Slice(Vec::new())),
            figures: cx.new(|_| Slice(Vec::new())),
            history: cx.new(|_| Slice(0)),
            cache: cx.new(|_| Slice(0)),
            tasks: cx.new(|_| Slice(Tasks::default())),
            sync: cx.new(|_| Slice(None)),
            covers: cx.new(|_| Slice(None)),
            devices: cx.new(|_| Slice(Devices::default())),
            connection: cx.new(|_| Slice(None)),
            renderers: cx.new(|_| Slice(Renderers::default())),
        };
        let stream = engine.observe();
        let held = mirror.clone();
        let background = cx.background_executor().clone();
        cx.spawn(async move |cx| {
            loop {
                let stream = stream.clone();
                let batch = background
                    .spawn(async move {
                        Some(
                            stream
                                .next()
                                .await?
                                .into_iter()
                                .map(Prepared::from)
                                .collect(),
                        )
                    })
                    .await;
                let Some(batch) = batch else { break };
                let replaced = cx.update(|cx| held.apply(batch, cx));
                if !replaced.is_empty() {
                    background.spawn(async move { drop(replaced) }).detach();
                }
            }
        })
        .detach();
        mirror
    }

    /// Returns the queues replaced, for dropping elsewhere.
    fn apply(&self, batch: Vec<Prepared>, cx: &mut App) -> Vec<Queue> {
        let mut replaced = Vec::new();
        for slice in batch {
            let slice = match slice {
                Prepared::Queue(next) => {
                    // Laid out under the grouping read when it was prepared:
                    // one toggled since, or read from the saved layout since,
                    // lays it out again, off this thread as well.
                    let stale = next.grouped
                        != crate::window_state::GROUPED.load(std::sync::atomic::Ordering::Relaxed);
                    self.queue.update(cx, |queue, cx| {
                        replaced.push(queue.replace(next));
                        cx.notify();
                    });
                    if stale {
                        let grouped =
                            crate::window_state::GROUPED.load(std::sync::atomic::Ordering::Relaxed);
                        regroup(&self.queue, grouped, cx);
                    }
                    continue;
                }
                Prepared::Playhead(next) => {
                    self.playhead.update(cx, |held, cx| {
                        *held = next;
                        cx.notify();
                    });
                    continue;
                }
                Prepared::Other(slice) => slice,
            };
            match slice {
                StateSlice::Playback { now_playing } => {
                    self.playback.update(cx, |held, cx| {
                        held.0 = now_playing;
                        cx.notify();
                    });
                }
                // Prepared above.
                StateSlice::Playhead { .. } | StateSlice::Queue { .. } => {}
                StateSlice::QueuePatch { base, items } => {
                    self.queue.update(cx, |queue, cx| {
                        if queue.patch(base, items) {
                            cx.notify();
                        }
                    });
                }
                StateSlice::Library { version } => {
                    self.library.update(cx, |held, cx| {
                        held.version = version;
                        held.known = true;
                        cx.notify();
                    });
                }
                StateSlice::Outputs { outputs } => {
                    self.outputs.update(cx, |held, cx| {
                        held.0 = outputs;
                        cx.notify();
                    });
                }
                StateSlice::Lock { lock } => set(&self.lock, lock, cx),
                StateSlice::Transfers { transfers } => set(&self.transfers, transfers, cx),
                StateSlice::Figures { figures } => set(&self.figures, figures, cx),
                StateSlice::History { version } => set(&self.history, version, cx),
                StateSlice::Cache { bytes } => set(&self.cache, bytes, cx),
                StateSlice::Tasks { scanning, syncing } => {
                    set(&self.tasks, Tasks { scanning, syncing }, cx)
                }
                StateSlice::Sync { progress } => set(&self.sync, progress, cx),
                StateSlice::Covers { progress } => set(&self.covers, progress, cx),
                StateSlice::Devices {
                    devices,
                    target,
                    target_reach,
                } => set(
                    &self.devices,
                    Devices {
                        devices,
                        target,
                        target_reach,
                    },
                    cx,
                ),
                StateSlice::Connection { connection } => {
                    set(&self.connection, Some(connection), cx)
                }
                StateSlice::Renderers {
                    renderers,
                    output,
                    local_network_blocked,
                } => set(
                    &self.renderers,
                    Renderers {
                        renderers,
                        output,
                        blocked: local_network_blocked,
                    },
                    cx,
                ),
            }
        }
        replaced
    }
}

/// Holds a slice that arrived and wakes that entity's readers alone.
fn set<T: 'static>(entity: &Entity<Slice<T>>, value: T, cx: &mut App) {
    entity.update(cx, |held, cx| {
        held.0 = value;
        cx.notify();
    });
}

/// A slice as the main thread applies it. Made once per slice that changed,
/// so the size of the largest variant costs nothing worth boxing for.
#[allow(clippy::large_enum_variant)]
enum Prepared {
    Queue(Queue),
    /// Stamped as it is read from the stream, which is when the engine said.
    Playhead(Playhead),
    Other(StateSlice),
}

impl From<StateSlice> for Prepared {
    fn from(slice: StateSlice) -> Self {
        match slice {
            StateSlice::Queue { items, version } => Prepared::Queue(Queue::prepare(items, version)),
            StateSlice::Playhead {
                position_ms,
                seekable_ms,
                playing,
            } => Prepared::Playhead(Playhead {
                position_ms,
                seekable_ms,
                playing,
                at: Instant::now(),
            }),
            other => Prepared::Other(other),
        }
    }
}

fn idle() -> NowPlaying {
    NowPlaying {
        state: koan_ffi::PlayState::Stopped,
        waiting: false,
        position_ms: 0,
        duration_ms: 0,
        queue_item_id: None,
        entry: None,
        format: None,
        playlist_version: 0,
        shuffle: false,
        repeat_mode: koan_ffi::RepeatMode::Off,
        undo_shuffles: 0,
        queue_shufflable: false,
        sleep: None,
        sleep_fading: false,
        start_failure: None,
    }
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use koan_ffi::{EntryStatus, QueueItem, RunRole};
    use koan_kit::app::test::TestAppContext;

    use super::*;

    /// A queue of `n` rows, ten to a record, with words the length a library's
    /// are, so a clone costs what it would.
    fn items(n: usize) -> Vec<QueueItem> {
        (0..n)
            .map(|i| QueueItem {
                queue_item_id: format!("0190a3f2-7c4e-7b9a-8d1e-{i:012}"),
                track_id: Some(i as i64),
                album_id: Some((i / 10) as i64),
                artist_id: Some(1),
                album_artist_id: Some(1),
                title: format!("A track title of some length {i}"),
                artist: "An Artist Name".into(),
                album_artist: "An Artist Name".into(),
                artist_credit: Vec::new(),
                album_artist_credit: Vec::new(),
                credit_differs: false,
                run: if i % 10 == 0 {
                    RunRole::Opens
                } else {
                    RunRole::Continues
                },
                album: format!("A record title {}", i / 10),
                year: Some(1999),
                codec: Some("FLAC".into()),
                track_number: Some((i % 10) as u32 + 1),
                disc: Some(1),
                duration_ms: Some(240_000),
                status: EntryStatus::Queued,
                playlist_entry_id: None,
                failure_reason: None,
                on_server: false,
                on_disk: true,
            })
            .collect()
    }

    /// What a regroup of 50,000 rows costs the main thread: the hand-off,
    /// not the layout, which is the background executor's.
    #[koan_kit::app::test::test]
    fn a_regroup_hands_the_queue_off_at_no_cost(cx: &mut TestAppContext) {
        let queue = cx.update(|cx| cx.new(|_| Queue::prepare(items(50_000), 1)));
        let mut spent = Vec::new();
        for i in 0..10 {
            let took = cx.update(|cx| {
                let start = std::time::Instant::now();
                regroup(&queue, i % 2 == 0, cx);
                start.elapsed()
            });
            spent.push(took);
            cx.run_until_parked();
        }
        spent.sort();
        println!(
            "regroup of 50,000 rows, main thread: median {:?}, most {:?}",
            spent[spent.len() / 2],
            spent[spent.len() - 1]
        );
    }
}
