//! A window's root: the wash beneath and the content over it, as two cached
//! views (see ARCHITECTURE.md), so a frame of the drift never lays the page
//! out again.
//!
//! The root also follows the playing record: its sleeve, fetched and decoded
//! off the main thread, sets the accent and the wash, and the wash drifts
//! while something plays.
//!
//! While the rainbow is drawn (`crate::rainbow`) the root also lays its
//! prism out: a layer between the wash and the page, the beat's pulse at the
//! edges and the mirror ball over it, and the confetti and the sweep between
//! records over everything. None of it is mounted otherwise.

use std::sync::Arc;

use crate::theme::wash::{Sleeve, Wash};
use crate::theme::{self, Accent, KoanTheme, Look};
use koan_ffi::PlayState;
use koan_kit::app::*;
use koan_kit::components::{Burst, Prism, Prisms, SHADED};

use crate::engine::engine;
use crate::mirror::{Mirror, Playback};

/// The side the sleeve is fetched and analysed at: enough for the accent's
/// hue and the wash's blur, which is baked smaller still.
const SAMPLE: u32 = 128;

/// The bursts, as `WindowRoot::flying` names them.
const CONFETTI: usize = 0;
const SWEEP: usize = 1;
const STICKER: usize = 2;

/// The mirror ball's side, and where it hangs: under the toolbar, in the
/// window's top right corner, in points.
const MIRROR_BALL: f32 = 72.0;
const MIRROR_BALL_TOP: f32 = 56.0;
const MIRROR_BALL_RIGHT: f32 = 18.0;

pub struct WindowRoot {
    wash: Entity<Wash>,
    content: AnyView,
    /// The record whose sleeve the wash and accent show.
    album: Option<i64>,
    /// "Colours from the record" and the theme tint, as last applied.
    record_colours: bool,
    /// Less transparency asked for: the window's ground in place of the wash.
    opaque: bool,
    wash_tint: u32,
    /// The wash's pace and turn as last drawn.
    drift: (u32, u32),
    /// The sleeve on its way; replacing it drops the one before.
    fetching: Option<Task<()>>,
    /// The wash shows the rainbow's sheen.
    rainbow: bool,
    /// The rainbow as the theme last said: drawn, and in brat's palette.
    painted: Option<bool>,
    /// The rainbow on the GPU, and whether it is fed and running.
    prism: Prism,
    prism_fed: bool,
    confetti: Burst,
    sweep: Burst,
    sticker: Burst,
    /// The record last seen playing while the rainbow was drawn: the sweep
    /// crosses the window when another one starts.
    swept: Option<i64>,
    /// Something is playing: the prism moves only then.
    playing: bool,
    /// Each burst's quad is mounted while it flies, and let go after.
    landings: [Option<Task<()>>; 3],
    /// This window is in front, and shows.
    active: bool,
    visible: bool,
    window: WindowId,
    /// The window's size and scale at its last render, where bursts are
    /// thrown from.
    viewport: Size<Pixels>,
    scale: f32,
    _subscriptions: Vec<Subscription>,
    /// The playing bars moved: painted again over the cached content.
    /// This window's bars, where they are quads, and whether it shows.
    _levels: [Option<Subscription>; 2],
}

/// The record a page is showing, whose colours the window takes over the
/// playing record's while it shows: the accent and the wash ease to it and
/// back, as they do when the record playing changes.
#[derive(Default)]
pub struct ShownRecord(pub Option<i64>);

impl Global for ShownRecord {}

impl WindowRoot {
    pub fn new(content: impl Into<AnyView>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let playback = cx.global::<Mirror>().playback.clone();
        let theme = cx.global::<KoanTheme>();
        let (record_colours, wash_tint) = (theme.record_colours, theme.wash_tint);
        let drift = (theme.wash_drift, theme.wash_turn);
        let followed = playback.clone();
        let shown = playback.clone();
        let prism = Prism::new();
        let mut root = Self {
            wash: cx.new(|_| Wash::new()),
            content: content.into(),
            album: None,
            record_colours,
            opaque: KoanTheme::opaque(cx),
            wash_tint,
            drift,
            fetching: None,
            rainbow: false,
            painted: KoanTheme::prism(cx),
            prism_fed: false,
            // One flash gate for the window: the beat and every burst start
            // at most three times a second between them.
            confetti: Burst::confetti().sharing(&prism.gate()),
            sweep: Burst::sweep().sharing(&prism.gate()),
            sticker: Burst::sticker().sharing(&prism.gate()),
            prism,
            swept: None,
            playing: false,
            landings: [None, None, None],
            active: window.is_window_active(),
            visible: true,
            window: window.window_handle().window_id(),
            viewport: window.viewport_size(),
            scale: window.scale_factor(),
            _subscriptions: vec![
                cx.observe_global::<ShownRecord>(move |this, cx| this.follow(&shown, false, cx)),
                theme::follow_appearance(window, cx),
                cx.observe(&playback, |this, playback, cx| {
                    this.follow(&playback, false, cx)
                }),
                cx.observe_global::<KoanTheme>(move |this, cx| this.restyle(&followed, cx)),
                // Its accent cycles only while a window is in front.
                cx.observe_window_activation(window, |this, window, cx| {
                    let active = cx.active_window().is_some();
                    crate::rainbow::set_active(active, cx);
                    this.active = window.is_window_active();
                    this.feed_prism(cx);
                }),
                // A heart favourited: confetti from the pointer, in the
                // window in front.
                cx.observe_global_in::<crate::rainbow::Celebration>(window, |this, window, cx| {
                    if window.is_window_active() && KoanTheme::motion_allowed(cx) {
                        this.throw_confetti(Some(window.mouse_position()), cx);
                    }
                }),
                // A window closed takes its prism from the analyser's
                // readers, which would otherwise keep it running.
                cx.on_release(|this, cx| {
                    if this.prism_fed {
                        crate::levels::prism(this.window, None, cx);
                    }
                    Prisms::set(this.window, None, cx);
                }),
            ],
            _levels: [
                crate::levels::feed(window.window_handle().window_id(), cx)
                    .map(|feed| cx.observe(&feed, |_, _, cx| cx.notify())),
                // A hidden window's bars leave the analyser at rest.
                Some(
                    cx.observe_window_visibility(window, |this, visibility, window, cx| {
                        let visible = visibility == WindowVisibility::Visible;
                        crate::levels::visibility(window.window_handle().window_id(), visible, cx);
                        this.visible = visible;
                        this.feed_prism(cx);
                    }),
                ),
            ],
        };
        root.wash
            .update(cx, |wash, cx| wash.set_hold(wash_tint, cx));
        // The sheens and gradients drawn in this window run with its prism.
        Prisms::set(root.window, Some(root.prism.clone()), cx);
        root.follow(&playback, false, cx);
        root.feed_prism(cx);
        root
    }

    /// Burst `which` was fired: mounted from now, until it has flown.
    fn flying(&mut self, which: usize, cx: &mut Context<Self>) {
        let length = self.bursts()[which].length();
        self.landings[which] = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(length).await;
            this.update(cx, |_, cx| cx.notify()).ok();
        }));
        cx.notify();
    }

    fn bursts(&self) -> [&Burst; 3] {
        [&self.confetti, &self.sweep, &self.sticker]
    }

    /// Feeds the prism the analyser and runs its clock while the rainbow is
    /// drawn and this window is in front and shows; otherwise neither. Each
    /// frame of levels fed wakes the window, so a window behind another
    /// app's is not fed: it stands as it was.
    fn feed_prism(&mut self, cx: &mut Context<Self>) {
        // Under Reduce Motion or Low Power nothing of it is drawn, so the
        // analyser is not read for it and its clock stands.
        let drawn = self.rainbow && SHADED && KoanTheme::motion_allowed(cx);
        // Paused or stopped, the music gives it nothing to move with: it
        // holds its frame, and presents nothing.
        let running = drawn && self.active && self.visible && self.playing;
        self.prism.run(running);
        if running != self.prism_fed {
            self.prism_fed = running;
            crate::levels::prism(self.window, running.then(|| self.prism.clone()), cx);
        }
    }

    /// Confetti from `at`, or from the middle of the window.
    fn throw_confetti(&mut self, at: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        let middle = Point::new(self.viewport.width / 2.0, self.viewport.height / 2.0);
        let brat = crate::theme::KoanTheme::prism_now() == Some(true);
        self.confetti.fire(at.unwrap_or(middle), self.scale, brat);
        self.flying(CONFETTI, cx);
    }

    /// Settings changed the record's colours or the tint, or the rainbow
    /// came out or went: the wash follows.
    fn restyle(&mut self, playback: &Entity<Playback>, cx: &mut Context<Self>) {
        // The rainbow drawn or put away: its sheen, or the record's sleeve
        // and accent back.
        let painted = KoanTheme::prism(cx);
        if painted != self.painted {
            self.painted = painted;
            let was = self.rainbow;
            self.follow(playback, true, cx);
            // Coming out: confetti and a sticker in the middle of the window.
            if self.rainbow && !was && self.active && KoanTheme::motion_allowed(cx) {
                self.throw_confetti(None, cx);
                let middle = Point::new(self.viewport.width / 2.0, self.viewport.height / 2.0);
                self.sticker.fire(middle, self.scale, painted == Some(true));
                self.flying(STICKER, cx);
            }
            self.feed_prism(cx);
            cx.notify();
        }
        // Reduce Motion or Low Power may have moved: the prism's layers come
        // or go with them.
        if self.rainbow {
            self.feed_prism(cx);
            cx.notify();
        }
        let theme = cx.global::<KoanTheme>();
        let (record_colours, wash_tint) = (theme.record_colours, theme.wash_tint);
        let drift = (theme.wash_drift, theme.wash_turn);
        if drift != self.drift {
            self.drift = drift;
            self.wash.update(cx, |_, cx| cx.notify());
        }
        if wash_tint != self.wash_tint {
            self.wash_tint = wash_tint;
            self.wash
                .update(cx, |wash, cx| wash.set_hold(wash_tint, cx));
        }
        let opaque = KoanTheme::opaque(cx);
        if opaque != self.opaque {
            self.opaque = opaque;
            cx.notify();
        }
        if record_colours != self.record_colours {
            self.record_colours = record_colours;
            self.follow(playback, true, cx);
        }
    }

    /// Follows the record playing; `again` fetches its sleeve even when it
    /// is the one shown, as after "Colours from the record" is switched.
    fn follow(&mut self, playback: &Entity<Playback>, again: bool, cx: &mut Context<Self>) {
        let now = &playback.read(cx).0;
        let playing = now.state == PlayState::Playing;
        let playing_album = now.entry.as_ref().and_then(|entry| entry.album_id);
        let shown = cx.try_global::<ShownRecord>().and_then(|s| s.0);
        if playing != self.playing {
            self.playing = playing;
            self.feed_prism(cx);
        }
        // Off, there is no wash and the accent is mint, as with no record.
        let album = shown.or(playing_album).filter(|_| self.record_colours);
        self.wash.update(cx, |wash, cx| wash.drift(playing, cx));
        // The rainbow paints over the record: its sheen in the wash, its
        // accent its own. The record's is fetched again once it goes.
        if KoanTheme::prism(cx).is_some() {
            // Another record started playing: a stop, a pause or a track of
            // the same record is not one, and the record before a stop is
            // still the one the next is told against.
            if let Some(album) = playing_album.filter(|_| playing) {
                if self.rainbow
                    && self.swept.is_some_and(|swept| swept != album)
                    && self.active
                    && KoanTheme::motion_allowed(cx)
                {
                    let middle = Point::new(self.viewport.width / 2.0, self.viewport.height / 2.0);
                    let brat = crate::theme::KoanTheme::prism_now() == Some(true);
                    self.sweep.fire(middle, self.scale, brat);
                    self.flying(SWEEP, cx);
                }
                self.swept = Some(album);
            }
            // Painted once on the way in, and again only when what is painted
            // changes; a change to what plays leaves it be.
            if !self.rainbow || again {
                self.rainbow = true;
                self.album = None;
                self.fetching = None;
                let sheen = crate::rainbow::Rainbow::sheen(cx);
                self.wash.update(cx, |wash, cx| wash.set_sleeve(sheen, cx));
            }
            return;
        }
        self.rainbow = false;
        self.swept = None;
        if album == self.album && !again {
            return;
        }
        self.album = album;
        let Some(album) = album else {
            self.fetching = None;
            self.wash.update(cx, |wash, cx| wash.set_sleeve(None, cx));
            KoanTheme::set_accent(Accent::of(None), true, cx);
            return;
        };
        let fetch = engine(cx).album_cover_art(album, Some(SAMPLE));
        let background = cx.background_executor().clone();
        self.fetching = Some(cx.spawn(async move |this, cx| {
            let decoded: Result<Option<(Option<Sleeve>, Accent)>, ()> = background
                .spawn(async move {
                    // `Err` only when the fetch failed, which is worth asking
                    // again; no art, or art that will not decode, is `Ok(None)`.
                    let Some(art) = fetch.await.map_err(|_| ())? else {
                        return Ok(None);
                    };
                    let Ok(image) = ::image::load_from_memory(&art.data) else {
                        return Ok(None);
                    };
                    let image = image.to_rgba8();
                    let small = ::image::imageops::resize(
                        &image,
                        SAMPLE,
                        SAMPLE,
                        ::image::imageops::FilterType::Triangle,
                    );
                    let pixels = small.into_raw();
                    let accent = Accent::of_sleeve(&pixels, SAMPLE, SAMPLE);
                    Ok(Some((
                        Sleeve::new(Arc::from(pixels), SAMPLE, SAMPLE),
                        accent,
                    )))
                })
                .await;
            this.update(cx, |this, cx| {
                // A record with no sleeve, or one that will not decode or
                // could not be fetched, takes the wash off and gives mint, as
                // no record does. Only a failed fetch is worth trying again:
                // forgetting the record has it asked for at the next change to
                // what is playing.
                if decoded.is_err() {
                    this.album = None;
                }
                let (sleeve, accent) = decoded.ok().flatten().unwrap_or((None, Accent::of(None)));
                this.wash.update(cx, |wash, cx| wash.set_sleeve(sleeve, cx));
                KoanTheme::set_accent(accent, true, cx);
            })
            .ok();
        }));
    }
}

impl Render for WindowRoot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        KoanTheme::drive(window, cx);
        self.viewport = window.viewport_size();
        self.scale = window.scale_factor();
        let fill = || StyleRefinement::default().size_full();
        // Nothing of it under Reduce Motion: every layer moves, and a still
        // frame of strobing spots is no calmer.
        let prism = (self.rainbow && SHADED && KoanTheme::motion_allowed(cx)).then(|| {
            let look = theme::Look::of(window, cx);
            let brat = crate::theme::KoanTheme::prism_now() == Some(true);
            // Its sizes and the ball's place in points grow with the
            // spacing, as the ball laid out below does.
            self.prism
                .style(look.dark, brat, self.scale * look.scale.space);
            self.prism.ball_at(
                MIRROR_BALL_RIGHT + MIRROR_BALL / 2.0,
                MIRROR_BALL_TOP + MIRROR_BALL / 2.0,
            );
            look
        });
        let layer = |quad: AnyElement| div().absolute().size_full().child(quad);
        div()
            .relative()
            .size_full()
            .child(div().absolute().size_full().map(|d| {
                if self.opaque {
                    d.bg(Look::of(window, cx).palette.bg.hsla())
                } else {
                    d.child(self.wash.clone().cached(fill()))
                }
            }))
            .when(prism.is_some(), |root| {
                root.child(layer(self.prism.wash().size_full().into_any_element()))
            })
            .child(
                div()
                    .absolute()
                    .size_full()
                    .child(self.content.clone().cached(fill())),
            )
            .when_some(prism, |root, look| {
                let side = look.spaced(MIRROR_BALL);
                root.child(layer(self.prism.disco().size_full().into_any_element()))
                    .child(layer(self.prism.party().size_full().into_any_element()))
                    .child(layer(self.prism.pulse().size_full().into_any_element()))
                    .child(
                        div()
                            .absolute()
                            .top(look.spaced(MIRROR_BALL_TOP))
                            .right(look.spaced(MIRROR_BALL_RIGHT))
                            .size(side)
                            .child(self.prism.ball().size_full()),
                    )
                    // A burst only while it flies.
                    .children(
                        [&self.sweep, &self.sticker, &self.confetti]
                            .into_iter()
                            .filter(|burst| burst.live())
                            .map(|burst| layer(burst.element().size_full().into_any_element())),
                    )
            })
            .child(crate::levels::paint(cx))
    }
}
