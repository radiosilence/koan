//! The transport on a phone, as the iOS app has it. At a phone's width there
//! is room for the sleeve, what is playing and two buttons: the mini player
//! along the foot, whose tap opens Now Playing, the page everything else is
//! on (the art or the lyrics, the names leading to their pages, the heart,
//! the seek bar, the controls).

use super::*;
use crate::components::KoanContextMenu as _;
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

/// Where Now Playing's art stands while the mini player's sleeve flies to
/// it: what the page tells the shell, which draws the flight.
#[derive(Clone)]
pub struct Flight {
    /// Where the art goes, once the page has been laid out.
    to: std::rc::Rc<std::cell::Cell<Option<Bounds<Pixels>>>>,
    /// Told when the art's place is known.
    shell: EntityId,
    /// The flight has landed: the art draws itself again.
    pub landed: std::rc::Rc<std::cell::Cell<bool>>,
}

impl Flight {
    pub fn new(shell: EntityId) -> Self {
        Self {
            to: Default::default(),
            shell,
            landed: Default::default(),
        }
    }

    pub fn to(&self) -> Option<Bounds<Pixels>> {
        self.to.get()
    }

    fn told(&self, bounds: Bounds<Pixels>, cx: &mut App) {
        if self.to.get() != Some(bounds) {
            self.to.set(Some(bounds));
            cx.notify(self.shell);
        }
    }
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
pub const SLEEVE: f32 = 30.0;

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

    /// The record playing, for the flight from the mini player's sleeve:
    /// `None` when nothing is playing.
    pub fn now_playing_album(&self, cx: &App) -> Option<Option<i64>> {
        self.playback
            .read(cx)
            .0
            .entry
            .as_ref()
            .map(|entry| entry.album_id)
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
        let anchor = *cx.global::<Mirror>().playhead.read(cx);
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
        let album_id = entry.and_then(|e| e.album_id).unwrap_or(0) as u64;
        let track_id = entry.and_then(|e| e.track_id).unwrap_or(0) as u64;
        let bar = div()
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
                        .thin()
                        .glide(&self.mini_glide),
                ),
            )
            // A new record's sleeve and a new track's names fade in over
            // what they replace, as Now Playing's do.
            .child(crate::components::FadeIn::new(
                ElementId::NamedInteger("mini-sleeve".into(), album_id),
                Cover::new(self.cover.clone(), sleeve),
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(crate::components::FadeIn::new(
                        ElementId::NamedInteger("mini-titles".into(), track_id),
                        div()
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
                    )),
            )
            // Where the music plays, a thumb away on every page, as on iOS.
            .child(
                div()
                    .id("mini-devices")
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(self.control_button(&p, window, cx)),
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
            // The bar opens Now Playing, by a tap or pushed up by a finger,
            // as iOS players open; its buttons keep their own taps.
            .when(has_entry, |bar| {
                bar.on_click(cx.listener(|_, _, _, cx| cx.emit(Expand)))
                    .on_scroll(cx.listener(|this, event: &ScrollEvent, _, cx| {
                        if event.source != ScrollSource::Touch {
                            return;
                        }
                        let dy = event.delta.pixel_delta(Pixels::from(16.)).y;
                        match event.touch_phase {
                            koan_kit::app::platform::TouchPhase::Started => this.lift = Some(dy),
                            koan_kit::app::platform::TouchPhase::Moved => {
                                let Some(lift) = this.lift else { return };
                                let lift = lift + dy;
                                if lift < -Pixels::from(24.) {
                                    this.lift = None;
                                    cx.emit(Expand);
                                } else {
                                    this.lift = Some(lift);
                                }
                            }
                            _ => this.lift = None,
                        }
                        cx.stop_propagation();
                    }))
            });
        // A first track's bar rises into the foot over "nothing playing".
        crate::components::Shifted::new(
            ElementId::NamedInteger("mini-arrives".into(), has_entry as u64),
            Point::new(
                Pixels::ZERO,
                if has_entry {
                    p.look.spaced(MINI_HEIGHT)
                } else {
                    Pixels::ZERO
                },
            ),
            bar,
        )
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
            _ => {
                // While the mini player's sleeve grows into this place the
                // art itself waits, a canvas telling the flight where it goes.
                let flying = self.flight.as_ref().filter(|f| !f.landed.get());
                let art = match flying {
                    Some(flight) => {
                        let flight = flight.clone();
                        div()
                            .size(side)
                            .child(
                                canvas(
                                    move |bounds, _, cx| flight.told(bounds, cx),
                                    |_, _, _, _| {},
                                )
                                .size_full(),
                            )
                            .into_any_element()
                    }
                    None => Cover::new(self.cover.clone(), side).into_any_element(),
                };
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(art)
                    .into_any_element()
            }
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
                // The track's menu, as the rows' hold opens it: its record,
                // its artist, a playlist, the queue.
                .children(e.track_id.zip(self.lib.clone()).map(|(track, lib)| {
                    let album = e.album_id;
                    div()
                        .koan_menu_on_press("np-more", window, cx, move |m, _, cx| {
                            crate::library::actions::menu(
                                m,
                                &lib,
                                std::sync::Arc::from([crate::library::drag::Playable::Track(
                                    track,
                                )]),
                                album,
                                None,
                                cx,
                            )
                        })
                        .child(Button::icon_only("np-more-button", Icon::More, "more"))
                }))
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
            // The art and the lyrics, a new record's sleeve and a new track's
            // names fade in over what they replace rather than snapping.
            .child(
                div().flex_1().min_h_0().child(
                    crate::components::FadeIn::new(
                        ElementId::NamedInteger(
                            if self.lyrics_shown {
                                "np-lyrics"
                            } else {
                                "np-art"
                            }
                            .into(),
                            entry.and_then(|e| e.album_id).unwrap_or(0) as u64,
                        ),
                        stage,
                    )
                    .fill(),
                ),
            )
            .children(titles.map(|titles| {
                crate::components::FadeIn::new(
                    ElementId::NamedInteger(
                        "np-titles".into(),
                        entry.and_then(|e| e.track_id).unwrap_or(0) as u64,
                    ),
                    titles,
                )
            }))
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
        // The page is a layer the GPU offsets while a finger holds it or it
        // eases back; nothing is drawn as it moves.
        let layered = self.pull.is_some() || self.released.is_some();
        let page = div()
            .id("now-playing-page")
            .size_full()
            .on_scroll(cx.listener(move |this, event: &ScrollEvent, window, cx| {
                if event.source != ScrollSource::Touch {
                    return;
                }
                let dy = event.delta.pixel_delta(Pixels::from(16.)).y;
                let down = |by: Pixels| Point::new(Pixels::ZERO, by.max(Pixels::ZERO));
                match event.touch_phase {
                    koan_kit::app::platform::TouchPhase::Started => {
                        if this.released == Some(true) {
                            return;
                        }
                        this.returning = None;
                        let at = dy.max(Pixels::ZERO);
                        this.pulled.follow(down(at), window);
                        this.pull = Some(at);
                        if this.released.take().is_none() {
                            cx.notify();
                        }
                    }
                    koan_kit::app::platform::TouchPhase::Moved => match this.pull {
                        Some(by) => {
                            let at = (by + dy).max(Pixels::ZERO);
                            this.pulled.follow(down(at), window);
                            this.pull = Some(at);
                        }
                        // A fling's coasting after the release decided.
                        None => return,
                    },
                    koan_kit::app::platform::TouchPhase::Ended => {
                        let Some(by) = this.pull.take() else { return };
                        let at = (by + dy).max(Pixels::ZERO);
                        if at > close_at {
                            this.pulled.follow(down(at), window);
                            this.released = Some(true);
                            cx.emit(Collapse);
                        } else {
                            this.ease_back(window, cx);
                        }
                    }
                    koan_kit::app::platform::TouchPhase::Cancelled => {
                        if this.pull.take().is_some() {
                            this.ease_back(window, cx);
                        }
                    }
                }
                cx.stop_propagation();
            }))
            .clear_of_system(crate::components::Clear::System, window)
            .child(page);
        if layered {
            self.pulled.layer(page).into_any_element()
        } else {
            page.into_any_element()
        }
    }

    /// A pull let go short of closing: the page eases back up on the GPU,
    /// and its layer is let go once it is in place.
    fn ease_back(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.released = Some(false);
        let length = self.pulled.settle(Point::default(), window, cx);
        self.returning = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(length).await;
            this.update(cx, |this, cx| {
                this.released = None;
                this.returning = None;
                cx.notify();
            })
            .ok();
        }));
    }
}
