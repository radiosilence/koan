//! The transport: a full-width bar along the foot of the window.
//!
//! Left, the record and track playing; centre, the controls over the seek
//! slider; right, the format, the sleep and output menus, and EQ.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::prelude::*;
use gpui_kit::*;
use koan_ffi::{
    DspOverview, NowPlaying, OutputChoice, PlayState, RepeatMode, SleepTimer, StreamFormat,
};

use crate::components::{Menu as ThemeMenu, MenuEntry};
use crate::covers::{self, CoversHandle};
use crate::engine::{command, engine};
use crate::mirror::{Mirror, Outputs, Playback, Playhead};
use crate::paint::Paint;
use crate::theme::{self, Icon, KoanIcon, KoanStyled, Space, TypeRole};
use crate::time::clock;

pub const HEIGHT: Pixels = px(68.);
const COVER: Pixels = px(40.);
const SLIDER_HIT: Pixels = px(20.);

pub struct Transport {
    playback: Entity<Playback>,
    outputs: Entity<Outputs>,
    seek: Entity<Seek>,
    menu: Option<(Menu, Entity<ThemeMenu>, Subscription)>,
    /// The sleeve on show, held so the cover cache keeps it: the bar is a
    /// cached view and may not draw again for a long while.
    cover: Option<Arc<RenderImage>>,
    /// What the output in use plays, for the EQ button; `None` while there
    /// are no EQ profiles, or while another device is controlled, whose
    /// presets are chosen under Output there.
    eq: Option<EqState>,
    /// The profile the playing stream named when `eq` was read.
    eq_profile: Option<String>,
    _eq_load: Option<Task<()>>,
    _subscriptions: [Subscription; 6],
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Menu {
    Sleep,
    Output,
    Eq,
}

/// The EQ of the output in use, as its button and menu show it.
#[derive(Clone, Debug, PartialEq)]
struct EqState {
    device: String,
    /// What to call the device: a renderer is named by its UDN otherwise.
    name: String,
    /// The preset it was set from, and whether it was changed since.
    preset: Option<String>,
    edited: bool,
    /// No correction and no tuning: it plays untouched.
    flat: bool,
    presets: Vec<String>,
}

impl EqState {
    fn of(overview: &DspOverview) -> Option<Self> {
        if overview.profiles.is_empty() {
            return None;
        }
        let device = overview.device.clone()?;
        let state = overview.outputs.iter().find(|o| o.device == device);
        Some(Self {
            name: overview
                .names
                .get(&device)
                .cloned()
                .unwrap_or_else(|| device.clone()),
            preset: state.and_then(|s| s.preset.clone()),
            edited: state.is_some_and(|s| s.preset_edited),
            flat: state.is_none_or(|s| s.flat),
            presets: overview
                .profiles
                .iter()
                .filter(|p| p.preset)
                .map(|p| p.name.clone())
                .collect(),
            device,
        })
    }

    /// What the button's tooltip says: the preset in force, never "flat".
    fn summary(&self) -> String {
        match &self.preset {
            Some(preset) if self.edited => format!("EQ: {preset}, edited"),
            Some(preset) => format!("EQ: {preset}"),
            None if self.flat => "EQ".into(),
            None => "EQ: unsaved".into(),
        }
    }
}

impl Transport {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mirror = cx.global::<Mirror>().clone();
        let covers = cx.global::<CoversHandle>().0.clone();
        let mut this = Self {
            seek: cx.new(|cx| Seek::new(window, cx)),
            _subscriptions: [
                cx.observe(&mirror.playback, |this, playback, cx| {
                    // A new stream naming another profile: the output or its
                    // EQ moved without the outputs saying so.
                    let profile = playback
                        .read(cx)
                        .0
                        .format
                        .as_ref()
                        .and_then(|f| f.dsp.as_ref())
                        .map(|d| d.profile.clone());
                    if profile != this.eq_profile {
                        this.eq_profile = profile;
                        this.reload_eq(cx);
                    }
                    cx.notify();
                }),
                cx.observe(&mirror.outputs, |_, _, cx| cx.notify()),
                // Redrawn when its own sleeve lands, not for every sleeve the
                // queue fetches as it scrolls.
                cx.observe(&covers, |this, _, cx| {
                    if this.cover.is_none() {
                        let album = this
                            .playback
                            .read(cx)
                            .0
                            .entry
                            .as_ref()
                            .and_then(|e| e.album_id);
                        if covers::image(album, COVER, cx).is_some() {
                            cx.notify();
                        }
                    }
                }),
                // Shuffle and repeat are underlined in the accent.
                theme::observe_accent(cx),
                // What EQ plays follows the output, its presets (published
                // with the outputs) and the profiles (synced with the library).
                cx.observe(&mirror.outputs, |this, _, cx| this.reload_eq(cx)),
                cx.observe(&mirror.library, |this, _, cx| this.reload_eq(cx)),
            ],
            playback: mirror.playback,
            outputs: mirror.outputs,
            menu: None,
            cover: None,
            eq: None,
            eq_profile: None,
            _eq_load: None,
        };
        this.reload_eq(cx);
        this
    }

    /// Reads what the output in use plays again; a newer read replaces one
    /// in flight. Redraws only when the button or its menu would change.
    fn reload_eq(&mut self, cx: &mut Context<Self>) {
        let read = engine(cx).dsp_overview();
        self._eq_load = Some(cx.spawn(async move |this, cx| {
            let overview = read.await;
            this.update(cx, |this, cx| {
                let eq = EqState::of(&overview);
                if eq != this.eq {
                    this.eq = eq;
                    cx.notify();
                }
            })
            .ok();
        }));
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
                ThemeMenu::build(window, cx, |menu, _, _| sleep_menu(menu, sleeping))
            }
            Menu::Output => {
                let Some(outputs) = self.outputs.read(cx).0.clone() else {
                    return;
                };
                ThemeMenu::build(window, cx, |menu, _, _| output_menu(menu, &outputs))
            }
            Menu::Eq => {
                let Some(eq) = self.eq.clone() else {
                    return;
                };
                ThemeMenu::build(window, cx, move |menu, _, _| eq_menu(menu, &eq))
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
        icon: Icon,
        label: &'static str,
        enabled: bool,
        p: &Paint,
        cx: &mut Context<Self>,
    ) -> Div {
        self.menu_anchor(
            which,
            text_button(icon, label, p)
                .id(label)
                .when(!enabled, |b| b.opacity(0.4))
                .when(enabled, |b| {
                    b.on_click(cx.listener(move |this, _, window, cx| this.open(which, window, cx)))
                }),
        )
    }

    /// The EQ button: the glyph in `ink` while the output plays untouched,
    /// in the accent while an EQ is in force, its name in the tooltip.
    fn eq_button(&self, eq: &EqState, p: &Paint, cx: &mut Context<Self>) -> Div {
        let color = if eq.flat { p.ink } else { p.accent_text };
        let summary: SharedString = eq.summary().into();
        let child = if p.look.icons {
            KoanIcon::new(Icon::Filters)
                .role(TypeRole::Body)
                .color(if eq.flat { p.ink } else { p.accent })
                .into_any_element()
        } else {
            div().child("eq").into_any_element()
        };
        self.menu_anchor(
            Menu::Eq,
            div()
                .id("eq")
                .flex()
                .items_center()
                .justify_center()
                .min_w(theme::HIT)
                .min_h(theme::HIT)
                .px(p.space(Space::S))
                .cursor_pointer()
                .text_role(TypeRole::Meta, &p.look)
                .text_color(color)
                .hover(|s| s.opacity(0.8))
                .child(child)
                .tooltip(move |window, cx| {
                    crate::components::Tooltip::view(summary.clone(), window, cx)
                })
                .on_click(cx.listener(|this, _, window, cx| this.open(Menu::Eq, window, cx))),
        )
    }

    /// `button`, with its menu under it while open.
    fn menu_anchor(&self, which: Menu, button: impl IntoElement) -> Div {
        let open = self
            .menu
            .as_ref()
            .filter(|(m, ..)| *m == which)
            .map(|(_, menu, _)| menu.clone());
        div().relative().child(button).children(open.map(|menu| {
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

fn sleep_menu(menu: ThemeMenu, sleeping: bool) -> ThemeMenu {
    let timer = |label: &'static str, timer: SleepTimer| {
        MenuEntry::new(label).on_click(move |_, _, cx| {
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
            MenuEntry::new("cancel")
                .on_click(|_, _, cx| command(cx, "cancel sleep timer", |e| e.cancel_sleep_timer())),
        )
    } else {
        menu
    }
}

fn output_menu(menu: ThemeMenu, outputs: &koan_ffi::OutputsInfo) -> ThemeMenu {
    let choice = |label: String, chosen: bool, choice: OutputChoice| {
        MenuEntry::new(label)
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

/// The output's EQ: none, or each preset, the one it was set from ticked. A
/// preset changed since is listed twice, as edited, ticked, and as saved,
/// which goes back to it. Edit… opens the EQ window on the device.
fn eq_menu(menu: ThemeMenu, eq: &EqState) -> ThemeMenu {
    let choose = |label: String, chosen: bool, preset: Option<String>| {
        let device = eq.device.clone();
        MenuEntry::new(label)
            .checked(chosen)
            .on_click(move |_, _, cx| {
                let (device, preset) = (device.clone(), preset.clone());
                command(cx, "EQ preset", move |e| e.dsp_apply_preset(device, preset));
            })
    };
    let mut menu = menu
        .label(eq.name.clone())
        .item(choose("No EQ".into(), eq.flat, None));
    if eq.preset.is_none() && !eq.flat {
        menu = menu.item(MenuEntry::new("Unsaved").checked(true).disabled(true));
    }
    for preset in &eq.presets {
        let current = eq.preset.as_ref() == Some(preset);
        if current && eq.edited {
            menu = menu.item(
                MenuEntry::data(format!("{preset}, edited"))
                    .checked(true)
                    .disabled(true),
            );
        }
        menu = menu.item(
            MenuEntry::data(preset.clone())
                .checked(current && !eq.edited)
                .on_click({
                    let (device, preset) = (eq.device.clone(), preset.clone());
                    move |_, _, cx| {
                        let (device, preset) = (device.clone(), Some(preset.clone()));
                        command(cx, "EQ preset", move |e| e.dsp_apply_preset(device, preset));
                    }
                }),
        );
    }
    let device = eq.device.clone();
    menu.separator().item(
        MenuEntry::new("Edit…")
            .on_click(move |_, _, cx| crate::dsp::window::open(Some(device.clone()), cx)),
    )
}

/// The theme's text button: the label in `muted`, `ink` under the pointer.
/// The control's glyph with icons on, its label without: the theme's two
/// variants. The label is what the plain variant shows, so it is lowercase.
fn glyph_or_label(
    icon: Icon,
    label: &'static str,
    role: TypeRole,
    color: Hsla,
    p: &Paint,
) -> AnyElement {
    if p.look.icons {
        KoanIcon::new(icon)
            .role(role)
            .color(color)
            .into_any_element()
    } else {
        div().child(label).into_any_element()
    }
}

/// The theme's text button, or with icons its icon button: `muted`, `ink`
/// under the pointer.
fn text_button(icon: Icon, label: &'static str, p: &Paint) -> Div {
    let ink = p.ink;
    let child = if p.look.icons {
        // A glyph button: the transport's glyph size, on the spec's target.
        KoanIcon::new(icon)
            .role(TypeRole::Body)
            .color(p.muted)
            .hover_color(TEXT_BUTTON, ink)
            .into_any_element()
    } else {
        div().child(label).into_any_element()
    };
    div()
        .group(TEXT_BUTTON)
        .flex()
        .items_center()
        .justify_center()
        .min_w(theme::HIT)
        .min_h(theme::HIT)
        .px(p.space(Space::S))
        .cursor_pointer()
        .text_role(TypeRole::Meta, &p.look)
        .text_color(p.muted)
        .hover(move |s| s.text_color(ink))
        .child(child)
}

/// The group a text button's glyph follows the pointer by.
const TEXT_BUTTON: &str = "transport-text-button";

/// A transport control: `on` for the two that toggle, underlined in the
/// accent while on and `muted` while off, as an icon toggle is.
fn control(
    id: &'static str,
    icon: Icon,
    label: &'static str,
    on: Option<bool>,
    p: &Paint,
) -> Stateful<Div> {
    let (color, underline) = match on {
        Some(true) => (p.ink, Some(p.accent)),
        Some(false) => (p.muted, None),
        None => (p.ink, None),
    };
    div()
        .id(id)
        .min_w(theme::HIT)
        .h(theme::HIT)
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .text_role(TypeRole::Control, &p.look)
        .text_color(color)
        .hover(|s| s.opacity(0.8))
        .child(
            div()
                .when_some(underline, |d, accent| d.border_b_1().border_color(accent))
                .child(glyph_or_label(icon, label, TypeRole::Body, color, p)),
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
        Some(bits) => format!("{} {bits}/{rate}", format.codec.to_uppercase()),
        None => match format.bitrate_kbps {
            Some(kbps) => format!("{} {kbps}k", format.codec.to_uppercase()),
            None => format!("{} {rate}", format.codec.to_uppercase()),
        },
    }
}

impl Render for Transport {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Paint::of(window, cx);
        let now: NowPlaying = self.playback.read(cx).0.clone();
        let entry = now.entry.as_ref();
        self.cover = covers::image(entry.and_then(|e| e.album_id), COVER, cx);
        let playing = now.state == PlayState::Playing;
        let has_outputs = self.outputs.read(cx).0.is_some();
        let repeat_icon = match now.repeat_mode {
            RepeatMode::One => Icon::RepeatOne,
            RepeatMode::Off | RepeatMode::Queue => Icon::RepeatQueue,
        };
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
            .gap(p.space(Space::M))
            .child(covers::draw(self.cover.clone(), COVER, p.surface))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w(px(0.))
                    .children(entry.map(|e| {
                        div()
                            .truncate()
                            .text_role(TypeRole::Meta, &p.look)
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
                            .text_role(TypeRole::Fine, &p.look)
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
            .text_role(TypeRole::Control, &p.look)
            .text_color(p.ink)
            .hover(|s| s.bg(p.hover))
            .child(if playing {
                glyph_or_label(Icon::Pause, "pause", TypeRole::Body, p.ink, &p)
            } else {
                glyph_or_label(Icon::Play, "play", TypeRole::Body, p.ink, &p)
            })
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
                    .gap(p.space(Space::S))
                    .child(
                        control("shuffle", Icon::Shuffle, "shuffle", Some(shuffle), &p).on_click(
                            move |_, _, cx| {
                                command(cx, "shuffle", move |e| e.set_shuffle(!shuffle));
                            },
                        ),
                    )
                    .child(
                        control("previous", Icon::Previous, "prev", None, &p)
                            .on_click(|_, _, cx| command(cx, "previous", |e| e.previous())),
                    )
                    .child(play_pause)
                    .child(
                        control("next", Icon::Next, "next", None, &p)
                            .on_click(|_, _, cx| command(cx, "next", |e| e.next())),
                    )
                    .child(
                        control(
                            "repeat",
                            repeat_icon,
                            repeat_label,
                            Some(now.repeat_mode != RepeatMode::Off),
                            &p,
                        )
                        .on_click(move |_, _, cx| {
                            command(cx, "repeat", move |e| e.set_repeat(next_repeat))
                        }),
                    ),
            )
            // Its own cached view. A tick re-renders the slider and the views
            // that hold it (the bar, the shell), whose other cached children
            // replay their last frame rather than laying out again.
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
            .gap(p.space(Space::S))
            .children(now.format.as_ref().map(|format| {
                div()
                    .px(p.space(Space::Xs))
                    .border_1()
                    .border_color(p.rule)
                    .text_role(TypeRole::Fine, &p.look)
                    .text_color(p.ink)
                    .child(format_badge(format))
            }))
            .child(self.menu_button(
                Menu::Sleep,
                if now.sleep.is_some() {
                    Icon::SleepSet
                } else {
                    Icon::Sleep
                },
                if now.sleep.is_some() {
                    "sleeping"
                } else {
                    "sleep"
                },
                true,
                &p,
                cx,
            ))
            .child(self.menu_button(Menu::Output, Icon::Speaker, "output", has_outputs, &p, cx))
            // This device's own output's EQ; another device's is chosen there.
            .children(
                self.eq
                    .clone()
                    .filter(|_| {
                        self.outputs
                            .read(cx)
                            .0
                            .as_ref()
                            .is_none_or(|o| o.owner.is_none())
                    })
                    .map(|eq| self.eq_button(&eq, &p, cx)),
            );

        div()
            .h(HEIGHT)
            .w_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(p.space(Space::L))
            .px(p.space(Space::L))
            .bg(p.bg)
            .border_t(theme::HAIRLINE)
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
    /// Whether the window shows: hidden, no timer runs.
    visible: bool,
    tick: Option<Task<()>>,
    /// Where the slider was last laid out, for turning a pointer into a
    /// position and the duration into a step.
    bounds: Bounds<Pixels>,
    /// While the thumb is held: where it is, as a share of the track.
    dragging: Option<f32>,
    _subscriptions: [Subscription; 4],
}

impl Seek {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mirror = cx.global::<Mirror>().clone();
        let mut seek = Self {
            _subscriptions: [
                cx.observe(&mirror.playhead, |this, _, cx| this.moved(cx)),
                cx.observe(&mirror.playback, |this, _, cx| this.moved(cx)),
                // The fill is the accent.
                theme::observe_accent(cx),
                // Nothing draws the slider while the window is hidden, so
                // nothing keeps it moving; showing again puts it where it is.
                cx.observe_window_visibility(window, |this, visibility, _, cx| {
                    this.visible = visibility == WindowVisibility::Visible;
                    this.moved(cx);
                }),
            ],
            visible: window.visibility() == WindowVisibility::Visible,
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
        if !self.visible || !anchor.playing || duration == 0 {
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Paint::of(window, cx);
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
                    .top(SLIDER_HIT / 2. - theme::HAIRLINE / 2.)
                    .h(theme::HAIRLINE)
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
                .text_role(TypeRole::Fine, &p.look)
                .text_color(p.muted)
                .child(clock(ms))
        };

        div()
            .w_full()
            .flex()
            .items_center()
            .gap(p.space(Space::S))
            .child(time(shown).flex().justify_end())
            .child(slider)
            .child(time(duration))
    }
}
