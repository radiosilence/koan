//! Transfers' progress as the app draws it.
//!
//! The availability marks and the seek bar take each running transfer as a
//! `Glide`: its share at the engine's last report and the rate its bytes
//! arrive at, fed from the mirror's transfer figures, so a ring or a seek bar
//! drawn from it fills smoothly between reports on the GPU with no frame
//! drawn and nothing read on a timer. A transfer that ends drops its glide,
//! which stops it.
//!
//! The downloads page's bars take the same glides (`bar`).

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Instant;

use koan_kit::app::*;
use koan_runtime::{EntryStatus, Transfer, TransferFigure, TransferState};

use crate::components::{Availability, Glide, RingGlide};
use crate::mirror::Mirror;
use crate::theme::Hsla;

/// Each transfer's glides, by track.
pub struct Transfers {
    glides: HashMap<i64, Entry>,
    /// The tracks whose transfer is running, from the Transfers slice: a
    /// figure is kept for queued, finished and failed transfers too.
    running: HashSet<i64>,
    _figures: Subscription,
    _transfers: Subscription,
}

struct Entry {
    /// One per colour a ring is drawn in, since a ring writes its colour
    /// into its glide.
    rings: Vec<(Hsla, Glide)>,
    /// The seek bar's, styled by the slider.
    reach: Glide,
    /// The downloads page's bar, styled by the bar.
    bar: Glide,
    /// The last report, for a glide made after it.
    last: Option<Report>,
    /// The engine has reported this transfer: once it stops reporting it,
    /// the transfer has ended.
    reported: bool,
}

impl Entry {
    fn new() -> Self {
        Self {
            rings: Vec::new(),
            reach: Glide::new(),
            bar: Glide::new(),
            last: None,
            reported: false,
        }
    }

    fn glides(&self) -> impl Iterator<Item = &Glide> {
        self.rings
            .iter()
            .map(|(_, g)| g)
            .chain([&self.reach, &self.bar])
    }
}

#[derive(Clone)]
struct TransfersHandle(Entity<Transfers>);

impl Global for TransfersHandle {}

/// After the mirror.
pub fn install(cx: &mut App) {
    let mirror = cx.global::<Mirror>().clone();
    let transfers = cx.new(|cx| {
        let mut transfers = Transfers {
            glides: HashMap::new(),
            running: HashSet::new(),
            _figures: cx.observe(&mirror.figures, |this: &mut Transfers, figures, cx| {
                let figures = figures.read(cx).0.clone();
                this.take(&figures, cx)
            }),
            _transfers: cx.observe(&mirror.transfers, |this: &mut Transfers, list, cx| {
                let list = list.read(cx).0.clone();
                this.states(&list, cx)
            }),
        };
        let list = mirror.transfers.read(cx).0.clone();
        transfers.states(&list, cx);
        let figures = mirror.figures.read(cx).0.clone();
        transfers.take(&figures, cx);
        transfers
    });
    cx.set_global(TransfersHandle(transfers));
}

/// A ring's glide for `track`'s transfer in the colour `rgba`, made now if
/// the engine has not reported it yet, so the row that shows it is fed from
/// its first report.
fn ring(track: i64, color: Hsla, cx: &mut App) -> Glide {
    let Some(handle) = cx.try_global::<TransfersHandle>().cloned() else {
        let unknown = Glide::new();
        unknown.set_unknown();
        return unknown;
    };
    let key = color;
    handle.0.update(cx, |transfers, _| {
        let entry = transfers.glides.entry(track).or_insert_with(Entry::new);
        if let Some((_, glide)) = entry.rings.iter().find(|(k, _)| *k == key) {
            return glide.clone();
        }
        // A new colour starts where the last report left the others.
        let glide = Glide::new();
        match entry.last {
            Some(last) => last.apply(&glide),
            None => glide.set_unknown(),
        }
        entry.rings.push((key, glide.clone()));
        glide
    })
}

/// A row's ring for `track`'s transfer, in whatever colour the mark draws.
fn ring_for(track: i64) -> RingGlide {
    Rc::new(move |color, cx| ring(track, color, cx))
}

/// The seek bar's glide for `track`'s transfer: only while it runs with a
/// size known and not all here.
pub fn reach(track: i64, cx: &App) -> Option<Glide> {
    let handle = cx.try_global::<TransfersHandle>()?;
    let transfers = handle.0.read(cx);
    let entry = transfers.glides.get(&track)?;
    let partial = entry
        .last
        .and_then(|last| last.share)
        .is_some_and(|share| share < 1.0);
    (transfers.running.contains(&track) && partial).then(|| entry.reach.clone())
}

/// The downloads page bar's glide for `track`'s transfer, fed from its
/// reports; a transfer not reported yet has an empty one until the page
/// draws again for its first report.
pub fn bar(track: i64, cx: &App) -> Glide {
    cx.try_global::<TransfersHandle>()
        .and_then(|handle| handle.0.read(cx).glides.get(&track).map(|e| e.bar.clone()))
        .unwrap_or_default()
}

/// The entity to observe for transfers starting and ending.
pub fn entity(cx: &App) -> Option<Entity<Transfers>> {
    cx.try_global::<TransfersHandle>().map(|h| h.0.clone())
}

impl Transfers {
    /// A report of every transfer: each one's glides set from it, those that
    /// ended dropped. Notifies only when what the seek bar shows can change:
    /// a moving share reaches the screen through its glides, not a render.
    fn take(&mut self, figures: &[TransferFigure], cx: &mut Context<Self>) {
        let now = Instant::now();
        let mut changed = false;
        for figure in figures {
            let entry = self
                .glides
                .entry(figure.track_id)
                .or_insert_with(Entry::new);
            let report = Report::of(figure, now);
            let partial = |r: Option<Report>| r.and_then(|r| r.share).is_some_and(|s| s < 1.0);
            changed |= !entry.reported || partial(Some(report)) != partial(entry.last);
            entry.reported = true;
            entry.last = Some(report);
            for glide in entry.glides() {
                report.apply(glide);
            }
        }
        self.glides.retain(|track, entry| {
            let ended = entry.reported && !figures.iter().any(|f| f.track_id == *track);
            if ended {
                for glide in entry.glides() {
                    glide.set_moving(false);
                }
                changed = true;
            }
            !ended
        });
        if changed {
            cx.notify();
        }
    }

    /// Which transfers run, from the structural slice.
    fn states(&mut self, transfers: &[Transfer], cx: &mut Context<Self>) {
        let running: HashSet<i64> = transfers
            .iter()
            .filter(|t| t.state == TransferState::Running)
            .map(|t| t.track_id)
            .collect();
        if running != self.running {
            self.running = running;
            cx.notify();
        }
    }
}

/// One report of a transfer: its share while its size is known, the rate it
/// grows at, and when.
#[derive(Clone, Copy)]
struct Report {
    share: Option<f32>,
    rate: f32,
    at: Instant,
}

impl Report {
    fn of(figure: &TransferFigure, at: Instant) -> Self {
        let rate = if figure.total_bytes > 0 {
            figure.bytes_per_second as f32 / figure.total_bytes as f32
        } else {
            0.0
        };
        Self {
            share: figure.progress.map(|p| p as f32),
            rate,
            at,
        }
    }

    /// Into a glide: the share and its rate, or turning while the server
    /// gave no size.
    fn apply(self, glide: &Glide) {
        match self.share {
            Some(share) => {
                glide.set(share, self.rate, self.at);
                glide.set_moving(self.rate > 0.0 && share < 1.0);
            }
            None => glide.set_unknown(),
        }
    }
}

/// A row's mark, the one rule every list draws by: the track's transfer
/// while the store lists one, else where its file is. A transfer that has
/// finished means the file is here, from the moment it settles: the library
/// says so too a little later, and the store forgets a finished transfer
/// whose file is cleared or evicted.
pub fn of_track(
    track: i64,
    on_server: bool,
    on_disk: bool,
    transfer: Option<TransferState>,
) -> Option<Availability> {
    match transfer {
        Some(TransferState::Queued) => Some(Availability::Queued),
        Some(TransferState::Running) => Some(Availability::Transferring(ring_for(track))),
        Some(TransferState::Failed) => Some(Availability::Failed),
        Some(TransferState::Done) => stored(on_server, true),
        None => stored(on_server, on_disk),
    }
}

/// A queue or playlist row's mark: `of_track`, except that a row the person
/// asked to play that is still waiting for its transfer to start shows it is
/// wanted, and one whose load failed with no transfer listed shows the
/// failure.
pub fn of_queue_row(
    status: EntryStatus,
    track: Option<i64>,
    on_server: bool,
    on_disk: bool,
    transfer: Option<TransferState>,
) -> Option<Availability> {
    let waiting = matches!(transfer, None | Some(TransferState::Queued));
    match (status, track) {
        (EntryStatus::PriorityPending, _) if waiting && !on_disk => Some(Availability::Pending),
        (EntryStatus::Failed, _) if transfer.is_none() => Some(Availability::Failed),
        (_, Some(track)) => of_track(track, on_server, on_disk, transfer),
        _ => stored(on_server, on_disk),
    }
}

fn stored(on_server: bool, on_disk: bool) -> Option<Availability> {
    (on_server || on_disk).then_some(Availability::Stored { on_server, on_disk })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored_here(mark: Option<Availability>) -> bool {
        matches!(mark, Some(Availability::Stored { on_disk: true, .. }))
    }

    /// A finished transfer is the file here at once: no empty cloud while
    /// the library catches up.
    #[test]
    fn a_finished_transfer_is_on_this_device() {
        assert!(stored_here(of_track(
            1,
            true,
            false,
            Some(TransferState::Done)
        )));
        assert!(stored_here(of_queue_row(
            EntryStatus::Queued,
            Some(1),
            true,
            false,
            Some(TransferState::Done),
        )));
    }

    /// A running transfer is a ring whatever the row's load state: a track
    /// streaming while it downloads is ready to play and still arriving.
    #[test]
    fn a_running_transfer_is_a_ring_whatever_the_status() {
        for status in [
            EntryStatus::Playing,
            EntryStatus::Queued,
            EntryStatus::Downloading,
        ] {
            let mark = of_queue_row(status, Some(1), true, false, Some(TransferState::Running));
            assert!(matches!(mark, Some(Availability::Transferring(_))));
        }
    }

    /// Waiting for its transfer to start, the track asked for shows it is
    /// wanted; on disk already, it does not.
    #[test]
    fn a_wanted_track_waits_only_while_it_is_not_here() {
        let waiting = of_queue_row(EntryStatus::PriorityPending, Some(1), true, false, None);
        assert!(matches!(waiting, Some(Availability::Pending)));
        let here = of_queue_row(EntryStatus::PriorityPending, Some(1), true, true, None);
        assert!(stored_here(here));
    }
}
