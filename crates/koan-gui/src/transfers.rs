//! Transfers' progress as the app draws it.
//!
//! The availability marks and the seek bar take each running transfer as a
//! `Glide`: its share at the engine's last report and the rate its bytes
//! arrive at, fed from the mirror's transfer figures, so a ring or a seek bar
//! drawn from it fills smoothly between reports on the GPU with no frame
//! drawn and nothing read on a timer. A transfer that ends drops its glide,
//! which stops it.
//!
//! The downloads page's bars read the byte counts directly (`progress`): one
//! reading of every running transfer per frame, shared by every bar drawn in
//! it, and only by a frame that draws one.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use koan_ffi::{EntryStatus, QueueItem, TransferFigure, TransferState};
use koan_kit::app::*;

use crate::components::{Availability, Glide};
use crate::engine::engine;
use crate::mirror::Mirror;

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

/// Each transfer's glide, by track.
pub struct Transfers {
    glides: HashMap<i64, Entry>,
    _figures: Subscription,
}

struct Entry {
    glide: Glide,
    /// The engine has reported this transfer: once it stops reporting it,
    /// the transfer has ended.
    reported: bool,
}

#[derive(Clone)]
struct TransfersHandle(Entity<Transfers>);

impl Global for TransfersHandle {}

/// After the mirror.
pub fn install(cx: &mut App) {
    let figures = cx.global::<Mirror>().figures.clone();
    let transfers = cx.new(|cx| {
        let mut transfers = Transfers {
            glides: HashMap::new(),
            _figures: cx.observe(&figures, |this: &mut Transfers, figures, cx| {
                this.take(&figures.read(cx).0, cx)
            }),
        };
        transfers.take(&figures.read(cx).0, cx);
        transfers
    });
    cx.set_global(TransfersHandle(transfers));
}

/// The glide for `track`'s transfer, made now if the engine has not reported
/// it yet, so the row that shows it is fed from its first report.
pub fn glide(track: i64, cx: &mut App) -> Glide {
    let Some(handle) = cx.try_global::<TransfersHandle>().cloned() else {
        return Glide::new();
    };
    handle.0.update(cx, |transfers, _| {
        transfers
            .glides
            .entry(track)
            .or_insert_with(|| Entry {
                glide: Glide::new(),
                reported: false,
            })
            .glide
            .clone()
    })
}

/// The glide for `track`'s transfer while the engine reports one.
pub fn reported(track: i64, cx: &App) -> Option<Glide> {
    let handle = cx.try_global::<TransfersHandle>()?;
    let entry = handle.0.read(cx).glides.get(&track)?;
    entry.reported.then(|| entry.glide.clone())
}

/// The entity to observe for transfers starting and ending.
pub fn entity(cx: &App) -> Option<Entity<Transfers>> {
    cx.try_global::<TransfersHandle>().map(|h| h.0.clone())
}

impl Transfers {
    /// A report of every transfer: each one's glide set from it, those that
    /// ended dropped. Notifies only when transfers start or end: a moving
    /// share reaches the screen through its glide, not a render.
    fn take(&mut self, figures: &[TransferFigure], cx: &mut Context<Self>) {
        let now = Instant::now();
        let mut changed = false;
        for figure in figures {
            let entry = self.glides.entry(figure.track_id).or_insert_with(|| Entry {
                glide: Glide::new(),
                reported: false,
            });
            changed |= !entry.reported;
            entry.reported = true;
            reading(&entry.glide, figure, now);
        }
        self.glides.retain(|track, entry| {
            let ended = entry.reported && !figures.iter().any(|f| f.track_id == *track);
            if ended {
                entry.glide.set_moving(false);
                changed = true;
            }
            !ended
        });
        if changed {
            cx.notify();
        }
    }
}

/// One report into a glide: the share and the rate it grows at, or turning
/// while the server gave no size.
fn reading(glide: &Glide, figure: &TransferFigure, now: Instant) {
    match figure.progress {
        Some(share) => {
            let rate = if figure.total_bytes > 0 {
                figure.bytes_per_second as f32 / figure.total_bytes as f32
            } else {
                0.0
            };
            glide.set(share as f32, rate, now);
            glide.set_moving(rate > 0.0 && share < 1.0);
        }
        None => glide.set_unknown(),
    }
}

/// A library row's mark: its transfer while it has one, else where its file
/// is.
pub fn of_track(
    track: i64,
    on_server: bool,
    on_disk: bool,
    transfer: Option<TransferState>,
    cx: &mut App,
) -> Option<Availability> {
    Some(match transfer {
        Some(TransferState::Queued) => Availability::Queued,
        Some(TransferState::Running) => Availability::Transferring(glide(track, cx)),
        Some(TransferState::Failed) => Availability::Failed,
        _ => stored(on_server, on_disk)?,
    })
}

/// A queue row's mark, from its status.
pub fn of_queue_item(item: &QueueItem, cx: &mut App) -> Option<Availability> {
    of_queue_status(item.status, item.track_id, item.on_server, item.on_disk, cx)
}

/// `of_queue_item` from the fields it reads, for a row whose item is
/// borrowed from the app.
pub fn of_queue_status(
    status: EntryStatus,
    track: Option<i64>,
    on_server: bool,
    on_disk: bool,
    cx: &mut App,
) -> Option<Availability> {
    Some(match status {
        EntryStatus::PriorityPending => Availability::Pending,
        EntryStatus::Failed => Availability::Failed,
        EntryStatus::Downloading => Availability::Transferring(match track {
            Some(track) => glide(track, cx),
            None => {
                let unknown = Glide::new();
                unknown.set_unknown();
                unknown
            }
        }),
        _ => stored(on_server, on_disk)?,
    })
}

fn stored(on_server: bool, on_disk: bool) -> Option<Availability> {
    (on_server || on_disk).then_some(Availability::Stored { on_server, on_disk })
}
