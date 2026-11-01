//! The transport: a full-width bar along the foot of the window.
//!
//! Left, the record and track playing; centre, the controls over the seek
//! slider; right, the format and the sleep and output menus.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::prelude::*;
use gpui_kit::*;
use koan_ffi::{NowPlaying, OutputChoice, PlayState, RepeatMode, SleepTimer, StreamFormat};

use crate::covers::{self, CoversHandle};
use crate::engine::command;
use crate::mirror::{Mirror, Outputs, Playback, Playhead};
use crate::theme::{self, HAIRLINE, Palette, space, text};
use crate::time::clock;

pub const HEIGHT: Pixels = px(68.);
const COVER: Pixels = px(40.);
const SLIDER_HIT: Pixels = px(20.);

pub struct Transport {
    playback: Entity<Playback>,
    outputs: Entity<Outputs>,
    seek: Entity<Seek>,
    menu: Option<(Menu, Entity<PopupMenu>, Subscription)>,
    /// The sleeve on show, held so the cover cache keeps it: the bar is a
    /// cached view and may not draw again for a long while.
    cover: Option<Arc<RenderImage>>,
    _subscriptions: [Subscription; 3],
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Menu {
    Sleep,
    Output,
}

impl Transport {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mirror = cx.global::<Mirror>().clone();
        let covers = cx.global::<CoversHandle>().0.clone();
        Self {
            seek: cx.new(Seek::new),
            _subscriptions: [
                cx.observe(&mirror.playback, |_, _, cx| cx.notify()),
                cx.observe(&mirror.outputs, |_, _, cx| cx.notify()),
                cx.observe(&covers, |_, _, cx| cx.notify()),
            ],
            playback: mirror.playback,
            outputs: mirror.outputs,
            menu: None,
            cover: None,
        }
    }

    fn open(&mut self, which: Menu, window: &mut Window, cx: &mut Context<Self>) {
        if self.menu.as_ref().is_some_and(|(open, ..)| *open == which) {
            self.menu = None;
            cx.notify();
            return;
        }
        let menu = match which {
            Menu::Sleep => {
                let sleeping = self.playback.read(cx).0.sleep.is_some();
                PopupMenu::build(window, cx, |menu, _, _| sleep_menu(menu, sleeping))
            }
            Menu::Output => {
                let Some(outputs) = self.outputs.read(cx).0.clone() else {
                    return;
                };
                PopupMenu::build(window, cx, |menu, _, _| output_menu(menu, &outputs))
            }
        };
        let dismissed = cx.subscribe(&menu, |this, _, _: &DismissEvent, cx| {
            this.menu = None;
            cx.notify();
        });
        menu.focus_handle(cx).focus(window, cx);
        self.menu = Some((which, menu, dismissed));
        cx.notify();
    }

    fn menu_button(
        &self,
        which: Menu,
        label: &'static str,
        enabled: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let open = self
            .menu
            .as_ref()
            .filter(|(m, ..)| *m == which)
            .map(|(_, menu, _)| menu.clone());
        div()
            .relative()
            .child(
                text_button(label, p)
                    .id(label)
                    .when(!enabled, |b| b.opacity(0.4))
                    .when(enabled, |b| {
                        b.on_click(
                            cx.listener(move |this, _, window, cx| this.open(which, window, cx)),
                        )
                    }),
            )
            .children(open.map(|menu| {
                deferred(
                    anchored()
                        .anchor(Anchor::BottomRight)
                        .snap_to_window()
                        .child(menu),
                )
                .with_priority(1)
            }))
    }
}

fn sleep_menu(menu: PopupMenu, sleeping: bool) -> PopupMenu {
    let timer = |label: &'static str, timer: SleepTimer| {
        PopupMenuItem::new(label).on_click(move |_, _, cx| {
            command(cx, "sleep timer", move |e| e.set_sleep_timer(timer));
        })
    };
    let menu = menu
        .item(timer("in 15 minutes", SleepTimer::After { minutes: 15 }))
        .item(timer("in 30 minutes", SleepTimer::After { minutes: 30 }))
        .item(timer("in 60 minutes", SleepTimer::After { minutes: 60 }))
        .item(timer("at the end of the track", SleepTimer::EndOfTrack))
        .item(timer("at the end of the record", SleepTimer::EndOfRecord));
    if sleeping {
        menu.separator().item(
            PopupMenuItem::new("cancel")
                .on_click(|_, _, cx| command(cx, "cancel sleep timer", |e| e.cancel_sleep_timer())),
        )
    } else {
        menu
    }
}

fn output_menu(menu: PopupMenu, outputs: &koan_ffi::OutputsInfo) -> PopupMenu {
    let choice = |label: String, chosen: bool, choice: OutputChoice| {
        PopupMenuItem::new(label)
            .checked(chosen)
            .on_click(move |_, _, cx| {
                let choice = choice.clone();
                command(cx, "select output", move |e| e.select_output(choice));
            })
    };
    let mut menu = menu.item(choice(
        "system default".into(),
        outputs.current == OutputChoice::Default,
        OutputChoice::Default,
    ));
    for device in &outputs.devices {
        let id = OutputChoice::Device {
            name: device.name.clone(),
        };
        menu = menu.item(choice(device.name.clone(), outputs.current == id, id));
    }
    if !outputs.renderers.is_empty() {
        menu = menu.separator();
        for renderer in &outputs.renderers {
            let id = OutputChoice::Renderer {
                udn: renderer.id.clone(),
            };
            menu = menu.item(choice(renderer.name.clone(), outputs.current == id, id));
        }
    }
    menu
}

/// The theme's text button: the label in `muted`, `ink` under the pointer.
fn text_button(label: &'static str, p: &Palette) -> Div {
    let ink = p.ink;
    div()
        .px(space::S)
        .py(space::XS)
        .cursor_pointer()
        .text_size(text::META)
        .text_color(p.muted)
        .hover(move |s| s.text_color(ink))
        .child(label)
}

/// A transport control. The glyph set replaces these labels.
fn control(id: &'static str, label: &'static str, on: Option<bool>, p: &Palette) -> Stateful<Div> {
    let (color, underline) = match on {
        Some(true) => (p.ink, Some(p.accent)),
        Some(false) => (p.muted, None),
        None => (p.ink, None),
    };
    div()
        .id(id)
        .min_w(px(44.))
        .h(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .text_size(text::CONTROL)
        .text_color(color)
        .hover(|s| s.opacity(0.8))
        .child(
            div()
                .when_some(underline, |d, accent| d.border_b_1().border_color(accent))
                .child(label),
        )
}

fn format_badge(format: &StreamFormat) -> String {
    let rate = format.sample_rate as f32 / 1000.0;
    let rate = if rate.fract() == 0.0 {
        format!("{rate:.0}")
    } else {
        format!("{rate:.1}")
    };
    match format.bit_depth {
        Some(bits) => format!("{} {bits}/{rate}", format.codec.to_lowercase()),
        None => match format.bitrate_kbps {
            Some(kbps) => format!("{} {kbps}k", format.codec.to_lowercase()),
            None => format!("{} {rate}", format.codec.to_lowercase()),
        },
    }
}

impl Render for Transport {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = theme::palette(cx);
        let now: NowPlaying = self.playback.read(cx).0.clone();
        let entry = now.entry.as_ref();
        self.cover = covers::image(entry.and_then(|e| e.album_id), COVER, cx);
        let playing = now.state == PlayState::Playing;
        let has_outputs = self.outputs.read(cx).0.is_some();
        let repeat_label = match now.repeat_mode {
            RepeatMode::One => "repeat one",
            RepeatMode::Off | RepeatMode::Queue => "repeat",
        };
        let next_repeat = match now.repeat_mode {
            RepeatMode::Off => RepeatMode::Queue,
            RepeatMode::Queue => RepeatMode::One,
            RepeatMode::One => RepeatMode::Off,
        };
        let shuffle = now.shuffle;

        let left = div()
            .flex_1()
            .min_w(px(0.))
            .flex()
            .items_center()
            .gap(space::M)
            .child(covers::draw(self.cover.clone(), COVER, cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w(px(0.))
                    .children(entry.map(|e| {
                        div()
                            .truncate()
                            .text_size(text::META)
                            .text_color(p.strong)
                            .child(e.title.clone())
                    }))
                    .children(entry.map(|e| {
                        let detail = if e.album.is_empty() {
                            e.artist.clone()
                        } else {
                            format!("{} · {}", e.artist, e.album)
                        };
                        div()
                            .truncate()
                            .text_size(text::FINE)
                            .text_color(p.muted)
                            .child(detail)
                    })),
            );

        let play_pause = div()
            .id("play-pause")
            .w(px(36.))
            .h(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .border_1()
            .border_color(p.ink)
            .cursor_pointer()
            .text_size(text::CONTROL)
            .text_color(p.ink)
            .hover(|s| s.bg(p.hover))
            .child(if playing { "pause" } else { "play" })
            .on_click(|_, _, cx| command(cx, "play/pause", |e| e.toggle_play_pause()));

        let centre = div()
            .w(px(520.))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(2.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::S)
                    .child(control("shuffle", "shuffle", Some(shuffle), &p).on_click(
                        move |_, _, cx| {
                            command(cx, "shuffle", move |e| e.set_shuffle(!shuffle));
                        },
                    ))
                    .child(
                        control("previous", "prev", None, &p)
                            .on_click(|_, _, cx| command(cx, "previous", |e| e.previous())),
                    )
                    .child(play_pause)
                    .child(
                        control("next", "next", None, &p)
                            .on_click(|_, _, cx| command(cx, "next", |e| e.next())),
                    )
                    .child(
                        control(
                            "repeat",
                            repeat_label,
                            Some(now.repeat_mode != RepeatMode::Off),
                            &p,
                        )
                        .on_click(move |_, _, cx| {
                            command(cx, "repeat", move |e| e.set_repeat(next_repeat))
                        }),
                    ),
            )
            // Its own cached view: the playhead's ticks redraw the slider and
            // what holds it, never the rest of the bar's layout.
            .child(
                self.seek
                    .clone()
                    .cached(StyleRefinement::default().w_full().h(SLIDER_HIT)),
            );

        let right = div()
            .flex_1()
            .min_w(px(0.))
            .flex()
            .items_center()
            .justify_end()
            .gap(space::S)
            .children(now.format.as_ref().map(|format| {
                div()
                    .px(space::XS)
                    .border_1()
                    .border_color(p.rule)
                    .text_size(text::FINE)
                    .text_color(p.ink)
                    .child(format_badge(format))
            }))
            .child(self.menu_button(
                Menu::Sleep,
                if now.sleep.is_some() {
                    "sleeping"
                } else {
                    "sleep"
                },
                true,
                &p,
                cx,
            ))
            .child(self.menu_button(Menu::Output, "output", has_outputs, &p, cx));

        div()
            .h(HEIGHT)
            .w_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(space::L)
            .px(space::L)
            .bg(p.bg)
            .border_t(HAIRLINE)
            .border_color(p.rule)
            .child(left)
            .child(centre)
            .child(right)
    }
}

/// The seek slider and the times either side of it.
///
/// The position is reckoned from the playhead's anchor. While it runs, one
/// timer is set for the next moment the slider or the times would draw
/// differently (the next pixel of the fill, or the next second); paused or
/// stopped, nothing is set, so a koan at rest costs nothing.
pub struct Seek {
    playhead: Entity<Playhead>,
    playback: Entity<Playback>,
    tick: Option<Task<()>>,
    /// Where the slider was last laid out, for turning a pointer into a
    /// position and the duration into a step.
    bounds: Bounds<Pixels>,
    /// While the thumb is held: where it is, as a share of the track.
    dragging: Option<f32>,
    _subscriptions: [Subscription; 2],
}

impl Seek {
    fn new(cx: &mut Context<Self>) -> Self {
        let mirror = cx.global::<Mirror>().clone();
        let mut seek = Self {
            _subscriptions: [
                cx.observe(&mirror.playhead, |this, _, cx| this.moved(cx)),
                cx.observe(&mirror.playback, |this, _, cx| this.moved(cx)),
            ],
            playhead: mirror.playhead,
            playback: mirror.playback,
            tick: None,
            bounds: Bounds::default(),
            dragging: None,
        };
        seek.schedule(cx);
        seek
    }

    fn moved(&mut self, cx: &mut Context<Self>) {
        self.schedule(cx);
        cx.notify();
    }

    fn duration_ms(&self, cx: &App) -> u64 {
        self.playback.read(cx).0.duration_ms
    }

    fn schedule(&mut self, cx: &mut Context<Self>) {
        self.tick = None;
        let anchor = *self.playhead.read(cx);
        let duration = self.duration_ms(cx);
        if !anchor.playing || duration == 0 {
            return;
        }
        let position = anchor.at(Instant::now(), duration);
        if position >= duration {
            return;
        }
        let to_second = 1000 - position % 1000;
        let width = f32::from(self.bounds.size.width);
        let to_pixel = if width > 0.0 {
            (duration as f32 / width).ceil() as u64
        } else {
            to_second
        };
        // No more often than a 60 Hz frame: a short track on a wide slider
        // would otherwise step its fill faster than the display shows it.
        let wait = Duration::from_millis(to_second.min(to_pixel).max(16));
        self.tick = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            this.update(cx, |this, cx| this.moved(cx)).ok();
        }));
    }

    fn share_at(&self, x: Pixels) -> f32 {
        let width = f32::from(self.bounds.size.width);
        if width <= 0.0 {
            return 0.0;
        }
        (f32::from(x - self.bounds.origin.x) / width).clamp(0.0, 1.0)
    }

    fn commit(&mut self, cx: &mut Context<Self>) {
        let Some(share) = self.dragging.take() else {
            return;
        };
        let duration = self.duration_ms(cx);
        let seekable = self.playhead.read(cx).seekable_ms;
        // The engine clamps a seek to what has arrived of a download; so does
        // the slider, so the playhead lands where the thumb was let go.
        let limit = if seekable > 0 {
            seekable.min(duration)
        } else {
            duration
        };
        let target = ((share as f64 * duration as f64) as u64).min(limit);
        if duration > 0 {
            command(cx, "seek", move |e| e.seek(target));
        }
        cx.notify();
    }
}

impl Render for Seek {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = theme::palette(cx);
        let duration = self.duration_ms(cx);
        let position = self.playhead.read(cx).at(Instant::now(), duration);
        let share = match self.dragging {
            Some(share) => share,
            None if duration > 0 => (position as f64 / duration as f64) as f32,
            None => 0.0,
        };
        let shown = self
            .dragging
            .map_or(position, |s| (s as f64 * duration as f64) as u64);
        let held = self.dragging.is_some();
        let view = cx.entity();

        let slider = div()
            .id("seek")
            .group("seek")
            .relative()
            .flex_1()
            .h(SLIDER_HIT)
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    if this.duration_ms(cx) > 0 {
                        this.dragging = Some(this.share_at(event.position.x));
                        cx.notify();
                    }
                }),
            )
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(SLIDER_HIT / 2. - HAIRLINE / 2.)
                    .h(HAIRLINE)
                    .bg(p.rule),
            )
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top(SLIDER_HIT / 2. - px(1.5))
                    .h(px(3.))
                    .w(relative(share))
                    .bg(p.accent),
            )
            .child(
                div()
                    .absolute()
                    .top(SLIDER_HIT / 2. - px(4.))
                    .left(relative(share))
                    .ml(px(-4.))
                    .size(px(8.))
                    .bg(p.ink)
                    .when(!held, |d| {
                        d.opacity(0.).group_hover("seek", |s| s.opacity(1.))
                    }),
            )
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, (), window, cx| {
                        view.update(cx, |this, _| this.bounds = bounds);
                        // While held, the pointer is followed past the
                        // slider's edges and let go of anywhere. Listened for
                        // on every frame, not only once a frame has drawn the
                        // thumb held: a tap's release can arrive before that.
                        let moved = view.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                            if phase == DispatchPhase::Bubble
                                && event.pressed_button == Some(MouseButton::Left)
                            {
                                moved.update(cx, |this, cx| {
                                    if this.dragging.is_some() {
                                        this.dragging = Some(this.share_at(event.position.x));
                                        cx.notify();
                                    }
                                });
                            }
                        });
                        let released = view.clone();
                        window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                            if phase == DispatchPhase::Bubble {
                                released.update(cx, |this, cx| this.commit(cx));
                            }
                        });
                    },
                )
                .absolute()
                .size_full(),
            );

        let time = |ms: u64| {
            div()
                .w(px(48.))
                .flex_shrink_0()
                .text_size(text::FINE)
                .text_color(p.muted)
                .child(clock(ms))
        };

        div()
            .w_full()
            .flex()
            .items_center()
            .gap(space::S)
            .child(time(shown).flex().justify_end())
            .child(slider)
            .child(time(duration))
    }
}
