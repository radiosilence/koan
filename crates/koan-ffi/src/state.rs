//! Engine state as slices, and one cursor per client.
//!
//! The engine publishes whole slices, never deltas. A slice says everything
//! about its corner of the state, so applying one cannot go wrong: there is no
//! merge to get right and no earlier message it depends on. A delta can be
//! applied wrongly, and was — three times in one afternoon, each a Swift-side
//! copy patched by a rule someone had to remember to write.
//!
//! Slices are cut by **rate of change, not by subject**. Playback position and
//! transfer figures move ten times a second; the queue and the set of transfers
//! move when someone does something. A client subscribes per slice, so putting
//! a fast thing next to a slow one makes every reader of the slow one wake at
//! the fast one's rate — which is how a byte counter came to rebuild the queue.
//!
//! Nothing is dropped. Each slice carries a sequence number and each client
//! keeps its own cursor, so falling behind costs you the intermediate values of
//! a slice and never the fact that it changed. A cursor that has seen nothing
//! reads the whole state, which is how a client seeds itself.

use std::sync::{Arc, Weak};
use std::time::Instant;

use crate::types::{NowPlaying, QueueItem, QueueLock, Transfer, TransferFigure};

/// One slice of engine state, whole.
// Boxing is what clippy wants for the size spread and is not on offer across
// the FFI. The saving would be nothing anyway: a handful of these exist at a
// time, built a few times a second, never held in a collection.
#[allow(clippy::large_enum_variant)]
#[derive(uniffi::Enum, Debug, Clone, PartialEq)]
pub enum StateSlice {
    /// Everything a transport bar shows other than the position. `position_ms`
    /// is always zero here — it has a slice of its own, and leaving it in would
    /// make every tick a change to this one.
    Playback { now_playing: NowPlaying },
    /// Where the playhead is, how far it can be dragged, and whether it is
    /// moving on its own.
    ///
    /// An anchor, not a reading. A client that knows the playhead was at
    /// `position_ms` when this arrived, and that it is playing, knows where it
    /// is now without being told again — so this is published when that stops
    /// being true (a seek, a pause, a new track, a stall) rather than on a
    /// clock. It is the one number in the engine that changes without anything
    /// happening, and a stream that had to keep saying so could never be quiet.
    ///
    /// One slice because they move together and are read by one thing. The
    /// seekable extent belongs here rather than beside the track's title: it
    /// grows for as long as a download is arriving, and a field that moves at
    /// that rate drags every reader of whatever it sits next to along with it.
    Playhead {
        position_ms: u64,
        seekable_ms: u64,
        playing: bool,
    },
    /// The queue, in order. The whole list: a queue that is one copy refetched
    /// whole has nothing to forget to patch.
    Queue { items: Vec<QueueItem> },
    /// What the queue still is, when it is still a playlist or a record.
    Lock { lock: Option<QueueLock> },
    /// Every transfer koan knows about — running first, then whatever settled
    /// most recently. Structural only; see `Figures`.
    Transfers { transfers: Vec<Transfer> },
    /// The byte counts behind those transfers. The fast half.
    Figures { figures: Vec<TransferFigure> },
    /// The library's rows changed — a scan, a sync, an import, an organize, a
    /// download landing, a folder being forgotten.
    ///
    /// A version, not a mirror. A record's tracks and a search's results are
    /// asked for on demand; sending every album's tracks across the boundary
    /// so a client can hold a copy is the opposite of the point. What a client
    /// needs is to know to ask again.
    Library { version: u64 },
}

/// Which slot a slice occupies. One per variant, in apply order.
#[derive(Clone, Copy)]
enum Slot {
    Playback,
    Playhead,
    Queue,
    Lock,
    Transfers,
    Figures,
    Library,
}

const SLOTS: usize = 7;

impl StateSlice {
    fn slot(&self) -> Slot {
        match self {
            Self::Playback { .. } => Slot::Playback,
            Self::Playhead { .. } => Slot::Playhead,
            Self::Queue { .. } => Slot::Queue,
            Self::Lock { .. } => Slot::Lock,
            Self::Transfers { .. } => Slot::Transfers,
            Self::Figures { .. } => Slot::Figures,
            Self::Library { .. } => Slot::Library,
        }
    }
}

/// The latest of every slice, and when each last moved.
struct Slots {
    latest: [Option<StateSlice>; SLOTS],
    seq: [u64; SLOTS],
    clock: u64,
}

impl Default for Slots {
    fn default() -> Self {
        Self {
            latest: [const { None }; SLOTS],
            seq: [0; SLOTS],
            clock: 0,
        }
    }
}

/// What the engine publishes and clients read.
pub struct EngineState {
    slots: parking_lot::Mutex<Slots>,
    /// Woken on every publish. Carries the clock so a receiver that was busy
    /// still sees it moved.
    tick: tokio::sync::watch::Sender<u64>,
}

impl EngineState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            slots: parking_lot::Mutex::new(Slots::default()),
            tick: tokio::sync::watch::channel(0).0,
        })
    }

    /// Replace a slice. A no-op when it says the same thing as the last one,
    /// which is what keeps a slice nobody touched off the wire entirely.
    ///
    /// Publishing more often than clients read is free: only the latest value
    /// of each slice is kept, so a burst inside one tick collapses to one
    /// message.
    pub fn publish(&self, slice: StateSlice) {
        let mut slots = self.slots.lock();
        let i = slice.slot() as usize;
        if slots.latest[i].as_ref() == Some(&slice) {
            return;
        }
        slots.clock += 1;
        let clock = slots.clock;
        slots.seq[i] = clock;
        slots.latest[i] = Some(slice);
        drop(slots);
        let _ = self.tick.send(clock);
    }

    /// Everything that moved past `seen`, and advance it.
    fn since(&self, seen: &mut [u64; SLOTS]) -> Vec<StateSlice> {
        let slots = self.slots.lock();
        let mut batch = Vec::new();
        for (i, seen) in seen.iter_mut().enumerate() {
            if slots.seq[i] > *seen {
                *seen = slots.seq[i];
                if let Some(slice) = &slots.latest[i] {
                    batch.push(slice.clone());
                }
            }
        }
        batch
    }
}

/// One client's place in the state stream.
///
/// A cursor, not a subscription queue. A broadcast channel loses whatever was
/// published while the client was away — which is a real bug and was one: a
/// receiver taken per message dropped everything that arrived between two
/// calls, and it went unnoticed for as long as position was the only traffic.
/// A cursor cannot lose a change. It can only arrive later, holding a newer
/// value, which is what every slice being a whole snapshot makes safe.
#[derive(uniffi::Object)]
pub struct StateStream {
    /// Weak, so a client's loop ends when the engine goes rather than holding
    /// it up. The loop *is* the subscription — there is nothing to unregister.
    state: Weak<EngineState>,
    inner: tokio::sync::Mutex<Cursor>,
}

struct Cursor {
    seen: [u64; SLOTS],
    tick: tokio::sync::watch::Receiver<u64>,
}

impl StateStream {
    pub fn new(state: &Arc<EngineState>) -> Arc<Self> {
        Arc::new(Self {
            state: Arc::downgrade(state),
            inner: tokio::sync::Mutex::new(Cursor {
                // Nothing seen, so the first read delivers the whole state and
                // a client needs no separate call to seed itself.
                seen: [0; SLOTS],
                tick: state.tick.subscribe(),
            }),
        })
    }
}

#[uniffi::export]
impl StateStream {
    /// Everything that has changed since this stream last answered, at most one
    /// message per slice.
    ///
    /// Waits when nothing has. `None` once the engine is gone, which ends the
    /// caller's loop.
    pub async fn next(&self) -> Option<Vec<StateSlice>> {
        let mut cursor = self.inner.lock().await;
        loop {
            let state = self.state.upgrade()?;
            // Marked seen *before* reading, so a publish that lands between the
            // read and the wait still counts as a change and `changed()`
            // returns at once rather than sleeping through it.
            cursor.tick.borrow_and_update();
            let batch = state.since(&mut cursor.seen);
            if !batch.is_empty() {
                return Some(batch);
            }
            drop(state);
            cursor.tick.changed().await.ok()?;
        }
    }
}

/// The playhead as a client last heard it.
///
/// Kept by whoever publishes so it can answer the only question that matters
/// for a value that changes on its own: would the client still be right? A
/// playhead advancing at one second per second is something the far side can
/// work out; a seek, a pause, a track boundary and a stall are not.
#[derive(Clone, Copy, Debug)]
pub struct Anchor {
    pub position_ms: u64,
    pub playing: bool,
    pub at: Instant,
}

impl Anchor {
    /// Where a client holding this anchor believes the playhead is now.
    pub fn reckoning(&self, now: Instant) -> u64 {
        if self.playing {
            self.position_ms + now.duration_since(self.at).as_millis() as u64
        } else {
            self.position_ms
        }
    }

    /// Whether that belief has come apart, by more than `tolerance`.
    pub fn stale(&self, position_ms: u64, playing: bool, now: Instant, tolerance: u64) -> bool {
        playing != self.playing || self.reckoning(now).abs_diff(position_ms) > tolerance
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn anchor(position_ms: u64, playing: bool) -> Anchor {
        Anchor {
            position_ms,
            playing,
            at: Instant::now(),
        }
    }

    #[test]
    fn a_playhead_running_on_is_not_worth_saying() {
        let held = anchor(10_000, true);
        let now = held.at + Duration::from_millis(500);
        // Exactly where a client would have put it by itself.
        assert!(!held.stale(10_500, true, now, 32));
    }

    #[test]
    fn a_seek_is() {
        let held = anchor(10_000, true);
        let now = held.at + Duration::from_millis(500);
        assert!(held.stale(90_000, true, now, 32));
        // And so is a jump backwards, which is the same distance the other way.
        assert!(held.stale(2_000, true, now, 32));
    }

    #[test]
    fn a_pause_is_said_even_where_the_position_agrees() {
        let held = anchor(10_000, true);
        let now = held.at + Duration::from_millis(500);
        assert!(held.stale(10_500, false, now, 32));
    }

    #[test]
    fn a_paused_playhead_stays_where_it_was_left() {
        let held = anchor(10_000, false);
        let now = held.at + Duration::from_secs(30);
        assert!(!held.stale(10_000, false, now, 32));
        // A stall while playing is the same shape: time passed, the playhead
        // did not, and that is worth saying.
        assert!(anchor(10_000, true).stale(10_000, true, now, 32));
    }

    fn library(version: u64) -> StateSlice {
        StateSlice::Library { version }
    }

    fn queue(n: usize) -> StateSlice {
        StateSlice::Queue {
            items: (0..n)
                .map(|i| QueueItem {
                    queue_item_id: format!("01930000-0000-7000-8000-{i:012}"),
                    track_id: Some(i as i64),
                    album_id: Some((i / 12) as i64),
                    title: format!("Track {i}"),
                    artist: "An Artist Whose Name Is Of Ordinary Length".into(),
                    album_artist: "An Artist Whose Name Is Of Ordinary Length".into(),
                    album: "A Record With A Reasonably Long Title".into(),
                    year: Some("1994".into()),
                    codec: Some("flac".into()),
                    track_number: Some((i % 12) as i64 + 1),
                    disc: Some(1),
                    duration_ms: Some(240_000),
                    status: crate::types::EntryStatus::Queued,
                    playlist_entry_id: None,
                    failure_reason: None,
                    on_server: true,
                    on_disk: false,
                })
                .collect(),
        }
    }

    #[test]
    fn a_fresh_cursor_reads_the_whole_state() {
        let state = EngineState::new();
        state.publish(library(1));
        state.publish(StateSlice::Playhead {
            position_ms: 5,
            seekable_ms: 0,
            playing: true,
        });

        let mut seen = [0; SLOTS];
        let batch = state.since(&mut seen);
        assert_eq!(batch.len(), 2);
        assert!(state.since(&mut seen).is_empty());
    }

    #[test]
    fn only_the_latest_of_a_slice_survives_a_window() {
        let state = EngineState::new();
        let mut seen = [0; SLOTS];
        state.since(&mut seen);

        for ms in 1..=100 {
            state.publish(StateSlice::Playhead {
                position_ms: ms,
                seekable_ms: 0,
                playing: true,
            });
        }

        let batch = state.since(&mut seen);
        assert_eq!(
            batch,
            vec![StateSlice::Playhead {
                position_ms: 100,
                seekable_ms: 0,
                playing: true,
            }]
        );
    }

    #[test]
    fn a_slice_that_says_the_same_thing_is_not_a_change() {
        let state = EngineState::new();
        let mut seen = [0; SLOTS];
        state.publish(library(7));
        state.since(&mut seen);

        state.publish(library(7));
        assert!(state.since(&mut seen).is_empty());
        state.publish(library(8));
        assert_eq!(state.since(&mut seen), vec![library(8)]);
    }

    /// A slow client misses values, never the fact that a slice moved. This is
    /// what a broadcast receiver could not promise, and the bug it cost was a
    /// transport that appeared to stall whenever downloads got busy.
    #[test]
    fn falling_behind_loses_values_and_never_a_slice() {
        let state = EngineState::new();
        let mut seen = [0; SLOTS];
        state.since(&mut seen);

        // Two slices move while the client is away, one of them repeatedly.
        state.publish(StateSlice::Playhead {
            position_ms: 1,
            seekable_ms: 0,
            playing: true,
        });
        state.publish(library(1));
        state.publish(StateSlice::Playhead {
            position_ms: 2,
            seekable_ms: 0,
            playing: true,
        });
        state.publish(library(2));

        let batch = state.since(&mut seen);
        assert_eq!(batch.len(), 2);
        assert!(batch.contains(&StateSlice::Playhead {
            position_ms: 2,
            seekable_ms: 0,
            playing: true,
        }));
        assert!(batch.contains(&library(2)));
    }

    /// What a snapshot costs, since the design turns on it being cheap enough
    /// to send whole rather than as a delta.
    ///
    /// A library-sized queue, published and read the way the watcher does it:
    /// one equality check against the last one and one clone out to a client.
    /// The ceiling is loose on purpose — it is here to catch an order of
    /// magnitude, not to police a millisecond on a busy CI box. Measured at
    /// well under 10ms for 5,000 rows on an M-series laptop.
    #[test]
    fn a_queue_snapshot_is_cheap_enough_to_send_whole() {
        let state = EngineState::new();
        let mut seen = [0; SLOTS];

        let start = std::time::Instant::now();
        for version in 0..10u64 {
            state.publish(queue(5_000));
            // The version moving is what a real queue change looks like: the
            // items differ, so the equality check runs to completion.
            state.publish(library(version));
            state.since(&mut seen);
        }
        let each = start.elapsed() / 10;
        println!("5,000-row queue snapshot: {each:?} per publish + read");
        assert!(each < std::time::Duration::from_millis(100), "{each:?}");
    }
}
