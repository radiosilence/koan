//! Something's progress, reported now and then (a download, a scan), as
//! the `Glide`s its rings, slider and bar are drawn from: each report sets
//! where it stands and the rate it moves at, and the GPU fills between
//! reports with no frame drawn and nothing read on a timer. Past
//! `horizon` with no report, the glides rest where they reckoned rather
//! than run on.

use std::time::{Duration, Instant};

use gpui::Hsla;

use super::Glide;

/// One report: the share done while the whole is known, the share it grows
/// by each second, and when.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProgressReport {
    pub share: Option<f32>,
    pub rate: f32,
    pub at: Instant,
}

impl ProgressReport {
    /// Into a glide: the share and its rate, or turning while the whole is
    /// unknown.
    pub fn apply(self, glide: &Glide) {
        match self.share {
            Some(share) => {
                glide.set(share, self.rate, self.at);
                glide.set_moving(self.rate > 0.0 && share < 1.0);
            }
            None => glide.set_unknown(),
        }
    }

    /// Under way and not all done.
    pub fn partial(self) -> bool {
        self.share.is_some_and(|share| share < 1.0)
    }
}

/// One thing's progress as glides: a ring's per colour (a ring writes its
/// colour into its glide), a slider's reach and a bar's, each styled by
/// what draws it.
pub struct ProgressGlides {
    horizon: Duration,
    rings: Vec<(Hsla, Glide)>,
    reach: Glide,
    bar: Glide,
    last: Option<ProgressReport>,
}

impl ProgressGlides {
    /// Glides reckoned at most `horizon` past a report: about the gap
    /// between a healthy feed's reports.
    pub fn new(horizon: Duration) -> Self {
        let glide = || {
            let glide = Glide::new();
            glide.reckon_for(horizon);
            glide
        };
        Self {
            horizon,
            rings: Vec::new(),
            reach: glide(),
            bar: glide(),
            last: None,
        }
    }

    /// A ring's glide in `color`, made now if none is: a new colour starts
    /// where the last report left the others, or turning before any.
    pub fn ring(&mut self, color: Hsla) -> Glide {
        if let Some((_, glide)) = self.rings.iter().find(|(c, _)| *c == color) {
            return glide.clone();
        }
        let glide = Glide::new();
        glide.reckon_for(self.horizon);
        match self.last {
            Some(last) => last.apply(&glide),
            None => glide.set_unknown(),
        }
        self.rings.push((color, glide.clone()));
        glide
    }

    /// A slider's reach (`Slider::reach`).
    pub fn reach(&self) -> &Glide {
        &self.reach
    }

    /// A bar's (`ProgressBar`).
    pub fn bar(&self) -> &Glide {
        &self.bar
    }

    /// The last report taken.
    pub fn last(&self) -> Option<ProgressReport> {
        self.last
    }

    /// Takes a report into every glide. A share below the last one is the
    /// work starting afresh (a download retried): the glides start again
    /// from it rather than standing where the last run finished, which a
    /// reckoned glide would otherwise hold. Whether it is now partial when
    /// it was not, or the other way, or this is the first report: what a
    /// page drawing only partial progress must draw again for.
    pub fn take(&mut self, report: ProgressReport) -> bool {
        let changed = self
            .last
            .is_none_or(|last| last.partial() != report.partial());
        let afresh = self
            .last
            .and_then(|last| last.share)
            .is_some_and(|last| report.share.is_none_or(|share| share < last));
        self.last = Some(report);
        for glide in self.glides() {
            if afresh {
                glide.set_unknown();
            }
            report.apply(glide);
        }
        changed
    }

    /// It has ended: every glide stops where it stands.
    pub fn end(&self) {
        for glide in self.glides() {
            glide.set_moving(false);
        }
    }

    fn glides(&self) -> impl Iterator<Item = &Glide> {
        self.rings
            .iter()
            .map(|(_, glide)| glide)
            .chain([&self.reach, &self.bar])
    }
}
