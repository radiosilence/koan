//! A window's root: the wash beneath and the content over it, as two cached
//! views (see ARCHITECTURE.md), so a frame of the drift or of the accent
//! settling never lays the page out again.
//!
//! The root also follows the playing record: its sleeve, fetched and decoded
//! off the main thread, sets the accent and the wash, and the wash drifts
//! while something plays.

use std::sync::Arc;

use crate::theme::wash::{Sleeve, Wash};
use crate::theme::{self, Accent, KoanTheme};
use koan_ffi::PlayState;
use koan_kit::app::*;

use crate::engine::engine;
use crate::mirror::{Mirror, Playback};

/// The side the sleeve is fetched and analysed at: enough for the accent's
/// hue and the wash's blur, which is baked smaller still.
const SAMPLE: u32 = 128;

pub struct WindowRoot {
    wash: Entity<Wash>,
    content: AnyView,
    /// The record whose sleeve the wash and accent show.
    album: Option<i64>,
    /// "Colours from the record" and the theme tint, as last applied.
    record_colours: bool,
    wash_tint: u32,
    /// The sleeve on its way; replacing it drops the one before.
    fetching: Option<Task<()>>,
    _subscriptions: [Subscription; 6],
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
        let followed = playback.clone();
        let shown = playback.clone();
        let painted = playback.clone();
        let mut root = Self {
            wash: cx.new(|_| Wash::new()),
            content: content.into(),
            album: None,
            record_colours,
            wash_tint,
            fetching: None,
            _subscriptions: [
                cx.observe_global::<ShownRecord>(move |this, cx| this.follow(&shown, false, cx)),
                theme::follow_appearance(window, cx),
                cx.observe(&playback, |this, playback, cx| {
                    this.follow(&playback, false, cx)
                }),
                cx.observe_global::<KoanTheme>(move |this, cx| this.restyle(&followed, cx)),
                // The rainbow drawn or put away: its sheen, or the record's
                // sleeve and accent back.
                cx.observe_global::<crate::rainbow::Painted>(move |this, cx| {
                    this.follow(&painted, true, cx)
                }),
                // Its accent cycles only while a window is in front.
                cx.observe_window_activation(window, |_, _, cx| {
                    let active = cx.active_window().is_some();
                    crate::rainbow::set_active(active, cx);
                }),
            ],
            _levels: [
                crate::levels::feed(window.window_handle().window_id(), cx)
                    .map(|feed| cx.observe(&feed, |_, _, cx| cx.notify())),
                // A hidden window's bars leave the analyser at rest.
                Some(
                    cx.observe_window_visibility(window, |_, visibility, window, cx| {
                        let fps = crate::levels::display_fps(window, cx);
                        crate::levels::visibility(
                            window.window_handle().window_id(),
                            visibility == WindowVisibility::Visible,
                            fps,
                            cx,
                        )
                    }),
                ),
            ],
        };
        root.wash
            .update(cx, |wash, cx| wash.set_hold(wash_tint, cx));
        root.follow(&playback, false, cx);
        root
    }

    /// Settings changed the record's colours or the tint: the wash follows.
    fn restyle(&mut self, playback: &Entity<Playback>, cx: &mut Context<Self>) {
        let theme = cx.global::<KoanTheme>();
        let (record_colours, wash_tint) = (theme.record_colours, theme.wash_tint);
        if wash_tint != self.wash_tint {
            self.wash_tint = wash_tint;
            self.wash
                .update(cx, |wash, cx| wash.set_hold(wash_tint, cx));
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
        // Off, there is no wash and the accent is mint, as with no record.
        let shown = cx.try_global::<ShownRecord>().and_then(|s| s.0);
        let album = shown
            .or_else(|| now.entry.as_ref().and_then(|entry| entry.album_id))
            .filter(|_| self.record_colours);
        self.wash.update(cx, |wash, cx| wash.drift(playing, cx));
        // The rainbow paints over the record: its sheen in the wash, its
        // accent its own. The record's is fetched again once it goes.
        if crate::rainbow::Rainbow::drawn(cx) {
            self.album = None;
            self.fetching = None;
            let sheen = crate::rainbow::Rainbow::sheen(cx);
            self.wash.update(cx, |wash, cx| wash.set_sleeve(sheen, cx));
            return;
        }
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
                    let Ok(image) = image::load_from_memory(&art.data) else {
                        return Ok(None);
                    };
                    let image = image.to_rgba8();
                    let small = image::imageops::resize(
                        &image,
                        SAMPLE,
                        SAMPLE,
                        image::imageops::FilterType::Triangle,
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
        let fill = || StyleRefinement::default().size_full();
        div()
            .relative()
            .size_full()
            .child(
                div()
                    .absolute()
                    .size_full()
                    .child(self.wash.clone().cached(fill())),
            )
            .child(
                div()
                    .absolute()
                    .size_full()
                    .child(self.content.clone().cached(fill())),
            )
            .child(crate::levels::paint(cx))
    }
}
