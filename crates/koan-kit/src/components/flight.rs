//! Shared-element flights: a picture that moves from where it sat on one
//! page to where it sits on the next, as a record's sleeve grows from its
//! tile into its page's header, or the mini player's sleeve into Now
//! Playing's art.
//!
//! Each end is a `FlightAnchor` around the picture, under a `FlightKey`
//! both ends share: a `source` (the tile, the mini player) and a
//! `destination` (the header, Now Playing). An anchor measures where it was
//! laid out each time it is drawn. A flight is set going by `depart` (a tap
//! that opens the destination) or `recall` (the destination going back to
//! its source); while it is in the air a copy of the picture is drawn by
//! `overlay`, which a window's shell mounts over its pages, between the two
//! ends' bounds, and both ends are drawn hidden. A flight runs on a clock of
//! its own or follows a progress its owner hands it (`drive`), as a swipe
//! back does, and ends at its clock's end or by `land`.
//!
//! The copy is laid out on the CPU each frame, for the length of the flight
//! only. Under Reduce Motion nothing flies.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::prelude::FluentBuilder as _;
use gpui::*;

use super::CoverArt;
use crate::theme::KoanTheme;
use crate::theme::tokens::Motion;

/// What both ends of a flight are drawn under: a kind of picture and whose
/// it is, such as a record's sleeve and the record's id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FlightKey(pub &'static str, pub u64);

/// Which end of a flight an anchor is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum End {
    /// Where the picture flies from as its page opens: a tile, the mini
    /// player.
    Source,
    /// Where it lands: the page's header, Now Playing's art.
    Destination,
}

/// The curve a flight on its own clock takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Curve {
    QuickOut,
    InOut,
}

impl Curve {
    fn at(self, t: f32) -> f32 {
        match self {
            Curve::QuickOut => Motion::quick_out(t),
            Curve::InOut => Motion::ease_in_out(t),
        }
    }
}

/// How long a departed flight waits for its destination to be drawn before
/// it is given up: the tap may not have opened the page it names.
const WAIT: Duration = Duration::from_millis(500);
/// How many destinations are remembered, each holding its picture.
const DESTINATIONS: usize = 4;
/// How many anchors are remembered before those not drawn lately go.
const SEEN: usize = 256;

/// The picture a flight carries.
#[derive(Clone)]
enum Art {
    Image(Arc<RenderImage>),
    Colour(Hsla),
}

impl Art {
    fn of(art: &CoverArt) -> Option<Self> {
        match art {
            CoverArt::Image(image) => Some(Art::Image(image.clone())),
            CoverArt::Colour(colour) => Some(Art::Colour(*colour)),
            CoverArt::Loading | CoverArt::Missing => None,
        }
    }
}

/// Where an anchor was last laid out.
#[derive(Clone)]
struct Seen {
    bounds: Bounds<Pixels>,
    /// The overlay's frame it was laid out in.
    frame: u64,
    /// The view it is drawn in, drawn again as a flight hides or shows it.
    view: Option<EntityId>,
    /// It stays where it was drawn (the mini player in the foot), so its
    /// last measure holds even while its view is not drawn again.
    lasting: bool,
    /// A destination's picture, for a flight back from it.
    art: Option<Art>,
}

/// A flight's progress from its source (0) to its destination (1).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Clock {
    /// On a clock of its own, from `from` to `to`, started once both ends
    /// were first known.
    Timed {
        started: Option<Instant>,
        length: Duration,
        from: f32,
        to: f32,
        curve: Curve,
    },
    /// Following its owner: a finger's swipe.
    Driven(f32),
}

impl Clock {
    /// Where the flight stands at `now`, and whether it has ended.
    fn at(&self, now: Instant) -> (f32, bool) {
        match *self {
            Clock::Driven(at) => (at, false),
            Clock::Timed {
                started: None,
                from,
                ..
            } => (from, false),
            Clock::Timed {
                started: Some(started),
                length,
                from,
                to,
                curve,
            } => {
                let t = if length.is_zero() {
                    1.0
                } else {
                    now.saturating_duration_since(started).as_secs_f32() / length.as_secs_f32()
                };
                if t >= 1.0 {
                    return (to, true);
                }
                (from + (to - from) * curve.at(t), false)
            }
        }
    }
}

struct Flight {
    key: FlightKey,
    art: Art,
    clock: Clock,
    /// Where the source stood when the flight was set going, for a tap,
    /// which knows exactly which of several tiles of one record it was.
    from: Option<Bounds<Pixels>>,
    /// The overlay's frame it was set going in.
    frame: u64,
    began: Instant,
    /// Going back to its source: the destination is the page in front, and
    /// the source must be drawn during the flight to be flown to.
    returning: bool,
    /// Both ends were known when the overlay last drew.
    drawable: bool,
}

/// Every window's flight and anchors. One flight is in the air at a time:
/// a new one replaces it.
#[derive(Default)]
struct Flights {
    frame: u64,
    seen: HashMap<(FlightKey, End), Seen>,
    flight: Option<Flight>,
}

impl Global for Flights {}

impl Flights {
    /// The two ends' bounds, source first, where both are known for the
    /// flight in the air.
    fn ends(&self) -> Option<(Bounds<Pixels>, Bounds<Pixels>)> {
        let flight = self.flight.as_ref()?;
        let source = match flight.from {
            Some(from) => from,
            None => {
                let seen = self.seen.get(&(flight.key, End::Source))?;
                if flight.returning && !seen.lasting && seen.frame < flight.frame {
                    return None;
                }
                seen.bounds
            }
        };
        let destination = self.seen.get(&(flight.key, End::Destination))?;
        if !flight.returning && destination.frame < flight.frame {
            return None;
        }
        Some((source, destination.bounds))
    }

    /// The views drawing the flight's ends.
    fn views(&self) -> Vec<EntityId> {
        let Some(flight) = &self.flight else {
            return Vec::new();
        };
        [End::Source, End::Destination]
            .into_iter()
            .filter_map(|end| self.seen.get(&(flight.key, end))?.view)
            .collect()
    }

    /// Whether the anchor at `end` under `key` is drawn hidden.
    fn hides(&self, key: FlightKey, end: End) -> bool {
        let Some(flight) = self.flight.as_ref().filter(|f| f.key == key) else {
            return false;
        };
        // A departing flight's destination is hidden from its first frame:
        // the copy is drawn as soon as the destination is measured.
        (end == End::Destination && !flight.returning) || flight.drawable
    }

    fn record(&mut self, key: FlightKey, end: End, seen: Seen) {
        self.seen.insert((key, end), seen);
        if end == End::Destination {
            let mut destinations: Vec<(u64, FlightKey)> = self
                .seen
                .iter()
                .filter(|((_, e), _)| *e == End::Destination)
                .map(|((k, _), s)| (s.frame, *k))
                .collect();
            if destinations.len() > DESTINATIONS {
                destinations.sort_by_key(|(frame, _)| std::cmp::Reverse(*frame));
                for (_, k) in destinations.into_iter().skip(DESTINATIONS) {
                    if self.flight.as_ref().is_none_or(|f| f.key != k) {
                        self.seen.remove(&(k, End::Destination));
                    }
                }
            }
        }
        if self.seen.len() > SEEN {
            let frame = self.frame;
            let flying = self.flight.as_ref().map(|f| f.key);
            self.seen.retain(|(k, e), s| {
                *e == End::Destination || s.lasting || s.frame >= frame || Some(*k) == flying
            });
        }
    }
}

fn notify_all(views: Vec<EntityId>, cx: &mut App) {
    for view in views {
        cx.notify(view);
    }
}

/// Sets a flight going from the source under `key` to its destination, as
/// a tap opens the destination's page: `from` is where the tapped picture
/// stood, or the source anchor's last measure. The destination must be drawn
/// within half a second, or nothing flies. It runs for `length` once both
/// ends are known.
pub fn depart(
    key: FlightKey,
    art: &CoverArt,
    from: Option<Bounds<Pixels>>,
    length: Duration,
    curve: Curve,
    cx: &mut App,
) {
    land(cx);
    if !KoanTheme::motion_allowed(cx) {
        return;
    }
    let Some(art) = Art::of(art) else { return };
    let flights = cx.default_global::<Flights>();
    let Some(from) = from.or_else(|| flights.seen.get(&(key, End::Source)).map(|s| s.bounds))
    else {
        return;
    };
    flights.flight = Some(Flight {
        key,
        art,
        clock: Clock::Timed {
            started: None,
            length,
            from: 0.0,
            to: 1.0,
            curve,
        },
        from: Some(from),
        frame: flights.frame,
        began: Instant::now(),
        returning: false,
        drawable: false,
    });
}

/// Sets a flight going back from the destination under `key`, the page in
/// front, to its source: on a clock of `length` and `curve`, or, with none,
/// following `drive` from where the destination stands. A source that is
/// not `lasting` must be drawn during the flight to be flown to; until it is,
/// nothing flies. Whether a flight was set going.
pub fn recall(key: FlightKey, timing: Option<(Duration, Curve)>, cx: &mut App) -> bool {
    land(cx);
    if !KoanTheme::motion_allowed(cx) {
        return false;
    }
    let flights = cx.default_global::<Flights>();
    let Some(art) = flights
        .seen
        .get(&(key, End::Destination))
        .and_then(|s| s.art.clone())
    else {
        return false;
    };
    flights.flight = Some(Flight {
        key,
        art,
        clock: match timing {
            Some((length, curve)) => Clock::Timed {
                started: None,
                length,
                from: 1.0,
                to: 0.0,
                curve,
            },
            None => Clock::Driven(1.0),
        },
        from: None,
        frame: flights.frame,
        began: Instant::now(),
        returning: true,
        drawable: false,
    });
    // Both ends may be known already, as a lasting source is: they hide at
    // once rather than a frame late.
    if flights.ends().is_some() {
        if let Some(flight) = flights.flight.as_mut() {
            flight.drawable = true;
        }
        let views = flights.views();
        notify_all(views, cx);
    }
    true
}

/// The flight in the air follows `at`, from its source (0) to its
/// destination (1), from now on.
pub fn drive(at: f32, cx: &mut App) {
    if !cx.has_global::<Flights>() {
        return;
    }
    if let Some(flight) = cx.global_mut::<Flights>().flight.as_mut() {
        flight.clock = Clock::Driven(at.clamp(0.0, 1.0));
    }
}

/// Ends the flight in the air, if any: both ends show again.
pub fn land(cx: &mut App) {
    if !cx.has_global::<Flights>() {
        return;
    }
    let flights = cx.global_mut::<Flights>();
    let views = flights.views();
    if flights.flight.take().is_some() {
        notify_all(views, cx);
    }
}

/// The key of the flight in the air.
pub fn flying(cx: &App) -> Option<FlightKey> {
    cx.try_global::<Flights>()?.flight.as_ref().map(|f| f.key)
}

/// `a` moved `t` of the way to `b`, its size with it.
pub fn between(a: Bounds<Pixels>, b: Bounds<Pixels>, t: f32) -> Bounds<Pixels> {
    let mix = |x: Pixels, y: Pixels| x + (y - x) * t;
    Bounds {
        origin: point(mix(a.origin.x, b.origin.x), mix(a.origin.y, b.origin.y)),
        size: size(
            mix(a.size.width, b.size.width),
            mix(a.size.height, b.size.height),
        ),
    }
}

/// Where a picture of `image` size is drawn to fill `bounds`, its middle
/// kept, as a cover's is.
fn filled(image: Size<f32>, bounds: Bounds<Pixels>) -> Bounds<Pixels> {
    if image.width <= 0.0 || image.height <= 0.0 {
        return bounds;
    }
    let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    let scale = (w / image.width).max(h / image.height);
    let drawn = size(px(image.width * scale), px(image.height * scale));
    Bounds {
        origin: point(
            bounds.origin.x + (bounds.size.width - drawn.width) / 2.,
            bounds.origin.y + (bounds.size.height - drawn.height) / 2.,
        ),
        size: drawn,
    }
}

/// What a window's shell mounts over its pages, under its sheets: the copy
/// in flight, while one is.
pub fn overlay() -> impl IntoElement {
    canvas(|_, _, _| {}, |_, _, window, cx| paint(window, cx))
        .absolute()
        .inset_0()
}

fn paint(window: &mut Window, cx: &mut App) {
    let flights = cx.default_global::<Flights>();
    flights.frame += 1;
    let Some(flight) = flights.flight.as_ref() else {
        return;
    };
    let ends = flights.ends();
    let Some((source, destination)) = ends else {
        let gone = !flight.returning && flight.began.elapsed() > WAIT;
        let was = flight.drawable;
        let views = flights.views();
        if gone {
            flights.flight = None;
        } else if let Some(flight) = flights.flight.as_mut() {
            flight.drawable = false;
        }
        if gone || was {
            window.on_next_frame(move |_, cx| notify_all(views, cx));
        }
        return;
    };
    let now = Instant::now();
    let Some(flight) = flights.flight.as_mut() else {
        return;
    };
    if let Clock::Timed { started, .. } = &mut flight.clock
        && started.is_none()
    {
        *started = Some(now);
    }
    let (at, ended) = flight.clock.at(now);
    let art = flight.art.clone();
    let shows = !flight.drawable;
    flight.drawable = true;
    let timed = matches!(flight.clock, Clock::Timed { .. });
    if ended || shows {
        let views = flights.views();
        if ended {
            flights.flight = None;
        }
        window.on_next_frame(move |_, cx| notify_all(views, cx));
    }
    if timed && !ended {
        window.request_animation_frame();
    }
    let bounds = between(source, destination, at);
    match art {
        Art::Image(image) => {
            let size = image.size(0);
            let picture = filled(
                gpui::size(size.width.0 as f32, size.height.0 as f32),
                bounds,
            );
            window
                .paint_image(bounds, picture, Corners::default(), image, 0, false)
                .ok();
        }
        Art::Colour(colour) => window.paint_quad(fill(bounds, colour)),
    }
}

/// One end of a flight around its picture: measured each time it is drawn,
/// and drawn hidden while the copy flies in its place.
#[derive(IntoElement)]
pub struct FlightAnchor {
    key: FlightKey,
    end: End,
    lasting: bool,
    art: Option<CoverArt>,
    child: AnyElement,
}

impl FlightAnchor {
    pub fn source(key: FlightKey, child: impl IntoElement) -> Self {
        Self {
            key,
            end: End::Source,
            lasting: false,
            art: None,
            child: child.into_any_element(),
        }
    }

    /// `art` is the picture a flight back from here carries.
    pub fn destination(key: FlightKey, art: CoverArt, child: impl IntoElement) -> Self {
        Self {
            key,
            end: End::Destination,
            lasting: false,
            art: Some(art),
            child: child.into_any_element(),
        }
    }

    /// It stays where it is drawn, as the mini player does in the foot, so
    /// a flight back may land on it without its being drawn again.
    pub fn lasting(mut self) -> Self {
        self.lasting = true;
        self
    }
}

impl RenderOnce for FlightAnchor {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let (key, end, lasting) = (self.key, self.end, self.lasting);
        let hidden = cx
            .try_global::<Flights>()
            .is_some_and(|f| f.hides(key, end));
        let art = self.art.as_ref().and_then(Art::of);
        div()
            .relative()
            .flex_none()
            .child(div().when(hidden, |d| d.opacity(0.)).child(self.child))
            .child(
                canvas(
                    move |bounds, window, cx| {
                        let view = Some(window.current_view());
                        let flights = cx.default_global::<Flights>();
                        let frame = flights.frame;
                        flights.record(
                            key,
                            end,
                            Seen {
                                bounds,
                                frame,
                                view,
                                lasting,
                                art,
                            },
                        );
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    fn b(x: f32, y: f32, side: f32) -> Bounds<Pixels> {
        Bounds {
            origin: point(px(x), px(y)),
            size: size(px(side), px(side)),
        }
    }

    fn seen(bounds: Bounds<Pixels>, frame: u64, lasting: bool) -> Seen {
        Seen {
            bounds,
            frame,
            view: None,
            lasting,
            art: Some(Art::Colour(gpui::black())),
        }
    }

    fn flight(key: FlightKey, from: Option<Bounds<Pixels>>, frame: u64, returning: bool) -> Flight {
        Flight {
            key,
            art: Art::Colour(gpui::black()),
            clock: Clock::Driven(0.0),
            from,
            frame,
            began: Instant::now(),
            returning,
            drawable: false,
        }
    }

    const KEY: FlightKey = FlightKey("sleeve", 7);

    #[test]
    fn between_moves_and_grows() {
        let (a, z) = (b(0., 100., 40.), b(100., 0., 240.));
        assert_eq!(between(a, z, 0.0), a);
        assert_eq!(between(a, z, 1.0), z);
        assert_eq!(between(a, z, 0.5), b(50., 50., 140.));
    }

    #[test]
    fn a_picture_fills_its_bounds_keeping_its_middle() {
        let wide = filled(size(200., 100.), b(0., 0., 50.));
        assert_eq!(wide.size, size(px(100.), px(50.)));
        assert_eq!(wide.origin, point(px(-25.), px(0.)));
        assert_eq!(filled(size(10., 10.), b(5., 5., 20.)), b(5., 5., 20.));
    }

    #[test]
    fn a_timed_clock_waits_for_its_start_then_eases_to_its_end() {
        let now = Instant::now();
        let mut clock = Clock::Timed {
            started: None,
            length: Duration::from_millis(100),
            from: 1.0,
            to: 0.0,
            curve: Curve::InOut,
        };
        assert_eq!(clock.at(now), (1.0, false));
        if let Clock::Timed { started, .. } = &mut clock {
            *started = Some(now);
        }
        let (half, ended) = clock.at(now + Duration::from_millis(50));
        assert!(!ended && (half - 0.5).abs() < 0.01);
        assert_eq!(clock.at(now + Duration::from_millis(100)), (0.0, true));
        assert_eq!(Clock::Driven(0.3).at(now), (0.3, false));
    }

    #[test]
    fn a_departure_waits_for_its_destination_drawn_after_it() {
        let mut flights = Flights {
            frame: 5,
            ..Default::default()
        };
        flights
            .seen
            .insert((KEY, End::Destination), seen(b(0., 0., 200.), 2, false));
        flights.flight = Some(flight(KEY, Some(b(10., 10., 40.)), 5, false));
        // The header drawn before the tap is another visit's.
        assert!(flights.ends().is_none());
        assert!(flights.hides(KEY, End::Destination));
        assert!(!flights.hides(KEY, End::Source));
        flights
            .seen
            .insert((KEY, End::Destination), seen(b(0., 0., 200.), 5, false));
        assert_eq!(flights.ends(), Some((b(10., 10., 40.), b(0., 0., 200.))));
    }

    #[test]
    fn a_return_flies_only_to_a_source_drawn_during_it() {
        let mut flights = Flights {
            frame: 9,
            ..Default::default()
        };
        flights
            .seen
            .insert((KEY, End::Destination), seen(b(0., 0., 200.), 1, false));
        flights
            .seen
            .insert((KEY, End::Source), seen(b(10., 10., 40.), 3, false));
        flights.flight = Some(flight(KEY, None, 9, true));
        assert!(flights.ends().is_none());
        assert!(!flights.hides(KEY, End::Destination));
        flights
            .seen
            .insert((KEY, End::Source), seen(b(12., 10., 40.), 9, false));
        assert_eq!(flights.ends(), Some((b(12., 10., 40.), b(0., 0., 200.))));
        // A lasting source holds where it was last drawn.
        flights
            .seen
            .insert((KEY, End::Source), seen(b(1., 1., 30.), 0, true));
        assert_eq!(flights.ends(), Some((b(1., 1., 30.), b(0., 0., 200.))));
    }

    #[test]
    fn only_the_latest_destinations_are_kept() {
        let mut flights = Flights::default();
        for id in 0..10u64 {
            flights.frame = id;
            flights.record(
                FlightKey("sleeve", id),
                End::Destination,
                seen(b(0., 0., 1.), id, false),
            );
        }
        let mut kept: Vec<u64> = flights.seen.keys().map(|(k, _)| k.1).collect();
        kept.sort();
        assert_eq!(kept, vec![6, 7, 8, 9]);
    }

    #[test]
    fn anchors_not_drawn_lately_go_once_there_are_many() {
        let mut flights = Flights::default();
        flights.record(
            FlightKey("mini", 0),
            End::Source,
            seen(b(0., 0., 1.), 0, true),
        );
        for id in 0..300u64 {
            flights.frame = id / 100;
            flights.record(
                FlightKey("sleeve", id),
                End::Source,
                seen(b(0., 0., 1.), id / 100, false),
            );
        }
        assert!(flights.seen.len() <= SEEN + 1);
        assert!(
            flights
                .seen
                .contains_key(&(FlightKey("mini", 0), End::Source))
        );
        assert!(
            flights
                .seen
                .contains_key(&(FlightKey("sleeve", 299), End::Source))
        );
    }
}
