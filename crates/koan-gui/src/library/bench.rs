//! Scrolling the album grid against a large library and measuring it, for
//! `KOAN_GUI_BENCH=albums`. Trackpad scrolls are synthesised one per
//! frame through the window's own input path, gestures and momentum
//! included, so the heading and prefetch are what is measured too. Prints
//! per phase: frame intervals, the grid's own CPU time per frame (render and
//! layout of its rows), frames that uploaded new artwork, and blank tiles.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::*;

/// What one frame of the grid recorded.
#[derive(Default)]
pub struct Frame {
    /// The frame's whole CPU time on the main thread: from the scroll event's
    /// dispatch to the end of paint, uploads of new artwork included.
    pub cpu: Duration,
    /// Of which building the grid's rows' elements.
    pub rows: Duration,
    /// Tiles on screen drawn without their sleeve.
    pub blank: usize,
    /// Tiles on screen drawn with another size of their sleeve.
    pub standin: usize,
    /// Sleeves drawn for the first time this frame, each an atlas upload.
    pub uploads: usize,
}

#[derive(Default)]
pub struct Recorder {
    pub frame: Frame,
    /// When this frame's work began, and when its paint ended.
    began: Option<Instant>,
    painted: Option<Instant>,
    /// Frame intervals while nothing scrolls: the display's own rate.
    idle: Vec<Instant>,
    frames: Vec<(Instant, Frame)>,
    /// When each tile still blank came on screen.
    blank_since: HashMap<i64, Instant>,
    /// How long tiles stayed blank once on screen.
    blank_for: Vec<Duration>,
    seen: std::collections::HashSet<ImageId>,
}

impl Recorder {
    /// The window's paint finished: the end of this frame's work.
    pub fn painted(&mut self) {
        self.painted = Some(Instant::now());
    }

    /// A tile on screen this frame: whether it had its sleeve, and whether
    /// that was another size of it.
    pub fn tile(
        &mut self,
        album: i64,
        drawn: bool,
        standin: bool,
        image: Option<ImageId>,
        now: Instant,
    ) {
        if standin {
            self.frame.standin += 1;
        }
        if drawn {
            if let Some(since) = self.blank_since.remove(&album) {
                self.blank_for.push(now - since);
            }
            if let Some(id) = image
                && self.seen.insert(id)
            {
                self.frame.uploads += 1;
            }
        } else {
            self.frame.blank += 1;
            self.blank_since.entry(album).or_insert(now);
        }
    }

    fn end_frame(&mut self, at: Instant) {
        let mut frame = std::mem::take(&mut self.frame);
        if let (Some(began), Some(painted)) = (self.began.take(), self.painted.take())
            && painted > began
        {
            frame.cpu = painted - began;
        }
        self.frames.push((at, frame));
        self.began = Some(Instant::now());
    }
}

pub type Shared = Rc<RefCell<Recorder>>;

/// A phase of the run: a speed in points per second for a steady drag, or a
/// flick, which is a short drag released into momentum that decays.
#[derive(Clone, Copy)]
enum Phase {
    Steady {
        name: &'static str,
        speed: f32,
        secs: f32,
    },
    Flicks {
        name: &'static str,
        speed: f32,
        count: u32,
    },
}

const PHASES: [Phase; 4] = [
    Phase::Steady {
        name: "slow 1000 pt/s",
        speed: 1000.0,
        secs: 6.0,
    },
    Phase::Steady {
        name: "normal 3000 pt/s",
        speed: 3000.0,
        secs: 6.0,
    },
    Phase::Steady {
        name: "fast 8000 pt/s",
        speed: 8000.0,
        secs: 4.0,
    },
    Phase::Flicks {
        name: "flicks from 6000 pt/s",
        speed: 6000.0,
        count: 6,
    },
];

/// Run the phases, a frame at a time, then print and quit.
pub fn run(window: &mut Window, recorder: Shared, cx: &mut App) {
    let state = Rc::new(RefCell::new(Driver {
        recorder,
        phase: 0,
        started: None,
        last: None,
        marks: Vec::new(),
        flick: 0,
        flick_at: None,
        released: false,
    }));
    // A moment for the listing and the first screen to land.
    let settle = Instant::now() + Duration::from_secs(3);
    tick(state, settle, window, cx);
}

struct Driver {
    recorder: Shared,
    phase: usize,
    started: Option<Instant>,
    last: Option<Instant>,
    /// Where each phase's frames begin.
    marks: Vec<(usize, &'static str)>,
    flick: u32,
    flick_at: Option<Instant>,
    /// The current flick's fingers have lifted.
    released: bool,
}

fn scroll(window: &mut Window, cx: &mut App, dy: f32, phase: TouchPhase) {
    let bounds = window.bounds();
    let event = ScrollEvent {
        position: point(bounds.size.width / 2.0, bounds.size.height / 2.0),
        delta: ScrollDelta::Pixels(point(px(0.0), px(-dy))),
        modifiers: Modifiers::default(),
        touch_phase: phase,
        source: ScrollSource::Pointer,
    };
    window.dispatch_event(PlatformInput::Scroll(event), cx);
}

fn tick(state: Rc<RefCell<Driver>>, settle: Instant, window: &mut Window, cx: &mut App) {
    let now = Instant::now();
    if now < settle {
        state.borrow().recorder.borrow_mut().idle.push(now);
        window.on_next_frame(move |window, cx| tick(state, settle, window, cx));
        window.refresh();
        return;
    }
    let mut d = state.borrow_mut();
    d.recorder.borrow_mut().end_frame(now);
    let dt = d
        .last
        .map_or(1.0 / 120.0, |l| (now - l).as_secs_f32())
        .min(0.05);
    d.last = Some(now);
    let Some(phase) = PHASES.get(d.phase).copied() else {
        drop(d);
        report(&state.borrow());
        cx.quit();
        return;
    };
    let started = *d.started.get_or_insert_with(|| {
        let n = d.recorder.borrow().frames.len();
        let name = match phase {
            Phase::Steady { name, .. } | Phase::Flicks { name, .. } => name,
        };
        d.marks.push((n, name));
        now
    });
    let elapsed = (now - started).as_secs_f32();
    let mut next = false;
    match phase {
        Phase::Steady { speed, secs, .. } => {
            let phase = if elapsed == 0.0 {
                TouchPhase::Started
            } else {
                TouchPhase::Moved
            };
            scroll(window, cx, speed * dt, phase);
            next = elapsed >= secs;
            if next {
                scroll(window, cx, 0.0, TouchPhase::Ended);
            }
        }
        Phase::Flicks { speed, count, .. } => {
            // 120 ms of drag, then momentum decaying at e^(-3t) until it is
            // slower than a point a frame, then the next flick.
            let at = *d.flick_at.get_or_insert(now);
            let t = (now - at).as_secs_f32();
            if t < 0.12 {
                let phase = if t == 0.0 {
                    TouchPhase::Started
                } else {
                    TouchPhase::Moved
                };
                scroll(window, cx, speed * dt, phase);
            } else {
                let v = speed * (-3.0 * (t - 0.12)).exp();
                if !d.released {
                    d.released = true;
                    scroll(window, cx, 0.0, TouchPhase::Ended);
                }
                if v * dt < 1.0 {
                    d.flick += 1;
                    d.flick_at = None;
                    d.released = false;
                    next = d.flick >= count;
                } else {
                    scroll(window, cx, v * dt, TouchPhase::Moved);
                }
            }
        }
    }
    if next {
        d.phase += 1;
        d.started = None;
    }
    drop(d);
    window.on_next_frame(move |window, cx| tick(state, settle, window, cx));
    window.refresh();
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

fn sorted(mut v: Vec<f64>) -> Vec<f64> {
    v.sort_by(f64::total_cmp);
    v
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn report(d: &Driver) {
    let rec = d.recorder.borrow();
    // The display's interval: what frames come at while nothing moves.
    let idle = sorted(rec.idle.windows(2).map(|w| ms(w[1] - w[0])).collect());
    let refresh = percentile(&idle, 0.5);
    eprintln!("koan-gui bench: album grid, display interval {refresh:.2} ms");
    for (i, (start, name)) in d.marks.iter().enumerate() {
        let end = d.marks.get(i + 1).map_or(rec.frames.len(), |m| m.0);
        let frames = &rec.frames[*start..end];
        if frames.len() < 2 {
            continue;
        }
        let intervals = sorted(frames.windows(2).map(|w| ms(w[1].0 - w[0].0)).collect());
        let late = intervals.iter().filter(|i| **i > refresh * 1.5).count();
        let cpu = |upload: bool| {
            sorted(
                frames
                    .iter()
                    .filter(|f| (f.1.uploads > 0) == upload && f.1.cpu > Duration::ZERO)
                    .map(|f| ms(f.1.cpu))
                    .collect(),
            )
        };
        let (steady, uploading) = (cpu(false), cpu(true));
        let rows = sorted(frames.iter().map(|f| ms(f.1.rows)).collect());
        let blank_frames = frames.iter().filter(|f| f.1.blank > 0).count();
        let blank: usize = frames.iter().map(|f| f.1.blank).sum();
        let standin: usize = frames.iter().map(|f| f.1.standin).sum();
        eprintln!("  {name}: {} frames", frames.len());
        eprintln!(
            "    interval p50 {:.2} p95 {:.2} p99 {:.2} max {:.2} ms; {late} frames later than 1.5 display intervals",
            percentile(&intervals, 0.5),
            percentile(&intervals, 0.95),
            percentile(&intervals, 0.99),
            intervals.last().copied().unwrap_or(0.0),
        );
        eprintln!(
            "    frame cpu without uploads ({} frames) p50 {:.2} p95 {:.2} max {:.2} ms; with uploads ({} frames) p50 {:.2} p95 {:.2} max {:.2} ms",
            steady.len(),
            percentile(&steady, 0.5),
            percentile(&steady, 0.95),
            steady.last().copied().unwrap_or(0.0),
            uploading.len(),
            percentile(&uploading, 0.5),
            percentile(&uploading, 0.95),
            uploading.last().copied().unwrap_or(0.0),
        );
        eprintln!(
            "    of which building rows p50 {:.2} p95 {:.2} ms; {blank_frames} frames with blank tiles ({blank} tile-frames), {standin} stand-in tile-frames",
            percentile(&rows, 0.5),
            percentile(&rows, 0.95),
        );
    }
    let blank = sorted(rec.blank_for.iter().map(|d| ms(*d)).collect());
    eprintln!(
        "  tiles that came on screen blank: {}, blank for p50 {:.1} p95 {:.1} max {:.1} ms",
        blank.len(),
        percentile(&blank, 0.5),
        percentile(&blank, 0.95),
        blank.last().copied().unwrap_or(0.0),
    );
}
