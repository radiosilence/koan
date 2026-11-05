//! The transport on a phone, as the iOS app has it. At a phone's width there
//! is room for the sleeve, what is playing and two buttons: the mini player
//! along the foot, whose tap opens Now Playing, the page everything else is
//! on (the art or the lyrics, the names leading to their pages, the heart,
//! the seek bar, the controls).

use super::*;
use crate::components::KoanInsets as _;

/// What the transport is drawn as.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Form {
    /// The desktop's bar.
    #[default]
    Bar,
    /// The phone's mini player.
    Mini,
    /// The phone's Now Playing.
    Full,
}

/// The mini player, tapped: Now Playing is asked for.
pub struct Expand;
/// Now Playing's close, tapped.
pub struct Collapse;
/// Now Playing's title, tapped: the playing track's details are asked for.
pub struct ShowInfo(pub i64);

impl EventEmitter<Expand> for Transport {}
impl EventEmitter<Collapse> for Transport {}
impl EventEmitter<ShowInfo> for Transport {}

/// The mini player's height, the playhead's line included, in points at the
/// theme's size.
pub const MINI_HEIGHT: f32 = 56.0;
/// The mini player's sleeve.
const SLEEVE: f32 = 36.0;

impl Transport {
    /// Draws the transport as `form` from now on. The phone's forms follow
    /// the playhead themselves: the mini player's line and Now Playing's
    /// times.
    pub fn set_form(&mut self, form: Form, cx: &mut Context<Self>) {
        self.form = form;
        self.playhead_followed = (form != Form::Bar).then(|| {
            let playhead = cx.global::<Mirror>().playhead.clone();
            cx.observe(&playhead, |_, _, cx| cx.notify())
        });
        cx.notify();
    }

    pub(super) fn mini(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = Paint::of(window, cx);
        let now: NowPlaying = self.playback.read(cx).0.clone();
        let entry = now.entry.as_ref();
        self.scale = window.scale_factor();
        let sleeve = p.look.spaced(SLEEVE);
        self.cover = covers::art(entry.and_then(|e| e.album_id), sleeve, self.scale, cx);
        let playing =
            now.state == PlayState::Playing || (now.state == PlayState::Stopped && now.waiting);
        // The playhead's line glides on the GPU from the engine's last anchor,
        // as the seek bar does: no frame is drawn between the anchors.
        let anchor = cx.global::<Mirror>().playhead.read(cx).clone();
        let share = if now.duration_ms > 0 {
            let at = anchor.at(std::time::Instant::now(), now.duration_ms);
            self.mini_glide.set(
                (anchor.position_ms as f64 / now.duration_ms as f64) as f32,
                if anchor.playing {
                    1000.0 / now.duration_ms as f32
                } else {
                    0.0
                },
                anchor.at,
            );
            (at as f32 / now.duration_ms as f32).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let detail = entry.map(|e| {
            if e.album.is_empty() {
                e.artist.clone()
            } else {
                format!("{} · {}", e.artist, e.album)
            }
        });
        let title = entry
            .map(|e| e.title.clone())
            .unwrap_or_else(|| theme::label("nothing playing").to_string());
        let has_entry = entry.is_some();
        div()
            .id("mini-player")
            .relative()
            .w_full()
            .h(p.look.spaced(MINI_HEIGHT))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(p.space(Space::M))
            .pl(p.space(Space::L))
            .pr(p.space(Space::S))
            // The playhead, along the top: where the track is, at a glance.
            .child(
                div().absolute().top_0().left_0().right_0().child(
                    crate::components::ProgressBar::new("mini-playhead", Some(share))
                        .glide(&self.mini_glide),
                ),
            )
            .child(Cover::new(self.cover.clone(), sleeve))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_role(TypeRole::Meta, &p.look)
                            .text_color(p.strong)
                            .truncate()
                            .child(title),
                    )
                    .children(detail.map(|detail| {
                        div()
                            .text_role(TypeRole::Fine, &p.look)
                            .text_color(p.muted)
                            .truncate()
                            .child(detail)
                    })),
            )
            .child(
                Button::icon_only(
                    "mini-play-pause",
                    if playing { Icon::Pause } else { Icon::Play },
                    if playing { "pause" } else { "play" },
                )
                .disabled(!has_entry)
                .on_click(|_, _, cx| {
                    cx.stop_propagation();
                    command(cx, "play/pause", |e| e.toggle_play_pause())
                }),
            )
            .child(
                Button::icon_only("mini-next", Icon::Next, "next")
                    .disabled(!has_entry)
                    .on_click(|_, _, cx| {
                        cx.stop_propagation();
                        command(cx, "next", |e| e.next())
                    }),
            )
            // The bar opens Now Playing; its buttons keep their own taps.
            .when(has_entry, |bar| {
                bar.on_click(cx.listener(|_, _, _, cx| cx.emit(Expand)))
            })
            .into_any_element()
    }

    pub(super) fn full(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = Paint::of(window, cx);
        let now: NowPlaying = self.playback.read(cx).0.clone();
        let entry = now.entry.as_ref();
        self.scale = window.scale_factor();
        let viewport = window.viewport_size();
        let margin = p.space(Space::Xl);
        // The art as wide as the page between its margins, and no more than
        // about half its height, so the controls stay on screen.
        let side = (viewport.width - margin * 2.).min(viewport.height * 0.45);
        self.cover = covers::art(entry.and_then(|e| e.album_id), side, self.scale, cx);
        let playing =
            now.state == PlayState::Playing || (now.state == PlayState::Stopped && now.waiting);
        if !self.lyrics_shown {
            self.lyrics = None;
        }
        let stage = match (self.lyrics_shown, &self.lyrics) {
            (true, Some(lyrics)) => lyrics
                .clone()
                .cached(StyleRefinement::default().size_full())
                .into_any_element(),
            _ => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(Cover::new(self.cover.clone(), side))
                .into_any_element(),
        };
        let titles = entry.map(|e| {
            let heart = e
                .track_id
                .zip(self.feed.as_ref())
                .map(|(track, (feed, _))| {
                    let item = crate::library::drag::Playable::Track(track);
                    let on = feed.read(cx).is_favourite(item);
                    let feed = feed.clone();
                    crate::components::Heart::new(("np-full-heart", track as u64), on)
                        .role(TypeRole::Body)
                        .on_click(move |_, _, cx| {
                            feed.update(cx, |feed, cx| feed.toggle_favourite(item, cx))
                        })
                });
            let album = e.album_id.map(crate::navigator::Route::Album);
            let credit = links::credit(
                &e.artist_credit,
                &e.artist,
                |ix| ElementId::NamedInteger("np-full-artist".into(), ix as u64),
                &self.nav,
                p.ink,
            );
            div()
                .w_full()
                .flex()
                .items_center()
                .gap(p.space(Space::M))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(p.space(Space::Xs))
                        // The title opens the track's details, as on iOS.
                        .child(
                            div()
                                .id("np-full-title")
                                .text_role(TypeRole::TitleSmall, &p.look)
                                .text_color(p.strong)
                                .truncate()
                                .child(e.title.clone())
                                .when_some(e.track_id, |d, track| {
                                    d.on_click(
                                        cx.listener(move |_, _, _, cx| cx.emit(ShowInfo(track))),
                                    )
                                }),
                        )
                        .child(links::names(credit, TypeRole::Body, p.ink, &p))
                        .when(!e.album.is_empty(), |d| {
                            d.child(links::names(
                                vec![
                                    links::link(
                                        "np-full-album".into(),
                                        e.album.clone(),
                                        album,
                                        &self.nav,
                                        p.muted,
                                    )
                                    .into_any_element(),
                                ],
                                TypeRole::Meta,
                                p.muted,
                                &p,
                            ))
                        }),
                )
                .children(heart)
        });
        let play_pause = Button::icon_outlined(
            "np-full-play-pause",
            if playing { Icon::Pause } else { Icon::Play },
            if playing { "pause" } else { "play" },
        )
        .on_click(|_, _, cx| command(cx, "play/pause", |e| e.toggle_play_pause()));
        let lyrics_shown = self.lyrics_shown;
        // The bar's menus, as the desktop's: sleep, the device controlled,
        // the output, this output's EQ.
        let sleeping = now.sleep.is_some();
        let sleep = self.menu_button(
            super::Menu::Sleep,
            if sleeping {
                Icon::SleepSet
            } else {
                Icon::Sleep
            },
            if sleeping { "sleeping" } else { "sleep" },
            if sleeping {
                "sleep timer: set".to_owned()
            } else {
                "sleep timer".to_owned()
            },
            true,
            &p,
            window,
            cx,
        );
        let devices = self.control_button(&p, window, cx);
        let output = self.menu_button(
            super::Menu::Output,
            Icon::Speaker,
            "output",
            "output".to_owned(),
            true,
            &p,
            window,
            cx,
        );
        let eq = self
            .eq
            .clone()
            .filter(|_| {
                self.outputs
                    .read(cx)
                    .0
                    .as_ref()
                    .is_some_and(|o| o.owner.is_none())
            })
            .map(|eq| self.eq_button(&eq, &p, window, cx));
        let format = now.format.as_ref().map(format_badge);
        // The system's insets are padding of their own: the page's margins
        // go inside them.
        let page = div()
            .size_full()
            .flex()
            .flex_col()
            .gap(p.space(Space::L))
            .px(margin)
            .pb(p.space(Space::L))
            .child(
                div().flex().items_center().child(
                    Button::icon_only("np-close", Icon::Expand, "close")
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(Collapse))),
                ),
            )
            .child(div().flex_1().min_h_0().child(stage))
            .children(titles)
            .child(
                self.seek.clone().cached(
                    StyleRefinement::default()
                        .w_full()
                        .h(TransportMetrics::SEEK),
                ),
            )
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_between()
                    .children(controls(&now, play_pause, cx)),
            )
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        Button::icon_only("np-lyrics", Icon::Lyrics, "lyrics")
                            .selected(lyrics_shown)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.lyrics_shown = !this.lyrics_shown;
                                if this.lyrics_shown && this.lyrics.is_none() {
                                    this.lyrics = Some(cx.new(crate::lyrics::LyricsPanel::new));
                                }
                                cx.notify();
                            })),
                    )
                    .child(sleep)
                    .child(devices)
                    .child(output)
                    .children(eq),
            )
            // What is playing as it reaches the output.
            .children(format.map(|format| {
                div()
                    .w_full()
                    .flex()
                    .justify_center()
                    .text_role(TypeRole::Fine, &p.look)
                    .text_color(p.muted)
                    .child(format)
            }));
        // Pulled down by a finger it follows, and closes once let go past a
        // fifth of the screen, as the iOS app's does; short of that it goes
        // back. A pan the lyrics can still scroll is theirs.
        let close_at = viewport.height * 0.2;
        div()
            .id("now-playing-page")
            .size_full()
            .when_some(self.pull, |page, by| page.relative().top(by))
            .on_scroll_wheel(cx.listener(move |this, event: &ScrollEvent, _, cx| {
                if event.source != ScrollSource::Touch {
                    return;
                }
                let dy = event.delta.pixel_delta(px(16.)).y;
                match event.touch_phase {
                    koan_kit::app::platform::TouchPhase::Started => {
                        this.pull = Some(dy.max(px(0.)))
                    }
                    koan_kit::app::platform::TouchPhase::Moved => match this.pull {
                        Some(by) => this.pull = Some((by + dy).max(px(0.))),
                        // A fling's coasting after the release decided.
                        None => return,
                    },
                    koan_kit::app::platform::TouchPhase::Ended => {
                        let Some(by) = this.pull.take() else { return };
                        if by + dy > close_at {
                            cx.emit(Collapse);
                        }
                    }
                    koan_kit::app::platform::TouchPhase::Cancelled => this.pull = None,
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .clear_of_system(crate::components::Clear::System, window)
            .child(page)
            .into_any_element()
    }
}
