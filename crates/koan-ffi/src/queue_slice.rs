//! The queue as clients are sent it: whole when its rows change, and otherwise
//! as the rows that read differently since.
//!
//! A queue is edited when someone does something, but its rows' statuses move
//! on their own: the cursor advancing, a download landing, a track failing.
//! Sending tens of thousands of rows across the boundary for each of those,
//! and having the client decode and regroup them all, is what made a large
//! queue unusable. So the rows go whole (`StateSlice::Queue`) only when the
//! queue's content changes, and between those a `StateSlice::QueuePatch` says
//! which rows now read differently from that base.
//!
//! The patch is whole too, relative to its base: it holds every row that
//! differs, not the ones that changed since the last patch. A client that
//! missed a patch loses nothing by applying the next one.

use std::collections::HashMap;

use koan_core::player::state::{QueueItemId, QueueReading, SharedPlayerState};

use crate::state::StateSlice;
use crate::types::{EntryStatus, QueueItem};

/// Past this many differing rows, the whole queue again: a client applies
/// that as cheaply as a patch the size of the queue, and a base that has
/// drifted far makes every later patch carry the drift.
pub(crate) const PATCH_MAX: usize = 512;

/// What the library says about each queued track: its album, and where its
/// bytes are (on a server, on this machine).
#[derive(Default)]
pub(crate) struct Joins {
    pub album_ids: HashMap<i64, i64>,
    pub sources: HashMap<i64, (bool, bool)>,
}

/// The queue as last sent whole, and what is needed to tell what has moved
/// since.
#[derive(Default)]
pub(crate) struct QueueSender {
    base: Option<Base>,
    /// The last base's version. Owned here rather than taken from the
    /// playlist, which does not move when the library does: two bases with one
    /// version would let a client apply the older one's patch to the newer.
    sent: u64,
}

struct Base {
    version: u64,
    content: u64,
    ids: Vec<QueueItemId>,
    items: Vec<QueueItem>,
    /// The library's reading of each row, in queue order, as of the last time
    /// it moved. Kept aligned with the rows so a pass over them hashes nothing.
    library: Vec<Library>,
}

/// What the library says about one row.
#[derive(Clone, Copy, PartialEq)]
struct Library {
    album_id: Option<i64>,
    on_server: bool,
    on_disk: bool,
}

impl Library {
    fn of(track: Option<i64>, joins: &Joins) -> Self {
        let (on_server, on_disk) = track
            .and_then(|id| joins.sources.get(&id))
            .copied()
            .unwrap_or((false, false));
        Self {
            album_id: track.and_then(|id| joins.album_ids.get(&id).copied()),
            on_server,
            on_disk,
        }
    }
}

impl QueueSender {
    /// Forget what was sent, so the next update sends the queue whole. For a
    /// change of the device in view, after which the client holds another
    /// device's queue.
    pub(crate) fn reset(&mut self) {
        self.base = None;
    }

    /// The slices that bring a client up to date with `state`'s queue.
    ///
    /// `library_moved` re-asks the library through `joins`; nothing in the
    /// player says when a download has written a track's file.
    pub(crate) fn update(
        &mut self,
        state: &SharedPlayerState,
        library_moved: bool,
        joins: impl FnOnce(&[i64]) -> Joins,
    ) -> Vec<StateSlice> {
        // Read before the rows: an edit landing in between moves it again,
        // and the next update sends the queue whole once more.
        let content = state.content_version();
        let Some(base) = self.base.as_mut().filter(|b| b.content == content) else {
            return self.rebuild(state, content, joins);
        };

        let readings = state.queue_readings();
        let same_rows = readings.len() == base.ids.len()
            && readings.iter().zip(&base.ids).all(|(r, id)| r.id == *id);
        if !same_rows {
            return self.rebuild(state, content, joins);
        }
        if library_moved {
            let track_ids: Vec<i64> = readings.iter().filter_map(|r| r.db_id).collect();
            let joins = joins(&track_ids);
            base.library = readings
                .iter()
                .map(|r| Library::of(r.db_id, &joins))
                .collect();
        }

        let mut items = Vec::new();
        for ((reading, sent), library) in readings.iter().zip(&base.items).zip(&base.library) {
            if let Some(row) = moved(sent, reading, *library) {
                items.push(row);
                if items.len() > PATCH_MAX {
                    break;
                }
            }
        }
        if items.len() <= PATCH_MAX {
            return vec![StateSlice::QueuePatch {
                base: base.version,
                items,
            }];
        }
        // What the library said is still what it says: the rebuild need not
        // ask it again.
        let pairs = || readings.iter().zip(&base.library);
        let held = Joins {
            album_ids: pairs()
                .filter_map(|(r, l)| Some((r.db_id?, l.album_id?)))
                .collect(),
            sources: pairs()
                .filter_map(|(r, l)| Some((r.db_id?, (l.on_server, l.on_disk))))
                .collect(),
        };
        self.rebuild(state, content, |_| held)
    }

    fn rebuild(
        &mut self,
        state: &SharedPlayerState,
        content: u64,
        joins: impl FnOnce(&[i64]) -> Joins,
    ) -> Vec<StateSlice> {
        self.sent += 1;
        let version = self.sent;
        let entries = state.derive_visible_queue().entries;
        let track_ids: Vec<i64> = entries.iter().filter_map(|e| e.db_id).collect();
        let joins = joins(&track_ids);
        let items: Vec<QueueItem> = entries
            .iter()
            .map(|e| QueueItem::from_entry(e, &joins.album_ids, &joins.sources))
            .collect();
        let slices = vec![
            StateSlice::Queue {
                items: items.clone(),
                version,
            },
            StateSlice::QueuePatch {
                base: version,
                items: Vec::new(),
            },
        ];
        self.base = Some(Base {
            version,
            content,
            ids: entries.iter().map(|e| e.id).collect(),
            library: entries
                .iter()
                .map(|e| Library::of(e.db_id, &joins))
                .collect(),
            items,
        });
        slices
    }
}

/// `sent` as it reads now, if that differs. Only what can move without the
/// queue's content changing is taken from the reading; the text stays as sent.
fn moved(sent: &QueueItem, reading: &QueueReading, library: Library) -> Option<QueueItem> {
    let status: EntryStatus = reading.status.into();
    let same = status == sent.status
        && reading.duration_ms == sent.duration_ms
        && reading.error == sent.failure_reason
        && library.on_server == sent.on_server
        && library.on_disk == sent.on_disk
        && library.album_id == sent.album_id;
    (!same).then(|| QueueItem {
        status,
        duration_ms: reading.duration_ms,
        failure_reason: reading.error.clone(),
        on_server: library.on_server,
        on_disk: library.on_disk,
        album_id: library.album_id,
        ..sent.clone()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::EngineState;
    use koan_core::player::state::{ItemState, PlaylistItem};
    use std::time::{Duration, Instant};

    const ROWS: usize = 50_000;

    fn item(i: usize, state: ItemState) -> PlaylistItem {
        PlaylistItem {
            id: QueueItemId::new(),
            db_id: Some(i as i64),
            playlist_entry_id: None,
            path: format!(
                "/music/An Artist/Record {}/{:02} Track {i}.flac",
                i / 12,
                i % 12
            )
            .into(),
            title: format!("Track {i}"),
            artist: "An Artist Whose Name Is Of Ordinary Length".into(),
            album_artist: "An Artist Whose Name Is Of Ordinary Length".into(),
            album: format!("A Record With A Reasonably Long Title {}", i / 12),
            year: Some("1994".into()),
            codec: Some("FLAC".into()),
            track_number: Some((i % 12) as i64 + 1),
            disc: Some(1),
            duration_ms: Some(240_000),
            state,
            pre_shuffle: None,
        }
    }

    fn queue(n: usize) -> (std::sync::Arc<SharedPlayerState>, Vec<QueueItemId>) {
        queue_of(n, ItemState::Pending)
    }

    fn queue_of(
        n: usize,
        state_of: ItemState,
    ) -> (std::sync::Arc<SharedPlayerState>, Vec<QueueItemId>) {
        let state = SharedPlayerState::new();
        let items: Vec<PlaylistItem> = (0..n).map(|i| item(i, state_of.clone())).collect();
        let ids = items.iter().map(|i| i.id).collect();
        state.add_items(items);
        (state, ids)
    }

    /// Half the queue's tracks on disk, as a library with a cache would say.
    fn joins(ids: &[i64]) -> Joins {
        Joins {
            album_ids: ids.iter().map(|&id| (id, id / 12)).collect(),
            sources: ids.iter().map(|&id| (id, (true, id % 2 == 0))).collect(),
        }
    }

    /// What a client holds: the base, with the latest patch applied.
    #[derive(Default)]
    struct Client {
        version: u64,
        base: Vec<QueueItem>,
        rows: Vec<QueueItem>,
        patched: Vec<usize>,
    }

    impl Client {
        fn apply(&mut self, slices: Vec<StateSlice>) {
            for slice in slices {
                match slice {
                    StateSlice::Queue { items, version } => {
                        self.version = version;
                        self.rows = items.clone();
                        self.base = items;
                        self.patched.clear();
                    }
                    StateSlice::QueuePatch { base, items } if base == self.version => {
                        for i in self.patched.drain(..) {
                            self.rows[i] = self.base[i].clone();
                        }
                        for row in items {
                            let i = self
                                .base
                                .iter()
                                .position(|b| b.queue_item_id == row.queue_item_id)
                                .unwrap();
                            self.rows[i] = row;
                            self.patched.push(i);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    /// The same library once every track has landed, and the first record
    /// has been split off into an album of its own.
    fn landed(ids: &[i64]) -> Joins {
        Joins {
            album_ids: ids
                .iter()
                .map(|&id| (id, if id < 12 { 1_000 } else { id / 12 }))
                .collect(),
            sources: ids.iter().map(|&id| (id, (true, true))).collect(),
        }
    }

    fn whole(state: &SharedPlayerState) -> Vec<QueueItem> {
        whole_with(state, joins)
    }

    fn whole_with(state: &SharedPlayerState, joins: fn(&[i64]) -> Joins) -> Vec<QueueItem> {
        let entries = state.derive_visible_queue().entries;
        let ids: Vec<i64> = entries.iter().filter_map(|e| e.db_id).collect();
        let j = joins(&ids);
        entries
            .iter()
            .map(|e| QueueItem::from_entry(e, &j.album_ids, &j.sources))
            .collect()
    }

    fn sent_bytes(slices: &[StateSlice]) -> usize {
        let buf = <Vec<StateSlice> as uniffi::Lower<crate::UniFfiTag>>::lower(slices.to_vec());
        let len = buf.len();
        buf.destroy();
        len
    }

    #[test]
    fn a_client_applying_patches_holds_the_queue_as_it_is() {
        let (state, ids) = queue(40);
        let mut sender = QueueSender::default();
        let mut client = Client::default();
        client.apply(sender.update(&state, false, joins));
        assert_eq!(client.rows, whole(&state));

        state.set_cursor(Some(ids[3]));
        let slices = sender.update(&state, false, joins);
        assert!(matches!(slices[..], [StateSlice::QueuePatch { .. }]));
        client.apply(slices);
        assert_eq!(client.rows, whole(&state));

        // A track failing, and the cursor moving on past it: the later patch
        // still carries the earlier change, since both differ from the base.
        state.update_item_state(ids[4], ItemState::Failed("gone".into()));
        let _missed = sender.update(&state, false, joins);
        state.set_cursor(Some(ids[5]));
        client.apply(sender.update(&state, false, joins));
        assert_eq!(client.rows, whole(&state));

        // Moving back puts the rows the last patch changed back as they were.
        state.set_cursor(Some(ids[0]));
        state.update_item_state(ids[4], ItemState::Pending);
        client.apply(sender.update(&state, false, joins));
        assert_eq!(client.rows, whole(&state));
    }

    /// The library saying something new about the queue's tracks reaches a
    /// client as a patch of the rows it changed; and a patch for a base the
    /// client no longer holds changes nothing there.
    #[test]
    fn a_library_move_reaches_the_rows_it_changed() {
        let (state, _) = queue(40);
        let mut sender = QueueSender::default();
        let mut client = Client::default();
        client.apply(sender.update(&state, false, joins));

        let moved = sender.update(&state, true, landed);
        assert!(
            matches!(&moved[..], [StateSlice::QueuePatch { items, .. }] if items.len() == 26),
            "{moved:?}"
        );
        client.apply(moved.clone());
        assert_eq!(client.rows, whole_with(&state, landed));

        // Forgotten, as on a change of the device in view: sent whole again.
        sender.reset();
        let again = sender.update(&state, false, joins);
        assert!(matches!(again[0], StateSlice::Queue { .. }));
        client.apply(again);
        assert_eq!(client.rows, whole(&state));
        client.apply(moved);
        assert_eq!(client.rows, whole(&state));
    }

    #[test]
    fn an_edit_sends_the_queue_whole() {
        let (state, _) = queue(10);
        let mut sender = QueueSender::default();
        sender.update(&state, false, joins);
        state.add_items(vec![item(10, ItemState::Pending)]);
        let slices = sender.update(&state, false, joins);
        assert!(matches!(
            &slices[..],
            [StateSlice::Queue { items, .. }, StateSlice::QueuePatch { .. }] if items.len() == 11
        ));
    }

    #[test]
    fn a_patch_past_its_bound_sends_the_queue_whole() {
        // Ready, so every row the cursor passes reads as played.
        let (state, ids) = queue_of(PATCH_MAX * 3, ItemState::Ready);
        let mut sender = QueueSender::default();
        sender.update(&state, false, joins);
        state.set_cursor(Some(ids[PATCH_MAX * 2]));
        let slices = sender.update(&state, false, joins);
        assert!(matches!(slices[0], StateSlice::Queue { .. }));
    }

    /// What a change to a 50,000-row queue costs, and what it puts across the
    /// boundary. Run with `--nocapture` for the figures.
    ///
    /// Before patches, every track change and every download landing sent the
    /// queue whole: derived, every row's text copied twice, compared against
    /// the last, copied out to each client, and lowered across the FFI for the
    /// client to decode. Now that happens on an edit alone; a track change
    /// costs one pass over the rows' statuses and sends the two that moved.
    ///
    /// The time ceilings are loose on purpose, to catch an order of magnitude
    /// on a busy CI box. The byte bounds are exact in kind: a patch is a
    /// handful of rows whatever the queue's length.
    #[test]
    fn a_change_to_a_large_queue_sends_what_changed() {
        let (state, ids) = queue(ROWS);
        let out = EngineState::new();
        let mut seen = [0; crate::state::SLOTS];
        let mut sender = QueueSender::default();

        // The whole queue: what every change cost before, and an edit now.
        let start = Instant::now();
        let slices = sender.update(&state, true, joins);
        let built = start.elapsed();
        let start = Instant::now();
        for slice in slices {
            out.publish(slice);
        }
        let batch = out.since(&mut seen);
        let handed = start.elapsed();
        let whole_bytes = sent_bytes(&batch);
        println!(
            "{ROWS} rows whole: build {built:?}, publish + read {handed:?}, {whole_bytes} bytes"
        );

        let start = Instant::now();
        let readings = state.queue_readings();
        println!("{ROWS} rows, readings alone: {:?}", start.elapsed());
        drop(readings);

        // A track change.
        state.set_cursor(Some(ids[1]));
        let start = Instant::now();
        let slices = sender.update(&state, false, joins);
        let patched = start.elapsed();
        for slice in slices {
            out.publish(slice);
        }
        let batch = out.since(&mut seen);
        let patch_bytes = sent_bytes(&batch);
        println!("{ROWS} rows, track change: {patched:?}, {patch_bytes} bytes");
        assert!(
            matches!(&batch[..], [StateSlice::QueuePatch { items, .. }] if items.len() == 1),
            "{batch:?}"
        );

        // The playing track's download landing, which moves the library too.
        state.update_item_state(ids[1], ItemState::Ready);
        let start = Instant::now();
        let slices = sender.update(&state, true, joins);
        let landed = start.elapsed();
        for slice in slices {
            out.publish(slice);
        }
        let batch = out.since(&mut seen);
        let landed_bytes = sent_bytes(&batch);
        println!("{ROWS} rows, download landed: {landed:?}, {landed_bytes} bytes");
        assert!(
            matches!(&batch[..], [StateSlice::QueuePatch { items, .. }]
                if items.len() == 1 && items[0].status == EntryStatus::Playing),
            "{batch:?}"
        );

        assert!(patch_bytes < 2_000, "{patch_bytes}");
        assert!(landed_bytes < 2_000, "{landed_bytes}");
        assert!(whole_bytes > 1_000 * patch_bytes);
        assert!(patched < Duration::from_millis(200), "{patched:?}");
        assert!(landed < Duration::from_millis(500), "{landed:?}");
    }
}
