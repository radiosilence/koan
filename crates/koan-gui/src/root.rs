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
use gpui_kit::prelude::*;
use gpui_kit::*;
use koan_ffi::PlayState;

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
    /// The sleeve on its way; replacing it drops the one before.
    fetching: Option<Task<()>>,
    _subscriptions: [Subscription; 2],
}

impl WindowRoot {
    pub fn new(content: impl Into<AnyView>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let playback = cx.global::<Mirror>().playback.clone();
        let mut root = Self {
            wash: cx.new(|_| Wash::new()),
            content: content.into(),
            album: None,
            fetching: None,
            _subscriptions: [
                theme::follow_appearance(window, cx),
                cx.observe(&playback, |this, playback, cx| this.follow(&playback, cx)),
            ],
        };
        root.follow(&playback, cx);
        root
    }

    fn follow(&mut self, playback: &Entity<Playback>, cx: &mut Context<Self>) {
        let now = &playback.read(cx).0;
        let playing = now.state == PlayState::Playing;
        let album = now.entry.as_ref().and_then(|entry| entry.album_id);
        self.wash.update(cx, |wash, cx| wash.drift(playing, cx));
        if album == self.album {
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
            let decoded = background
                .spawn(async move {
                    let art = fetch.await.ok().flatten()?;
                    let image = image::load_from_memory(&art.data).ok()?.to_rgba8();
                    let small = image::imageops::resize(
                        &image,
                        SAMPLE,
                        SAMPLE,
                        image::imageops::FilterType::Triangle,
                    );
                    let pixels = small.into_raw();
                    let accent = Accent::of_sleeve(&pixels, SAMPLE, SAMPLE);
                    Some((Sleeve::new(Arc::from(pixels), SAMPLE, SAMPLE), accent))
                })
                .await;
            this.update(cx, |this, cx| {
                // A record with no sleeve, or one that will not decode, takes
                // the wash off and gives mint, as no record does.
                let (sleeve, accent) = decoded.unwrap_or((None, Accent::of(None)));
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
    }
}
