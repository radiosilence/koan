//! Transfers' progress as the availability marks draw it: one reading of
//! every running transfer's byte counts per frame (`transfer_readings`, a
//! lock held for one pass over a few dozen entries), shared by every mark
//! drawn in that frame. Read only by a frame that draws a transferring mark,
//! which asks for the next one; nothing transferring or nothing on screen,
//! and nothing is read.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use koan_ffi::{EntryStatus, QueueItem, TransferState};
use koan_kit::app::*;

use crate::components::{Availability, ProgressReader};
use crate::engine::engine;

/// Readings younger than this are the same frame's: shorter than a frame at
/// 144 Hz, longer than one frame's drawing.
const FRAME: Duration = Duration::from_millis(4);

struct Readings {
    at: Instant,
    progress: HashMap<i64, Option<f32>>,
}

thread_local! {
    static READINGS: RefCell<Option<Readings>> = const { RefCell::new(None) };
}

/// The progress of `track`'s transfer as of this frame.
pub fn progress(track: i64, cx: &App) -> Option<f32> {
    READINGS.with(|cell| {
        let mut cell = cell.borrow_mut();
        let now = Instant::now();
        if cell.as_ref().is_none_or(|r| now - r.at > FRAME) {
            *cell = Some(Readings {
                at: now,
                progress: engine(cx)
                    .transfer_readings()
                    .into_iter()
                    .map(|f| (f.track_id, f.progress.map(|p| p as f32)))
                    .collect(),
            });
        }
        cell.as_ref()?.progress.get(&track).copied().flatten()
    })
}

/// A reader of `track`'s progress, for its mark.
pub fn reader(track: i64) -> ProgressReader {
    Rc::new(move |cx| progress(track, cx))
}

/// A library row's mark: its transfer while it has one, else where its file
/// is.
pub fn of_track(
    track: i64,
    on_server: bool,
    on_disk: bool,
    transfer: Option<TransferState>,
) -> Option<Availability> {
    Some(match transfer {
        Some(TransferState::Queued) => Availability::Queued,
        Some(TransferState::Running) => Availability::Transferring(reader(track)),
        Some(TransferState::Failed) => Availability::Failed,
        _ => stored(on_server, on_disk)?,
    })
}

/// A queue row's mark, from its status.
pub fn of_queue_item(item: &QueueItem) -> Option<Availability> {
    Some(match item.status {
        EntryStatus::PriorityPending => Availability::Pending,
        EntryStatus::Failed => Availability::Failed,
        EntryStatus::Downloading => Availability::Transferring(match item.track_id {
            Some(track) => reader(track),
            None => Rc::new(|_| None),
        }),
        _ => stored(item.on_server, item.on_disk)?,
    })
}

fn stored(on_server: bool, on_disk: bool) -> Option<Availability> {
    (on_server || on_disk).then_some(Availability::Stored { on_server, on_disk })
}
