//! What the album grid's benchmark counts beside GPUI's frame timings: tiles
//! drawn blank or with a sleeve of another size, how long each tile that came
//! on screen blank stayed so, and how many sleeves were drawn for the first
//! time (each an upload to the GPU's atlas). See `benches/album_grid.rs`.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::ImageId;

#[derive(Default)]
pub struct Recorder {
    tiles: u64,
    blank: u64,
    standin: u64,
    uploads: u64,
    /// When each tile still blank came on screen.
    blank_since: HashMap<i64, Instant>,
    /// How long tiles stayed blank once on screen.
    blank_for: Vec<Duration>,
    seen: HashSet<ImageId>,
}

pub type Shared = Rc<RefCell<Recorder>>;

impl Recorder {
    /// A tile on screen in a frame: whether it had its sleeve, and whether
    /// that was another size of it.
    pub fn tile(
        &mut self,
        album: i64,
        drawn: bool,
        standin: bool,
        image: Option<ImageId>,
        now: Instant,
    ) {
        self.tiles += 1;
        if standin {
            self.standin += 1;
        }
        if drawn {
            if let Some(since) = self.blank_since.remove(&album) {
                self.blank_for.push(now - since);
            }
            if let Some(id) = image
                && self.seen.insert(id)
            {
                self.uploads += 1;
            }
        } else {
            self.blank += 1;
            self.blank_since.entry(album).or_insert(now);
        }
    }

    pub fn summary(&self) -> String {
        let mut blank: Vec<f64> = self
            .blank_for
            .iter()
            .map(|d| d.as_secs_f64() * 1000.0)
            .collect();
        blank.sort_by(f64::total_cmp);
        let at = |p: f64| {
            blank
                .get(((blank.len().max(1) - 1) as f64 * p).round() as usize)
                .copied()
                .unwrap_or(0.0)
        };
        format!(
            "{} tile-frames, {} blank, {} stand-in, {} first draws; {} tiles came on screen blank, blank for p50 {:.1} p95 {:.1} max {:.1} ms",
            self.tiles,
            self.blank,
            self.standin,
            self.uploads,
            blank.len(),
            at(0.5),
            at(0.95),
            blank.last().copied().unwrap_or(0.0),
        )
    }
}
