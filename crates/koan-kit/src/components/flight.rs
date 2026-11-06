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
//! its source); while it is in the air both ends are drawn hidden and
//! `overlay`, which a window's shell mounts over its pages, draws the
//! picture between the two ends.
//!
//! The picture is a shader quad (`FLIGHT`) whose progress from source to
//! destination is a track the GPU reckons at each present: nothing is laid
//! out while it flies. On its own clock the whole flight is one track; a
//! finger writes the track as it moves (`drive`) and hands the GPU the rest
//! of the way as it lets go (`release`). The ends are taken where they stand
//! at rest: a shell sliding pages tells the flight how far each page stands
//! from its place (`page_offsets`), and the anchors' measures are taken back
//! by that much. Under Reduce Motion, and where the renderer draws no
//! shaders, nothing flies.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use gpui::prelude::FluentBuilder as _;
use gpui::*;

use super::{Animated, CoverArt, Ease, Repeat, SHADED, ShaderProgram};
use crate::theme::KoanTheme;

/// The picture between its two ends. Track 0 the progress, 0 at the source
/// and 1 at the destination; uniforms 0..4 the source's rectangle and 4..8
/// the destination's, as x, y, width and height in device pixels from the
/// quad's corner; 8..12 the colour drawn where there is no picture; 12 one
/// where there is.
static FLIGHT: ShaderProgram = ShaderProgram {
    name: "koan-flight",
    msl: r#"
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    float t = koan_track(u, 0, time);
    float4 rect = mix(float4(u[0], u[1], u[2], u[3]), float4(u[4], u[5], u[6], u[7]), t);
    float2 uv = (position - rect.xy) / rect.zw;
    if (any(uv < 0.0) || any(uv > 1.0)) {
        return float4(0.0);
    }
    if (u[12] > 0.5) {
        float4 c = koan_sample(image, tile, uv);
        return c;
    }
    return float4(u[8], u[9], u[10], u[11]);
}
"#,
    wgsl: r#"
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 20>, tile: vec4<f32>) -> vec4<f32> {
    let t = koan_track(u, 0u, time);
    let rect = mix(u[0], u[1], t);
    let uv = (position - rect.xy) / rect.zw;
    if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0))) {
        return vec4<f32>(0.0);
    }
    if (u[3].x > 0.5) {
        return koan_sample(tile, uv);
    }
    return u[2];
}
"#,
    prepare: None,
};

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

/// Where an anchor stands at rest, as last measured.
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

/// How the flight's progress moves.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Clock {
    /// From `from` to `to` over `length`, set going once both ends are known.
    Timed {
        length: Duration,
        from: f32,
        to: f32,
        ease: Ease,
    },
    /// Where its owner puts it: a finger's swipe.
    Driven,
}

struct Flight {
    key: FlightKey,
    art: Art,
    clock: Clock,
    /// Which flight this is, so a timer set for one does not end the next.
    number: u64,
    /// Where the source stood when the flight was set going, for a tap,
    /// which knows exactly which of several tiles of one record it was.
    from: Option<Bounds<Pixels>>,
    /// The overlay's frame it was set going in.
    frame: u64,
    /// Going back to its source: the destination is the page in front, and
    /// the source must be drawn during the flight to be flown to.
    returning: bool,
    /// Both ends were known when the overlay last drew.
    drawable: bool,
    /// Its clock has been handed to the GPU.
    going: bool,
}

/// Every window's flight and anchors. One flight is in the air at a time:
/// a new one replaces it.
#[derive(Default)]
struct Flights {
    frame: u64,
    flights: u64,
    seen: HashMap<(FlightKey, End), Seen>,
    flight: Option<Flight>,
    /// How far the pages holding each end stand from their places, along
    /// the window's width, while a shell slides them.
    offsets: (Pixels, Pixels),
    /// The progress and the ends' rectangles, drawn by `FLIGHT`.
    animated: Animated,
    /// The view the overlay is drawn in, drawn again as a flight lands.
    overlay: Option<EntityId>,
}

impl Global for Flights {}

impl Flights {
    /// The two ends' bounds at rest, source first, where both are known for
    /// the flight in the air.
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

    /// The views drawing the flight's ends, and the overlay's.
    fn views(&self) -> Vec<EntityId> {
        let Some(flight) = &self.flight else {
            return Vec::new();
        };
        [End::Source, End::Destination]
            .into_iter()
            .filter_map(|end| self.seen.get(&(flight.key, end))?.view)
            .chain(self.overlay)
            .collect()
    }

    /// Whether the anchor at `end` under `key` is drawn hidden.
    fn hides(&self, key: FlightKey, end: End) -> bool {
        let Some(flight) = self.flight.as_ref().filter(|f| f.key == key) else {
            return false;
        };
        // A departing flight's destination is hidden from its first frame:
        // the picture is drawn as soon as the destination is measured.
        (end == End::Destination && !flight.returning) || flight.drawable
    }

    /// Notes where an anchor was laid out, taken back to its page's place.
    fn record(&mut self, key: FlightKey, end: End, mut seen: Seen) {
        if !seen.lasting {
            seen.bounds.origin.x -= match end {
                End::Source => self.offsets.0,
                End::Destination => self.offsets.1,
            };
        }
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

    /// Starts a flight, replacing the one in the air.
    fn begin(&mut self, mut flight: Flight) -> u64 {
        self.flights += 1;
        flight.number = self.flights;
        flight.frame = self.frame;
        self.flight = Some(flight);
        self.flights
    }
}

fn flies(cx: &App) -> bool {
    SHADED && KoanTheme::motion_allowed(cx)
}

fn notify_all(views: Vec<EntityId>, cx: &mut App) {
    for view in views {
        cx.notify(view);
    }
}

/// Ends flight `number` after `wait`, if it is still in the air.
fn land_after(number: u64, wait: Duration, cx: &mut App) {
    cx.spawn(async move |cx| {
        cx.background_executor().timer(wait).await;
        cx.update(|cx| {
            let current = cx
                .try_global::<Flights>()
                .and_then(|f| f.flight.as_ref())
                .map(|f| f.number);
            if current == Some(number) {
                land(cx);
            }
        });
    })
    .detach();
}

/// Sets a flight going from the source under `key` to its destination, as
/// a tap opens the destination's page: `from` is where the tapped picture
/// stood, or the source anchor's last measure. The destination must be drawn
/// within half a second, or nothing flies. It runs for `length` on `ease`
/// from when both ends are known.
pub fn depart(
    key: FlightKey,
    art: &CoverArt,
    from: Option<Bounds<Pixels>>,
    length: Duration,
    ease: Ease,
    cx: &mut App,
) {
    land(cx);
    if !flies(cx) {
        return;
    }
    let Some(art) = Art::of(art) else { return };
    let flights = cx.default_global::<Flights>();
    let Some(from) = from.or_else(|| flights.seen.get(&(key, End::Source)).map(|s| s.bounds))
    else {
        return;
    };
    let number = flights.begin(Flight {
        key,
        art,
        clock: Clock::Timed {
            length,
            from: 0.0,
            to: 1.0,
            ease,
        },
        number: 0,
        from: Some(from),
        frame: 0,
        returning: false,
        drawable: false,
        going: false,
    });
    // Given up if its destination never comes.
    cx.spawn(async move |cx| {
        cx.background_executor().timer(WAIT).await;
        cx.update(|cx| {
            let waiting = cx
                .try_global::<Flights>()
                .and_then(|f| f.flight.as_ref())
                .is_some_and(|f| f.number == number && !f.going);
            if waiting {
                land(cx);
            }
        });
    })
    .detach();
}

/// Sets a flight going back from the destination under `key`, the page in
/// front, to its source: on a clock of `length` and `ease`, or, with none,
/// where `drive` puts it, starting at the destination. A source that is not
/// `lasting` must be drawn during the flight to be flown to; until it is,
/// nothing flies. Whether a flight was set going.
pub fn recall(key: FlightKey, timing: Option<(Duration, Ease)>, cx: &mut App) -> bool {
    land(cx);
    if !flies(cx) {
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
    flights.begin(Flight {
        key,
        art,
        clock: match timing {
            Some((length, ease)) => Clock::Timed {
                length,
                from: 1.0,
                to: 0.0,
                ease,
            },
            None => Clock::Driven,
        },
        number: 0,
        from: None,
        frame: 0,
        returning: true,
        drawable: false,
        going: false,
    });
    flights.animated.hold(0, 1.0);
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

/// The flight in the air stands at `at`, from its source (0) to its
/// destination (1), until it is driven again or released.
pub fn drive(at: f32, cx: &mut App) {
    if !cx.has_global::<Flights>() {
        return;
    }
    let flights = cx.global_mut::<Flights>();
    if let Some(flight) = flights.flight.as_mut() {
        flight.clock = Clock::Driven;
        flights.animated.hold(0, at.clamp(0.0, 1.0));
    }
}

/// The flight in the air, let go: the GPU takes it from where it stands to
/// `to` over `length` on `ease`, and it lands there.
pub fn release(to: f32, length: Duration, ease: Ease, cx: &mut App) {
    if !cx.has_global::<Flights>() {
        return;
    }
    let flights = cx.global_mut::<Flights>();
    let Some(flight) = flights.flight.as_mut() else {
        return;
    };
    let from = flights.animated.at(0, Animated::now());
    flight.clock = Clock::Timed {
        length,
        from,
        to,
        ease,
    };
    flight.going = true;
    let number = flight.number;
    flights.animated.to(0, to, length, ease);
    land_after(number, length, cx);
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

/// How far, along the window's width, the page holding a flight's source
/// and the page holding its destination stand from their places: set by a
/// shell as it slides them, zero at rest. What the anchors measure is taken
/// back by as much, so a flight runs between where the pictures will rest.
pub fn page_offsets(source: Pixels, destination: Pixels, cx: &mut App) {
    cx.default_global::<Flights>().offsets = (source, destination);
}

/// `bounds` as the program's rectangle: device pixels from `origin`.
fn rect(bounds: Bounds<Pixels>, origin: Point<Pixels>, scale: f32) -> [f32; 4] {
    [
        f32::from(bounds.origin.x - origin.x) * scale,
        f32::from(bounds.origin.y - origin.y) * scale,
        f32::from(bounds.size.width) * scale,
        f32::from(bounds.size.height) * scale,
    ]
}

/// What a window's shell mounts over its pages, under its sheets: the
/// picture in flight, while one is.
pub fn overlay() -> impl IntoElement {
    canvas(
        |_, window, cx| {
            let animated = cx.default_global::<Flights>().animated.clone();
            animated.attach(false, window, cx);
        },
        |bounds, _, window, cx| paint(bounds, window, cx),
    )
    .absolute()
    .inset_0()
}

fn paint(bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
    let view = window.current_view();
    let flights = cx.default_global::<Flights>();
    flights.frame += 1;
    flights.overlay = Some(view);
    let Some(flight) = flights.flight.as_ref() else {
        return;
    };
    let was = flight.drawable;
    let Some((source, destination)) = flights.ends() else {
        if was {
            if let Some(flight) = flights.flight.as_mut() {
                flight.drawable = false;
            }
            let views = flights.views();
            window.on_next_frame(move |_, cx| notify_all(views, cx));
        }
        return;
    };
    let scale = window.scale_factor();
    let animated = flights.animated.clone();
    let mut ends = [0.0; 8];
    ends[0..4].copy_from_slice(&rect(source, bounds.origin, scale));
    ends[4..8].copy_from_slice(&rect(destination, bounds.origin, scale));
    animated.set_all(0, &ends);
    let Some(flight) = flights.flight.as_mut() else {
        return;
    };
    let image = match &flight.art {
        Art::Image(image) => {
            animated.set(12, 1.0);
            Some(image.clone())
        }
        Art::Colour(colour) => {
            animated.set_color(8, *colour);
            animated.set(12, 0.0);
            None
        }
    };
    flight.drawable = true;
    // A flight on its own clock is handed to the GPU once, as both ends
    // are first known, and lands by a timer: no frame is drawn while it
    // flies.
    if let Clock::Timed {
        length,
        from,
        to,
        ease,
    } = flight.clock
        && !flight.going
    {
        flight.going = true;
        animated.segment(0, from, to, Animated::now(), length, ease, Repeat::Once);
        land_after(flight.number, length, cx);
    }
    if !was {
        let views = cx.global::<Flights>().views();
        window.on_next_frame(move |_, cx| notify_all(views, cx));
    }
    animated.paint(&FLIGHT, image, bounds, window);
}

/// One end of a flight around its picture: measured each time it is drawn,
/// and drawn hidden while the picture flies in its place.
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

    fn flight(key: FlightKey, from: Option<Bounds<Pixels>>, returning: bool) -> Flight {
        Flight {
            key,
            art: Art::Colour(gpui::black()),
            clock: Clock::Driven,
            number: 0,
            from,
            frame: 0,
            returning,
            drawable: false,
            going: false,
        }
    }

    const KEY: FlightKey = FlightKey("sleeve", 7);

    #[test]
    fn a_rectangle_is_in_device_pixels_from_the_quad() {
        assert_eq!(
            rect(b(30., 40., 10.), point(px(10.), px(20.)), 2.0),
            [40., 40., 20., 20.]
        );
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
        flights.begin(flight(KEY, Some(b(10., 10., 40.)), false));
        // The header drawn before the tap is another visit's.
        assert!(flights.ends().is_none());
        assert!(flights.hides(KEY, End::Destination));
        assert!(!flights.hides(KEY, End::Source));
        assert!(!flights.hides(FlightKey("sleeve", 8), End::Destination));
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
        flights.begin(flight(KEY, None, true));
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
    fn ends_are_taken_back_to_their_pages_places() {
        let mut flights = Flights {
            offsets: (px(-30.), px(300.)),
            ..Default::default()
        };
        flights.record(KEY, End::Source, seen(b(70., 10., 40.), 0, false));
        flights.record(KEY, End::Destination, seen(b(316., 0., 200.), 0, false));
        flights.record(
            FlightKey("mini", 0),
            End::Source,
            seen(b(16., 800., 30.), 0, true),
        );
        assert_eq!(flights.seen[&(KEY, End::Source)].bounds, b(100., 10., 40.));
        assert_eq!(
            flights.seen[&(KEY, End::Destination)].bounds,
            b(16., 0., 200.)
        );
        assert_eq!(
            flights.seen[&(FlightKey("mini", 0), End::Source)].bounds,
            b(16., 800., 30.)
        );
    }

    #[test]
    fn a_new_flight_replaces_the_one_in_the_air() {
        let mut flights = Flights::default();
        let first = flights.begin(flight(KEY, Some(b(0., 0., 1.)), false));
        let second = flights.begin(flight(FlightKey("sleeve", 8), Some(b(0., 0., 1.)), false));
        assert_ne!(first, second);
        assert_eq!(flights.flight.as_ref().map(|f| f.number), Some(second));
        assert!(!flights.hides(KEY, End::Destination));
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
