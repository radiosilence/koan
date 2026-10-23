//! A device's audio levels, drawn by another device controlling it.
//!
//! The playing indicator's bars read the analyser. While one device controls
//! another, the music is on the other device, so its levels come over the link:
//! the controller subscribes while it has bars on screen (`LinkCommand::
//! WatchLevels`), and the playing device sends a [`Frame`] for every frame its
//! analyser publishes until the last subscriber goes. Nothing is sent while
//! nobody watches, and the analyser parks as it does with no local reader.
//!
//! Frames are keyed by the playhead position they were heard at, never by wall
//! time, and they are their own clock: the controller follows the remote
//! playhead from the frames as they arrive, draws slightly behind it and
//! interpolates between the two frames either side, so a late or dropped frame
//! is covered by the delay ([`Interp`]). The device's state reports are not
//! that clock. They are sent only when the playhead strays three seconds from
//! where a report would put it, so one taken while the output was still
//! starting (a rate switch's lead-in, a session coming up) runs ahead of the
//! music for the whole track, and bars drawn by it would wait for frames that
//! never come. When there is nothing to draw from — paused, a stalled link,
//! the buffer run dry — the bars ease to rest. Nothing is extrapolated beyond
//! the frames: what is drawn is the device's own analysis, smoothed.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Weak};
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};
use serde::{Deserialize, Serialize};

use crate::audio::viz::{VizLevels, VizSnapshot};
use crate::remote::link::LinkCommand;
use crate::remote::wire::Waker;
use crate::signal::Wake;

/// One analysed frame, as the link carries it: milliseconds into the track it
/// was heard at, then the low, mid and high bands in thousandths. An array, so
/// a frame is about forty bytes of JSON at the analyser's full rate.
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frame(pub u64, pub u16, pub u16, pub u16);

impl Frame {
    pub fn new(at_ms: u64, levels: VizLevels) -> Self {
        let q = |v: f32| (v.clamp(0.0, 1.0) * 1000.0).round() as u16;
        Self(at_ms, q(levels.low), q(levels.mid), q(levels.high))
    }

    pub fn at_ms(&self) -> u64 {
        self.0
    }

    pub fn levels(&self) -> VizLevels {
        VizLevels {
            low: f32::from(self.1) / 1000.0,
            mid: f32::from(self.2) / 1000.0,
            high: f32::from(self.3) / 1000.0,
        }
    }
}

// --- The playing device -----------------------------------------------------

/// The frames this device sends, and the link sessions waiting for them.
///
/// One thread reads the analyser for every session watching, and only while
/// one is: it waits on the analyser's frames, so with music paused it sleeps,
/// and with no session watching it parks.
pub struct Feed {
    inner: Mutex<FeedState>,
    changed: Condvar,
    next_id: AtomicU64,
}

struct FeedState {
    source: Option<Arc<Source>>,
    watchers: Vec<(u64, Weak<Waker>)>,
    /// The newest frame and its sequence number, which each `Watch` compares
    /// with the last one it sent.
    latest: Option<(u64, Frame)>,
    pumping: bool,
}

struct Source {
    viz: Arc<VizSnapshot>,
    position_ms: Box<dyn Fn() -> u64 + Send + Sync>,
}

static FEED: LazyLock<Arc<Feed>> = LazyLock::new(Feed::new);

/// This process's feed.
pub fn feed() -> &'static Arc<Feed> {
    &FEED
}

impl Feed {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(FeedState {
                source: None,
                watchers: Vec::new(),
                latest: None,
                pumping: false,
            }),
            changed: Condvar::new(),
            next_id: AtomicU64::new(1),
        })
    }

    /// Where frames come from: the analyser, and the playhead each is heard at.
    pub fn provide(
        self: &Arc<Self>,
        viz: Arc<VizSnapshot>,
        position_ms: impl Fn() -> u64 + Send + Sync + 'static,
    ) {
        let mut inner = self.inner.lock();
        inner.source = Some(Arc::new(Source {
            viz,
            position_ms: Box::new(position_ms),
        }));
        self.changed.notify_all();
    }

    /// Start sending frames to the session `waker` wakes. Frames stop when the
    /// returned `Watch` is dropped: on unsubscribe, or with the session.
    pub fn watch(self: &Arc<Self>, waker: &Arc<Waker>) -> Watch {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut inner = self.inner.lock();
        inner.watchers.push((id, Arc::downgrade(waker)));
        let sent = inner.latest.map_or(0, |(seq, _)| seq);
        if !inner.pumping {
            inner.pumping = true;
            let feed = Arc::clone(self);
            std::thread::Builder::new()
                .name("koan-levels".into())
                .spawn(move || feed.pump())
                .expect("failed to spawn the levels thread");
        }
        self.changed.notify_all();
        Watch {
            feed: Arc::downgrade(self),
            id,
            sent,
        }
    }

    /// How many sessions are watching.
    pub fn watchers(&self) -> usize {
        self.inner.lock().watchers.len()
    }

    fn pump(&self) {
        loop {
            let source = {
                let mut inner = self.inner.lock();
                loop {
                    inner.watchers.retain(|(_, w)| w.strong_count() > 0);
                    match &inner.source {
                        Some(source) if !inner.watchers.is_empty() => break Arc::clone(source),
                        _ => self.changed.wait(&mut inner),
                    }
                }
            };
            // Wanting a frame wakes an analyser parked for want of a reader.
            let seen = source.viz.frames().generation();
            source.viz.touch();
            source.viz.frames().wait(seen);
            let frame = Frame::new((source.position_ms)(), source.viz.levels());
            let mut inner = self.inner.lock();
            let seq = inner.latest.map_or(0, |(seq, _)| seq) + 1;
            inner.latest = Some((seq, frame));
            inner.watchers.retain(|(_, w)| match w.upgrade() {
                Some(w) => {
                    w.wake();
                    true
                }
                None => false,
            });
        }
    }

    #[cfg(test)]
    fn publish_for_test(&self, frame: Frame) {
        let mut inner = self.inner.lock();
        let seq = inner.latest.map_or(0, |(seq, _)| seq) + 1;
        inner.latest = Some((seq, frame));
    }
}

/// One session's subscription. See `Feed::watch`.
pub struct Watch {
    feed: Weak<Feed>,
    id: u64,
    sent: u64,
}

impl Watch {
    /// The newest frame this session has not sent, if one has arrived since.
    /// A session that fell behind sends the newest, not the backlog.
    pub fn take(&mut self) -> Option<Frame> {
        let feed = self.feed.upgrade()?;
        let (seq, frame) = feed.inner.lock().latest?;
        (seq > self.sent).then(|| {
            self.sent = seq;
            frame
        })
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        if let Some(feed) = self.feed.upgrade() {
            feed.inner.lock().watchers.retain(|(id, _)| *id != self.id);
        }
    }
}

// --- The controlling device -------------------------------------------------

/// How far behind the playhead frames are drawn, in frame intervals: enough
/// for one late or lost frame to be bridged rather than seen.
const DELAY_FRAMES: f32 = 2.0;
/// A frame further ahead of the last than this is a seek: what came before it
/// says nothing about what follows.
const JUMP_MS: u64 = 1_000;
/// A frame further behind the last than this is a seek back or another track.
const BACK_MS: u64 = 50;
/// How fast the bars settle once there is nothing to draw from.
const EASE_HALF_LIFE: Duration = Duration::from_millis(80);
/// Below this the bars are at rest.
const REST: f32 = 0.001;
const MAX_FRAMES: usize = 240;

/// Frames from another device, drawn behind its playhead. See the module note.
#[derive(Debug)]
pub struct Interp {
    frames: VecDeque<(u64, VizLevels)>,
    /// The sender's frame interval as seen, in milliseconds: what the delay is
    /// measured in. Starts at a 60 Hz analyser's.
    spacing_ms: f32,
    /// The remote playhead as the frames give it: where it stood at an
    /// instant here. Set back by a frame that arrives behind it, and moved on
    /// a little by one ahead of it, so it settles with the latest arrivals and
    /// the delay is left to cover jitter.
    clock: Option<(Instant, u64)>,
    shown: VizLevels,
    sampled: Option<Instant>,
}

impl Default for Interp {
    fn default() -> Self {
        Self {
            frames: VecDeque::new(),
            spacing_ms: 1000.0 / 60.0,
            clock: None,
            shown: VizLevels::default(),
            sampled: None,
        }
    }
}

impl Interp {
    /// A frame heard at `at_ms`, arriving `now`. Clears what came before on a
    /// seek or a new track, any move back included; replaces the newest when
    /// the playhead has not moved, as it does while a paused analyser lets its
    /// bars fall.
    pub fn push(&mut self, at_ms: u64, levels: VizLevels, now: Instant) {
        if let Some(&(newest, _)) = self.frames.back() {
            // Back by more than a frame's jitter is a seek or another track,
            // however short the way; forward by more than a second, a seek.
            if at_ms + BACK_MS < newest || at_ms > newest + JUMP_MS {
                self.clear();
            } else if at_ms <= newest {
                if let Some(back) = self.frames.back_mut() {
                    *back = (newest, levels);
                }
                return;
            } else {
                let gap = (at_ms - newest) as f32;
                if gap < 200.0 {
                    self.spacing_ms = self.spacing_ms * 0.8 + gap * 0.2;
                }
            }
        }
        self.frames.push_back((at_ms, levels));
        while self.frames.len() > MAX_FRAMES {
            self.frames.pop_front();
        }
        self.clock = Some(match self.playhead(now) {
            Some(expected) if at_ms > expected => (now, expected + (at_ms - expected) / 8),
            _ => (now, at_ms),
        });
    }

    /// Forget every frame, and the clock they kept: the playhead has moved
    /// somewhere they do not cover.
    pub fn clear(&mut self) {
        self.frames.clear();
        self.clock = None;
    }

    /// Where the remote playhead is `now`, by the frames.
    fn playhead(&self, now: Instant) -> Option<u64> {
        let (at, ms) = self.clock?;
        Some(ms + now.saturating_duration_since(at).as_millis() as u64)
    }

    /// How far behind the playhead frames are drawn.
    pub fn delay_ms(&self) -> u64 {
        (self.spacing_ms * DELAY_FRAMES).round() as u64
    }

    /// The levels to draw `now`, with the remote device playing or not.
    pub fn sample(&mut self, playing: bool, now: Instant) -> VizLevels {
        let elapsed = self
            .sampled
            .map_or(Duration::ZERO, |at| now.saturating_duration_since(at));
        self.sampled = Some(now);
        let at = self
            .playhead(now)
            .map(|p| p.saturating_sub(self.delay_ms()));
        match at.and_then(|at| self.between(at)).filter(|_| playing) {
            Some(levels) => self.shown = levels,
            None => {
                let keep = 0.5f32.powf(elapsed.as_secs_f32() / EASE_HALF_LIFE.as_secs_f32());
                self.shown = VizLevels {
                    low: self.shown.low * keep,
                    mid: self.shown.mid * keep,
                    high: self.shown.high * keep,
                };
                // Run dry: nothing here will be drawn.
                let stale = self
                    .frames
                    .back()
                    .zip(at)
                    .is_some_and(|(&(newest, _), at)| at > newest + JUMP_MS);
                if stale || !playing {
                    self.clear();
                }
            }
        }
        self.shown
    }

    /// The levels at `at`, between the frames either side of it, dropping
    /// those already behind. `None` outside what the frames cover.
    fn between(&mut self, at: u64) -> Option<VizLevels> {
        let after = self.frames.iter().position(|&(t, _)| t >= at)?;
        if after == 0 {
            return (self.frames[0].0 == at).then_some(self.frames[0].1);
        }
        self.frames.drain(..after - 1);
        let (t0, a) = self.frames[0];
        let (t1, b) = self.frames[1];
        let f = (at - t0) as f32 / (t1 - t0) as f32;
        let lerp = |x: f32, y: f32| x + (y - x) * f;
        Some(VizLevels {
            low: lerp(a.low, b.low),
            mid: lerp(a.mid, b.mid),
            high: lerp(a.high, b.high),
        })
    }

    /// Whether drawing again could show anything different: frames to draw
    /// from, or bars still settling.
    pub fn moving(&self) -> bool {
        !self.frames.is_empty()
            || self.shown.low > REST
            || self.shown.mid > REST
            || self.shown.high > REST
    }
}

/// How long a playing target may go unheard while watched before it is asked
/// again: it may have relinked, or the server restarted, and lost the watch.
const STALL: Duration = Duration::from_secs(3);

/// The controlling side: subscribed, per device, while bars for it are on
/// screen, and ticking at the display's rate while there is something to
/// draw. With nothing to draw it parks; while a view is held it wakes only to
/// renew a watch that has gone quiet.
pub struct Remote {
    inner: Mutex<RemoteState>,
    /// Bumped once per display frame while the bars move: what a stream of
    /// levels waits on.
    ticks: Wake,
    tick: Condvar,
    /// Gets a `WatchLevels` to a device over a connection that is up now, and
    /// says whether it did. Never queued, never a push: levels are no reason
    /// to wake anything. `devices::send_live`, but for tests.
    send: Box<SendWatch>,
}

/// See `Remote::send`.
type SendWatch = dyn Fn(&str, LinkCommand) -> bool + Send + Sync;

struct RemoteState {
    views: Vec<Viewed>,
    /// The device whose frames `interp` holds: the one viewed last.
    drawing: Option<String>,
    interp: Interp,
    /// When `drawing` was last heard from.
    heard: Option<Instant>,
    interval: Duration,
    ticking: bool,
}

/// A device with bars for it on screen, and how many.
struct Viewed {
    target: String,
    count: usize,
    /// The device list's version when it was last asked and the ask went out;
    /// asked again whenever the list moves after that, which is when a link
    /// or a connection on the network comes or goes. `None` after an ask that
    /// found no way through.
    asked: Option<u64>,
}

static REMOTE: LazyLock<Arc<Remote>> =
    LazyLock::new(|| Remote::new(crate::remote::devices::send_live));

pub fn remote() -> &'static Arc<Remote> {
    &REMOTE
}

impl Remote {
    pub fn new(send: impl Fn(&str, LinkCommand) -> bool + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(RemoteState {
                views: Vec::new(),
                drawing: None,
                interp: Interp::default(),
                heard: None,
                interval: Duration::from_micros(1_000_000 / 60),
                ticking: false,
            }),
            ticks: Wake::new(),
            tick: Condvar::new(),
            send: Box::new(send),
        })
    }

    /// Watch `target`'s levels until the returned `View` is dropped.
    pub fn view(self: &Arc<Self>, target: String) -> View {
        let mut inner = self.inner.lock();
        match inner.views.iter_mut().find(|v| v.target == target) {
            Some(v) => v.count += 1,
            None => inner.views.push(Viewed {
                target: target.clone(),
                count: 1,
                asked: None,
            }),
        }
        if inner.drawing.as_deref() != Some(&target) {
            inner.drawing = Some(target.clone());
            inner.interp = Interp::default();
            inner.heard = None;
        }
        if !inner.ticking {
            inner.ticking = true;
            let remote = Arc::clone(self);
            std::thread::Builder::new()
                .name("koan-levels-tick".into())
                .spawn(move || remote.run_ticks())
                .expect("failed to spawn the levels ticker");
        }
        self.tick.notify_all();
        drop(inner);
        self.ask(false);
        View {
            remote: Arc::downgrade(self),
            target,
        }
    }

    /// Ask each viewed device to send its levels, where it has not been asked
    /// since the way to it last changed, or every one with `again`. Cheap
    /// when nothing moved: one version read. Called when the device list
    /// changes, and by the ticker when a playing target has gone quiet.
    pub fn ask(&self, again: bool) {
        let version = crate::remote::devices::version();
        let due: Vec<String> = {
            let inner = self.inner.lock();
            inner
                .views
                .iter()
                .filter(|v| again || v.asked != Some(version))
                .map(|v| v.target.clone())
                .collect()
        };
        for target in due {
            let sent = (self.send)(&target, LinkCommand::WatchLevels { on: true });
            if !sent {
                log::debug!("levels: no way to {target} now; asking when one opens");
            }
            if let Some(v) = self
                .inner
                .lock()
                .views
                .iter_mut()
                .find(|v| v.target == target)
            {
                v.asked = sent.then_some(version);
            }
        }
    }

    /// Frames from `from`, over either path.
    pub fn received(&self, from: &str, frame: Frame) {
        let mut inner = self.inner.lock();
        if inner.drawing.as_deref() != Some(from) || !inner.views.iter().any(|v| v.target == from) {
            return;
        }
        inner
            .interp
            .push(frame.at_ms(), frame.levels(), Instant::now());
        inner.heard = Some(Instant::now());
        self.tick.notify_all();
    }

    /// What to draw now, with the target playing or not.
    pub fn sample(&self, playing: bool) -> VizLevels {
        self.inner.lock().interp.sample(playing, Instant::now())
    }

    /// Draw at `fps`: the display's rate.
    pub fn set_fps(&self, fps: u8) {
        self.inner.lock().interval = Duration::from_micros(1_000_000 / fps.clamp(1, 240) as u64);
    }

    /// Bumped once per display frame while there is something to draw.
    pub fn ticks(&self) -> &Wake {
        &self.ticks
    }

    fn release(&self, target: &str) {
        let mut inner = self.inner.lock();
        let Some(at) = inner.views.iter().position(|v| v.target == target) else {
            return;
        };
        inner.views[at].count -= 1;
        if inner.views[at].count > 0 {
            return;
        }
        inner.views.remove(at);
        if inner.drawing.as_deref() == Some(target) {
            inner.drawing = None;
            inner.interp = Interp::default();
            inner.heard = None;
        }
        drop(inner);
        (self.send)(target, LinkCommand::WatchLevels { on: false });
    }

    fn run_ticks(&self) {
        loop {
            let interval = {
                let mut inner = self.inner.lock();
                loop {
                    if inner.views.is_empty() {
                        self.tick.wait(&mut inner);
                    } else if inner.interp.moving() {
                        break inner.interval;
                    } else if self.tick.wait_for(&mut inner, STALL).timed_out()
                        && inner.heard.is_none_or(|at| at.elapsed() >= STALL)
                        && !inner.views.is_empty()
                    {
                        drop(inner);
                        let playing = crate::remote::devices::target_playhead()
                            .is_some_and(|(_, playing)| playing);
                        if playing {
                            self.ask(true);
                        }
                        inner = self.inner.lock();
                    }
                }
            };
            std::thread::sleep(interval);
            self.ticks.bump();
        }
    }

    #[cfg(test)]
    fn viewers(&self, target: &str) -> usize {
        self.inner
            .lock()
            .views
            .iter()
            .find(|v| v.target == target)
            .map_or(0, |v| v.count)
    }
}

/// A subscription to a device's levels. See `Remote::view`.
pub struct View {
    remote: Weak<Remote>,
    target: String,
}

impl View {
    pub fn target(&self) -> &str {
        &self.target
    }
}

impl Drop for View {
    fn drop(&mut self) {
        if let Some(remote) = self.remote.upgrade() {
            remote.release(&self.target);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `Remote` whose sends are recorded, and answer as `through` says.
    type Sent = Arc<Mutex<Vec<(String, bool)>>>;

    fn recording(through: bool) -> (Arc<Remote>, Sent, Arc<std::sync::atomic::AtomicBool>) {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let open = Arc::new(std::sync::atomic::AtomicBool::new(through));
        let (log, gate) = (Arc::clone(&sent), Arc::clone(&open));
        let remote = Remote::new(move |to, cmd| {
            if let LinkCommand::WatchLevels { on } = cmd {
                log.lock().push((to.to_string(), on));
            }
            gate.load(Ordering::Relaxed)
        });
        (remote, sent, open)
    }

    #[test]
    fn switching_devices_tells_the_old_one_to_stop() {
        let (remote, sent, _) = recording(true);
        let x = remote.view("x".into());
        // The new device viewed before the old one is let go: still per device.
        let y = remote.view("y".into());
        drop(x);
        assert_eq!(remote.viewers("x"), 0);
        assert!(
            sent.lock().contains(&("x".into(), false)),
            "{:?}",
            sent.lock()
        );
        assert!(!sent.lock().contains(&("y".into(), false)));
        drop(y);
        assert!(sent.lock().contains(&("y".into(), false)));
    }

    #[test]
    fn a_device_stays_watched_while_any_view_of_it_is_held() {
        let (remote, sent, _) = recording(true);
        let a = remote.view("x".into());
        let b = remote.view("x".into());
        drop(a);
        assert!(!sent.lock().contains(&("x".into(), false)));
        drop(b);
        assert!(sent.lock().contains(&("x".into(), false)));
    }

    #[test]
    fn an_ask_that_found_no_way_through_is_made_again() {
        let (remote, sent, open) = recording(false);
        let _view = remote.view("x".into());
        let asks = |sent: &Mutex<Vec<(String, bool)>>| {
            sent.lock().iter().filter(|(t, on)| t == "x" && *on).count()
        };
        assert_eq!(asks(&sent), 1);
        // A connection opens: the device list moves, and the stream asks.
        open.store(true, Ordering::Relaxed);
        remote.ask(false);
        assert_eq!(asks(&sent), 2, "asked again, having not got through");
        remote.ask(true);
        assert_eq!(asks(&sent), 3, "and on a renewal");
    }

    #[test]
    fn frames_from_a_device_not_viewed_are_ignored() {
        let (remote, _, _) = recording(true);
        let _view = remote.view("x".into());
        remote.received("y", Frame(100, 500, 500, 500));
        assert!(!remote.inner.lock().interp.moving());
        remote.received("x", Frame(100, 500, 500, 500));
        assert!(remote.inner.lock().interp.moving());
    }

    #[test]
    fn a_short_skip_back_clears_the_buffer() {
        let (mut i, last) = interp(400, 10);
        i.push(150, lv(0.9), last + ms(20));
        assert_eq!(i.frames.len(), 1);
    }

    fn lv(v: f32) -> VizLevels {
        VizLevels {
            low: v,
            mid: v / 2.0,
            high: v / 4.0,
        }
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// Frames every 20 ms from `from`, the level rising one hundredth a frame,
    /// each arriving as it is heard. Returns when the last one arrived.
    fn interp(from: u64, frames: u64) -> (Interp, Instant) {
        let start = Instant::now();
        let mut i = Interp::default();
        for n in 0..frames {
            i.push(from + n * 20, lv(n as f32 / 100.0), start + ms(n * 20));
        }
        (i, start + ms((frames - 1) * 20))
    }

    #[test]
    fn a_frame_round_trips_in_a_few_dozen_bytes() {
        let frame = Frame::new(
            183_456,
            VizLevels {
                low: 0.5126,
                mid: 0.3,
                high: 2.0,
            },
        );
        let json = serde_json::to_string(&frame).unwrap();
        assert_eq!(json, "[183456,513,300,1000]");
        assert_eq!(serde_json::from_str::<Frame>(&json).unwrap(), frame);
        assert!((frame.levels().low - 0.513).abs() < 1e-6);
    }

    #[test]
    fn levels_between_frames_are_interpolated_behind_the_playhead() {
        let (mut i, last) = interp(10_000, 10);
        let delay = i.delay_ms();
        assert!((34..=40).contains(&delay), "about two frames: {delay}");
        // The playhead stands at the last frame, 10_180; drawn `delay` behind.
        let got = i.sample(true, last);
        let want = (180 - delay) as f32 / 20.0 / 100.0;
        assert!((got.low - want).abs() < 1e-4, "{got:?}, wanted {want}");
        assert!((got.high - want / 4.0).abs() < 1e-4);
    }

    #[test]
    fn a_dropped_frame_is_bridged_by_the_delay() {
        let start = Instant::now();
        let mut i = Interp::default();
        for n in [0u64, 1, 2, 4, 5] {
            i.push(n * 20, lv(n as f32 / 10.0), start + ms(n * 20));
        }
        // The frame at 60 never came; 70 lies between 40 and 80.
        let got = i.sample(true, start + ms(70 + i.delay_ms()));
        assert!((got.low - 0.35).abs() < 1e-3, "{got:?}");
    }

    /// The bug this clock fixes: a device whose state report put its playhead
    /// ahead of the music — taken during a rate switch's lead-in, and not
    /// corrected while it stayed within three seconds — had its bars drawn
    /// past every frame it sent, so they never moved. Drawn by the frames'
    /// own clock, they move however the frames are keyed and however late,
    /// within the delay, each arrives.
    #[test]
    fn the_frames_are_the_clock_through_jitter() {
        let start = Instant::now();
        let mut i = Interp::default();
        let late = [0u64, 12, 3, 15, 0, 8, 14, 1, 9, 0, 11, 4];
        let level = VizLevels {
            low: 0.5,
            mid: 0.5,
            high: 0.5,
        };
        // Frames heard every 20 ms and arriving up to 15 ms late, drawn every
        // 5 ms; keyed by a playhead a minute in, nothing like the time here.
        let mut events: Vec<(u64, Option<u64>)> = (0..late.len() as u64)
            .map(|n| (n * 20 + late[n as usize], Some(n)))
            .chain((0..late.len() as u64 * 4).map(|tick| (tick * 5, None)))
            .collect();
        events.sort();
        let mut drawn = 0;
        for (t, push) in events {
            let now = start + ms(t);
            match push {
                Some(n) => i.push(61_000 + n * 20, level, now),
                None => {
                    let got = i.sample(true, now);
                    if t >= 60 {
                        assert!((got.low - 0.5).abs() < 1e-6, "at {t} ms: {got:?}");
                        drawn += 1;
                    }
                }
            }
        }
        assert!(drawn > 30);
    }

    #[test]
    fn a_dry_buffer_eases_to_rest_without_inventing_motion() {
        let (mut i, last) = interp(0, 5);
        // Drawn at the last frame, then on past it.
        let delay = i.delay_ms();
        let first = i.sample(true, last + ms(delay));
        assert!(first.low > 0.0);
        let mut prev = first.low;
        for n in 1..=60u64 {
            let got = i.sample(true, last + ms(delay + n * 20));
            assert!(got.low <= prev, "only ever falls");
            prev = got.low;
        }
        assert!(prev < REST, "at rest: {prev}");
        assert!(!i.moving());
    }

    #[test]
    fn a_paused_device_eases_to_rest() {
        let (mut i, last) = interp(0, 10);
        i.sample(true, last);
        let got = i.sample(false, last + Duration::from_secs(1));
        assert!(got.low < REST);
    }

    #[test]
    fn a_seek_clears_the_buffer() {
        let (mut i, last) = interp(30_000, 10);
        i.push(5_000, lv(0.9), last + ms(20));
        assert_eq!(i.frames.len(), 1, "only the frame after the seek");
        i.push(5_020, lv(0.7), last + ms(40));
        // Drawn at 5_010, halfway between the two.
        let got = i.sample(true, last + ms(30 + i.delay_ms()));
        assert!((got.low - 0.8).abs() < 1e-4, "{got:?}");
    }

    #[test]
    fn a_frame_where_the_playhead_stood_replaces_the_last() {
        let (mut i, last) = interp(0, 3);
        i.push(40, lv(0.0), last + ms(20));
        assert_eq!(i.frames.len(), 3);
        assert_eq!(i.frames.back().unwrap().1, lv(0.0));
    }

    #[test]
    fn nothing_is_sent_with_no_watcher_and_it_stops_on_unsubscribe() {
        let feed = Feed::new();
        let waker = Waker::new().unwrap();
        feed.publish_for_test(Frame(1, 1, 1, 1));
        assert_eq!(feed.watchers(), 0);

        let mut watch = feed.watch(&waker);
        assert_eq!(watch.take(), None, "nothing new since it subscribed");
        feed.publish_for_test(Frame(2, 2, 2, 2));
        assert_eq!(watch.take(), Some(Frame(2, 2, 2, 2)));
        assert_eq!(watch.take(), None, "sent once");
        feed.publish_for_test(Frame(3, 3, 3, 3));
        feed.publish_for_test(Frame(4, 4, 4, 4));
        assert_eq!(
            watch.take(),
            Some(Frame(4, 4, 4, 4)),
            "the newest, not a backlog"
        );

        drop(watch);
        assert_eq!(feed.watchers(), 0);
    }

    #[test]
    fn the_feed_reads_the_analyser_only_while_watched() {
        let feed = Feed::new();
        let viz = VizSnapshot::new();
        feed.provide(Arc::clone(&viz), || 1_234);
        let waker = Waker::new().unwrap();
        let mut watch = feed.watch(&waker);

        let deadline = Instant::now() + Duration::from_secs(5);
        let frame = loop {
            viz.write(Default::default());
            if let Some(frame) = watch.take() {
                break frame;
            }
            assert!(Instant::now() < deadline, "no frame reached the watcher");
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(frame.at_ms(), 1_234);

        // The session goes: its waker with it, and the watch.
        drop(watch);
        drop(waker);
        let reads = viz.reads();
        for _ in 0..5 {
            viz.write(Default::default());
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            viz.reads() <= reads + 1,
            "the pump stopped reading once nobody watched"
        );
    }
}
