//! The engine's state as the views see it.
//!
//! The engine publishes whole slices and this holds the latest of each. GPUI
//! invalidates per entity, so the slices are cut into entities by rate of
//! change, as `EngineMirror.swift` cuts them into observed properties: the
//! playhead and the queue's row patches move often and live apart, so a view
//! that draws neither never hears from them. Nothing here decides what a
//! change means; a slice arrives only when it differs, and Rust decided that.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use gpui_kit::{App, AppContext, Entity, Global};
use koan_ffi::{KoanEngine, NowPlaying, OutputsInfo, QueueItem, StateSlice};

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
    pub base: Vec<QueueItem>,
    /// `base` with the latest patch applied: what a row reads now.
    pub items: Vec<QueueItem>,
    pub version: u64,
    /// Moves each time the queue is sent whole. A view lays its rows out
    /// again only when this moves; a patch redraws the rows on screen.
    pub whole: u64,
    /// Where each queue item is in `items`.
    pub at: HashMap<String, usize>,
    /// The rows the latest patch changed, put back from `base` before the
    /// next is applied, since each patch is relative to the base.
    patched: Vec<usize>,
}

impl Queue {
    fn replace(&mut self, items: Vec<QueueItem>, version: u64) {
        self.at = items
            .iter()
            .enumerate()
            .rev()
            .map(|(row, item)| (item.queue_item_id.clone(), row))
            .collect();
        self.items = items.clone();
        self.base = items;
        self.version = version;
        self.whole = self.whole.wrapping_add(1);
        self.patched.clear();
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

/// Moves whenever the library's rows change. Pages that read the library ask
/// again when it does.
pub struct Library(pub u64);

/// What the device in view plays through.
pub struct Outputs(pub Option<OutputsInfo>);

#[derive(Clone)]
pub struct Mirror {
    pub playback: Entity<Playback>,
    pub playhead: Entity<Playhead>,
    pub queue: Entity<Queue>,
    pub library: Entity<Library>,
    pub outputs: Entity<Outputs>,
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
            library: cx.new(|_| Library(0)),
            outputs: cx.new(|_| Outputs(None)),
        };
        let stream = engine.observe();
        let held = mirror.clone();
        cx.spawn(async move |cx| {
            while let Some(batch) = stream.next().await {
                cx.update(|cx| held.apply(batch, cx));
            }
        })
        .detach();
        mirror
    }

    fn apply(&self, batch: Vec<StateSlice>, cx: &mut App) {
        for slice in batch {
            match slice {
                StateSlice::Playback { now_playing } => {
                    self.playback.update(cx, |held, cx| {
                        held.0 = now_playing;
                        cx.notify();
                    });
                }
                StateSlice::Playhead {
                    position_ms,
                    seekable_ms,
                    playing,
                } => {
                    self.playhead.update(cx, |held, cx| {
                        *held = Playhead {
                            position_ms,
                            seekable_ms,
                            playing,
                            at: Instant::now(),
                        };
                        cx.notify();
                    });
                }
                StateSlice::Queue { items, version } => {
                    self.queue.update(cx, |queue, cx| {
                        queue.replace(items, version);
                        cx.notify();
                    });
                }
                StateSlice::QueuePatch { base, items } => {
                    self.queue.update(cx, |queue, cx| {
                        if queue.patch(base, items) {
                            cx.notify();
                        }
                    });
                }
                StateSlice::Library { version } => {
                    self.library.update(cx, |held, cx| {
                        held.0 = version;
                        cx.notify();
                    });
                }
                StateSlice::Outputs { outputs } => {
                    self.outputs.update(cx, |held, cx| {
                        held.0 = outputs;
                        cx.notify();
                    });
                }
                // Drawn by pages this app does not have yet.
                StateSlice::Lock { .. }
                | StateSlice::Transfers { .. }
                | StateSlice::Figures { .. }
                | StateSlice::History { .. }
                | StateSlice::Cache { .. }
                | StateSlice::Tasks { .. }
                | StateSlice::Sync { .. }
                | StateSlice::Devices { .. }
                | StateSlice::Connection { .. }
                | StateSlice::Renderers { .. } => {}
            }
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
