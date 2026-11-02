//! The transport: a full-width bar along the foot of the window.
//!
//! Left, the record and track playing; centre, the controls over the seek
//! slider; right, the format, the sleep and output menus, and EQ.

use std::time::{Duration, Instant};

use koan_ffi::{
    DspOverview, NowPlaying, OutputChoice, PlayState, RepeatMode, SleepTimer, StreamFormat,
};
use koan_kit::app::*;
// sizes and colours computed at runtime from measurements, not chosen from the theme.
use koan_kit::raw::{Hsla, px};
// the bounds a cached view keeps between frames.
use koan_kit::raw::StyleRefinement;
// painted and hit-tested by hand: drawing koan-kit has no component for.
use koan_kit::raw::canvas;

use crate::components::{
    Badge, Button, ButtonKind, Cover, CoverArt, Glide, Level, Menu as ThemeMenu, MenuEntry,
    MenuSlot, SHADED, Slider, TransportMetrics,
};
use crate::covers::{self, CoversHandle};
use crate::engine::{command, engine};
use crate::links;
use crate::mirror::{Devices, Mirror, Outputs, Playback, Playhead, Renderers, Slice};
use crate::paint::Paint;
use crate::platform::actions::{Next, Previous, help};
use crate::theme::{self, Icon, KoanIcon, KoanStyled, Space, Tone, TypeRole};
use crate::time::clock;

pub const HEIGHT: Pixels = TransportMetrics::HEIGHT;
const COVER: Pixels = TransportMetrics::COVER;

pub struct Transport {
    /// Where F6 brings the keyboard: the bar, from which Tab reaches its
    /// controls.
    focus: FocusHandle,
    playback: Entity<Playback>,
    outputs: Entity<Outputs>,
    /// The renderer played to, for why its last track was skipped.
    renderers: Entity<Slice<Renderers>>,
    /// The device controlled, to name it while it has not said what it
    /// plays through.
    devices: Entity<Slice<Devices>>,
    /// Where the playing track's names lead.
    nav: Entity<crate::navigator::Navigator>,
    seek: Entity<Seek>,
    menus: MenuSlot<Menu>,
    /// The sleeve on show, held so the cover cache keeps it: the bar is a
    /// cached view and may not draw again for a long while.
    cover: CoverArt,
    /// What the output in use plays, for the EQ button; `None` while there
    /// are no EQ profiles, or while another device is controlled, whose
    /// presets are chosen under Output there.
    eq: Option<EqState>,
    /// The profile the playing stream named when `eq` was read.
    eq_profile: Option<String>,
    /// The library version the EQ was last read at.
    eq_library: u64,
    _eq_load: Option<Task<()>>,
    _subscriptions: [Subscription; 8],
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Menu {
    Sleep,
    Devices,
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
    /// No correction and no tuning: it holds no EQ.
    flat: bool,
    /// Processing is off: it plays untouched whatever it holds, and the
    /// button is not tinted. Its menu still shows what it holds.
    off: bool,
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
            off: !overview.enabled,
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
            _ if self.off => "EQ: processing off".into(),
            Some(preset) if self.edited => format!("EQ: {preset}, edited"),
            Some(preset) => format!("EQ: {preset}"),
            None if self.flat => "EQ".into(),
            None => "EQ: unsaved".into(),
        }
    }
}

impl Transport {
    pub fn new(
        nav: &Entity<crate::navigator::Navigator>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mirror = cx.global::<Mirror>().clone();
        let covers = cx.global::<CoversHandle>().0.clone();
        let mut this = Self {
            nav: nav.clone(),
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
                // The output menu follows both while it is open: renderers
                // arrive over the seconds after it opens.
                cx.observe(&mirror.outputs, |this, _, cx| {
                    this.refill_output(cx);
                    cx.notify();
                }),
                cx.observe(&mirror.renderers, |this, _, cx| this.refill_output(cx)),
                // The control button names the device controlled, and its
                // menu follows the devices while it is open.
                cx.observe(&mirror.devices, |this, _, cx| {
                    if let Some(menu) = this.menus.menu(Menu::Devices).cloned() {
                        let view = crate::devices::ControlView::read(cx);
                        menu.update(cx, |menu, cx| {
                            menu.refill(cx, |menu, _| crate::devices::control_menu(menu, &view))
                        });
                    }
                    cx.notify();
                }),
                // Redrawn when its own sleeve lands, not for every sleeve the
                // queue fetches as it scrolls.
                cx.observe(&covers, |this, _, cx| {
                    if !matches!(this.cover, CoverArt::Image(_)) {
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
                // Only when the library's version moves: a scan notifies
                // with every batch, and an overview reads every profile's
                // responses from disk.
                cx.observe(&mirror.library, |this, library, cx| {
                    let version = library.read(cx).version;
                    if version != this.eq_library {
                        this.eq_library = version;
                        this.reload_eq(cx);
                    }
                }),
            ],
            focus: cx.focus_handle(),
            playback: mirror.playback,
            outputs: mirror.outputs,
            renderers: mirror.renderers,
            devices: mirror.devices,
            menus: MenuSlot::default(),
            cover: CoverArt::Missing,
            eq: None,
            eq_profile: None,
            eq_library: mirror.library.read(cx).version,
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

    /// The menu `which` opens, made from what the transport shows now.
    fn build(
        &mut self,
        which: Menu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<ThemeMenu>> {
        Some(match which {
            Menu::Sleep => {
                let sleeping = self.playback.read(cx).0.sleep.is_some();
                ThemeMenu::build(window, cx, |menu, _, _| sleep_menu(menu, sleeping))
            }
            Menu::Devices => {
                let view = crate::devices::ControlView::read(cx);
                ThemeMenu::build(window, cx, |menu, _, _| {
                    crate::devices::control_menu(menu, &view)
                })
            }
            Menu::Output => {
                self.ask_outputs(cx);
                let view = self.output_view(cx);
                ThemeMenu::build(window, cx, |menu, _, _| output_menu(menu, &view))
            }
            Menu::Eq => {
                let eq = self.eq.clone()?;
                ThemeMenu::build(window, cx, move |menu, _, _| eq_menu(menu, &eq))
            }
        })
    }

    /// The device in view lists what it plays through again as its output
    /// menu opens: this one searches the network for renderers and lists its
    /// audio devices, off the main thread; a device controlled is asked over
    /// the link. What moves arrives in the Outputs and Renderers slices.
    fn ask_outputs(&self, cx: &mut Context<Self>) {
        let engine = engine(cx);
        if self.devices.read(cx).0.target.is_some() {
            cx.background_executor()
                .spawn(engine.refresh_controlled_outputs())
                .detach();
        } else {
            let refresh = engine.clone().refresh_outputs();
            cx.background_executor()
                .spawn(async move {
                    engine.search_renderers();
                    refresh.await;
                })
                .detach();
        }
    }

    /// What the output menu shows, read now.
    fn output_view(&self, cx: &App) -> OutputView {
        let devices = &self.devices.read(cx).0;
        OutputView {
            outputs: self.outputs.read(cx).0.clone(),
            problem: self
                .renderers
                .read(cx)
                .0
                .output
                .as_ref()
                .and_then(|o| o.problem.clone()),
            processed: self
                .playback
                .read(cx)
                .0
                .format
                .as_ref()
                .and_then(|f| f.dsp.as_ref())
                .map(|d| d.profile.clone()),
            controlled: devices.target.as_ref().map(|t| {
                devices
                    .devices
                    .iter()
                    .find(|d| &d.id == t)
                    .map_or_else(|| "That device".to_owned(), |d| d.name.clone())
            }),
        }
    }

    /// Makes the open output menu again from what the slices say now.
    fn refill_output(&mut self, cx: &mut Context<Self>) {
        let Some(menu) = self.menus.menu(Menu::Output).cloned() else {
            return;
        };
        let view = self.output_view(cx);
        menu.update(cx, |menu, cx| {
            menu.refill(cx, |menu, _| output_menu(menu, &view))
        });
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
        self.menus.trigger(
            which,
            text_button(icon, label, p)
                .id(label)
                .when(!enabled, |b| b.opacity(0.4)),
            |this: &mut Self| &mut this.menus,
            move |this, window, cx| enabled.then(|| this.build(which, window, cx)).flatten(),
            cx,
        )
    }

    /// The control button: the devices glyph, and while another device is
    /// controlled its name beside it in an accent badge, since everything the
    /// window shows is then that device's. A device out of reach turns the
    /// glyph into a warning in `bad`.
    fn control_button(&self, p: &Paint, cx: &mut Context<Self>) -> Div {
        let view = &self.devices.read(cx).0;
        let away = matches!(
            view.target_reach,
            Some(koan_ffi::DeviceReach::OutOfReach { .. })
        );
        let name = view.target.as_ref().map(|t| {
            view.devices
                .iter()
                .find(|d| &d.id == t)
                .map_or_else(|| "another device".to_owned(), |d| d.name.clone())
        });
        let Some(name) = name else {
            return self.menu_button(Menu::Devices, Icon::Devices, "devices", true, p, cx);
        };
        let glyph = if away {
            KoanIcon::new(Icon::Warning)
                .role(TypeRole::Body)
                .tone(Tone::Bad)
        } else {
            KoanIcon::new(Icon::Devices)
                .role(TypeRole::Body)
                .color(p.accent)
        };
        self.menus.trigger(
            Menu::Devices,
            trailing_item(p)
                .id("devices")
                .gap(p.space(Space::Xs))
                .child(glyph)
                .child(Badge::new(name).accent(!away)),
            |this: &mut Self| &mut this.menus,
            |this, window, cx| this.build(Menu::Devices, window, cx),
            cx,
        )
    }

    /// The EQ button: the glyph in `ink` while the output plays untouched,
    /// in the accent while an EQ is in force, its name in the tooltip.
    fn eq_button(&self, eq: &EqState, p: &Paint, cx: &mut Context<Self>) -> Div {
        let plain = eq.flat || eq.off;
        let color = if plain { p.ink } else { p.accent_text };
        let summary: SharedString = eq.summary().into();
        let child = if p.look.icons {
            KoanIcon::new(Icon::Filters)
                .points(TransportMetrics::trailing_icon(&p.look))
                .color(if plain { p.ink } else { p.accent })
                .into_any_element()
        } else {
            div().child("eq").into_any_element()
        };
        self.menus.trigger(
            Menu::Eq,
            trailing_item(p)
                .id("eq")
                .text_color(color)
                .hover(|s| s.opacity(0.8))
                .child(child)
                .tooltip(move |window, cx| {
                    crate::components::Tooltip::view(summary.clone(), window, cx)
                }),
            |this: &mut Self| &mut this.menus,
            |this, window, cx| this.build(Menu::Eq, window, cx),
            cx,
        )
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

/// What the output menu draws, read from the slices as it opens and again
/// whenever one of them moves while it is open.
struct OutputView {
    /// `None` while the device controlled has not said what it plays through.
    outputs: Option<koan_ffi::OutputsInfo>,
    /// Why the renderer played to skipped its last track, until one plays.
    problem: Option<String>,
    /// The EQ profile the stream playing goes through.
    processed: Option<String>,
    /// The device controlled, by name.
    controlled: Option<String>,
}

/// Where the device in view plays, as the Mac's output picker lists it: the
/// system default, each of its devices under the glyph of how it is
/// connected with its EQ under its name, then the renderers on the network;
/// the one in use on `surface`, its glyph in the accent, ticked. Each device
/// has its EQ button, in the accent while an EQ is in force there. Under the
/// renderer in use, its own volume and what it is sent.
fn output_menu(menu: ThemeMenu, view: &OutputView) -> ThemeMenu {
    let mut menu = menu.section("output");
    let Some(outputs) = &view.outputs else {
        let name = view.controlled.as_deref().unwrap_or("That device");
        return menu.note(format!(
            "{name} has not said what it plays through. It may need updating."
        ));
    };
    let choice = |entry: MenuEntry, chosen: bool, choice: OutputChoice| {
        entry.checked(chosen).on_click(move |_, _, cx| {
            let choice = choice.clone();
            command(cx, "select output", move |e| e.select_output(choice));
        })
    };
    let eq = |id: String, on: bool| {
        move |_: &mut Window, _: &mut App| {
            let device = id.clone();
            Button::new(("output-eq", id_hash(&id)), ButtonKind::Icon)
                .icon(Icon::Filters)
                .label("eq")
                .when(on, |b| b.tone(Tone::Accent))
                // The EQ of the device this row names, which need not be the
                // one in use.
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    crate::dsp::window::open(Some(device.clone()), cx);
                })
                .into_any_element()
        }
    };
    let preset = |o: &koan_ffi::OutputInfo| match (&o.preset, o.unsaved) {
        (Some(preset), true) => format!("{preset}, edited"),
        (Some(preset), false) => preset.clone(),
        (None, true) => "unsaved".into(),
        (None, false) => "flat".into(),
    };
    let here = outputs.owner.is_none();
    if let Some(owner) = &outputs.owner {
        menu = menu.item(MenuEntry::data(format!("on {owner}")).disabled(true));
    }
    menu = menu.item(choice(
        MenuEntry::new("system default")
            .icon(Icon::Output)
            .rich("whatever the system plays through"),
        outputs.current == OutputChoice::Default,
        OutputChoice::Default,
    ));
    for device in &outputs.devices {
        let id = OutputChoice::Device {
            name: device.id.clone(),
        };
        let chosen = outputs.current == id;
        let busy = device.busy && !chosen;
        let detail = if busy {
            "in use by another app".to_owned()
        } else {
            preset(device)
        };
        let shaped = device.preset.is_some() || device.unsaved;
        menu = menu.item(choice(
            MenuEntry::data(device.name.clone())
                .icon(output_icon(&device.kind))
                .rich(detail)
                .warns(busy)
                .trailing(eq(device.id.clone(), shaped)),
            chosen,
            id,
        ));
    }
    let mut playing = None;
    for renderer in &outputs.renderers {
        let id = OutputChoice::Renderer {
            udn: renderer.id.clone(),
        };
        let chosen = outputs.current == id;
        let busy = renderer.busy && !chosen;
        let problem = view.problem.as_ref().filter(|_| chosen && here);
        let detail = match (problem, busy) {
            (Some(problem), _) => problem.clone(),
            (None, true) => "in use by something else; picking it takes over".to_owned(),
            (None, false) => {
                let kind = if renderer.detail.is_empty() {
                    "upnp".to_owned()
                } else {
                    format!("{} · upnp", renderer.detail)
                };
                format!("{kind} · {}", preset(renderer))
            }
        };
        let shaped = renderer.preset.is_some() || renderer.unsaved;
        menu = menu.item(choice(
            MenuEntry::data(renderer.name.clone())
                .icon(Icon::Speaker)
                .reach(Icon::Wifi)
                .rich(detail)
                .warns(problem.is_some() || busy)
                .trailing(eq(renderer.id.clone(), shaped)),
            chosen,
            id,
        ));
        if chosen {
            playing = Some(renderer.name.clone());
        }
    }
    let Some(name) = playing else {
        return menu;
    };
    if let Some(volume) = outputs.volume {
        menu = menu.control(move |_, cx| {
            // Read as drawn, not as the menu opened: the menu stays up while
            // the volume moves.
            let volume = cx
                .global::<Mirror>()
                .outputs
                .read(cx)
                .0
                .as_ref()
                .and_then(|o| o.volume)
                .unwrap_or(volume);
            Level::new("renderer-volume", u32::from(volume), 100)
                .numbered()
                .on_commit(|to, _, cx| {
                    let to = (*to).min(100) as u8;
                    command(cx, "volume", move |e| e.set_output_volume(to));
                })
                .into_any_element()
        });
    }
    if here {
        menu = menu.note(match &view.processed {
            Some(profile) => format!(
                "{name} is sent a stream processed through \u{201C}{profile}\u{201D}. Fades don't apply."
            ),
            None => format!("{name} plays the original files. ReplayGain and fades don't apply."),
        });
    }
    menu
}

/// A stable small number for an id string, for an element id.
pub(crate) fn id_hash(id: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut h);
    h.finish()
}

/// An output's glyph by how it is connected, as the Mac's picker gives it.
fn output_icon(kind: &str) -> Icon {
    match kind {
        "builtin" => Icon::Laptop,
        "usb" => Icon::Cable,
        "bluetooth" => Icon::Headphones,
        "airplay" => Icon::Airplay,
        "display" => Icon::Display,
        _ => Icon::Speaker,
    }
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

/// How far an arrow key moves the seek bar.
const SEEK_KEY_MS: u64 = 5_000;

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
        KoanIcon::new(icon)
            .points(TransportMetrics::trailing_icon(&p.look))
            .color(p.muted)
            .hover_color(TEXT_BUTTON, ink)
            .into_any_element()
    } else {
        div().child(label).into_any_element()
    };
    trailing_item(p)
        .group(TEXT_BUTTON)
        .text_color(p.muted)
        .hover(move |s| s.text_color(ink))
        .child(child)
}

/// One of the trailing group's items, as the Mac lays them out: as wide as
/// what it shows, the full bar's height and half the gap either side to hit.
fn trailing_item(p: &Paint) -> Div {
    let reach = TransportMetrics::trailing_gap(&p.look) / 2.;
    div()
        .flex()
        .items_center()
        .justify_center()
        .h(theme::HIT)
        .px(reach)
        .mx(-reach)
        .cursor_pointer()
        .text_role(TypeRole::Meta, &p.look)
}

/// The group a text button's glyph follows the pointer by.
const TEXT_BUTTON: &str = "transport-text-button";

/// A transport control's tooltip.
fn tip(text: String) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text = SharedString::from(text);
    move |window, cx| crate::components::Tooltip::view(text.clone(), window, cx)
}

/// A transport control: `on` for the two that toggle, underlined in the
/// accent while on and `muted` while off, as an icon toggle is.
fn control(
    id: &'static str,
    icon: Icon,
    label: &'static str,
    on: Option<bool>,
    p: &Paint,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    // A stop in the window's Tab order, which Return and Space click.
    let focus = crate::components::tab_stop(&id.into(), window, cx);
    let ring = crate::components::focus_ring(&focus, window, cx);
    let (color, underline) = match on {
        Some(true) => (p.ink, Some(p.accent)),
        Some(false) => (p.muted, None),
        None => (p.ink, None),
    };
    // 44 points wide to hit, as the spec asks, but only the glyph's width in
    // the layout: the rest of the hit area reaches into the gaps beside it,
    // so the glyphs sit as far apart as the Mac's. As tall as the controls'
    // row, which the transport's metrics keep clear of the seek bar.
    let glyph = px(crate::theme::icons::side(p.look.points(TypeRole::Body)));
    let reach = (theme::HIT - glyph) / 2.;
    div()
        .id(id)
        .relative()
        .track_focus(&focus)
        .w(theme::HIT)
        .mx(-reach)
        .h_full()
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
        .children(ring)
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
        self.cover = covers::art(entry.and_then(|e| e.album_id), COVER, cx);
        let playing = now.state == PlayState::Playing;
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
            .child(
                div()
                    .id("transport-cover")
                    .flex_shrink_0()
                    .when_some(
                        entry.and_then(|e| {
                            e.album_id.map(|id| (id, e.album.clone(), e.artist.clone()))
                        }),
                        |d, (album, title, artist)| {
                            d.cursor_pointer()
                                .tooltip(tip("show artwork".to_owned()))
                                .on_click(move |_, window, cx| {
                                    crate::artwork::show(
                                        album,
                                        title.clone(),
                                        Some(artist.clone().into()),
                                        window,
                                        cx,
                                    )
                                })
                        },
                    )
                    .child(Cover::new(self.cover.clone(), COVER).id("transport-cover")),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w(px(0.))
                    // The title and the record lead to the record, each
                    // credited artist to the artist.
                    .children(entry.map(|e| {
                        let album = e.album_id.map(crate::navigator::Route::Album);
                        links::names(
                            vec![
                                links::link(
                                    "np-title".into(),
                                    e.title.clone(),
                                    album,
                                    &self.nav,
                                    p.ink,
                                )
                                .into_any_element(),
                            ],
                            TypeRole::Meta,
                            p.strong,
                            &p,
                        )
                    }))
                    .children(entry.map(|e| {
                        let mut detail = links::credit(
                            &e.artist_credit,
                            &e.artist,
                            |ix| ElementId::NamedInteger("np-artist".into(), ix as u64),
                            &self.nav,
                            p.ink,
                        );
                        if !e.album.is_empty() {
                            detail.push(
                                div()
                                    .flex_shrink_0()
                                    .child("\u{a0}·\u{a0}")
                                    .into_any_element(),
                            );
                            detail.push(
                                links::link(
                                    "np-album".into(),
                                    e.album.clone(),
                                    e.album_id.map(crate::navigator::Route::Album),
                                    &self.nav,
                                    p.ink,
                                )
                                .into_any_element(),
                            );
                        }
                        links::names(detail, TypeRole::Fine, p.muted, &p)
                    })),
            );

        // The one control reached for without looking: outlined, as the
        // Mac's is.
        let play_pause = Button::new("play-pause", ButtonKind::IconOutlined)
            .icon(if playing { Icon::Pause } else { Icon::Play })
            .label(if playing { "pause" } else { "play" })
            .on_click(|_, _, cx| command(cx, "play/pause", |e| e.toggle_play_pause()));

        // The controls and the seek bar placed by koan-kit's metrics: centred
        // in the bar as the Mac's, the controls' hit row never reaching the
        // seek row or above the rule.
        let rows = TransportMetrics::centre_rows(&p.look);
        let centre = div()
            .w(TransportMetrics::CENTRE)
            .h_full()
            .relative()
            .child(
                div()
                    .absolute()
                    .top(rows.controls_top)
                    .left_0()
                    .right_0()
                    .h(rows.controls_height)
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(TransportMetrics::control_gap(&p.look))
                    .child(
                        control(
                            "shuffle",
                            Icon::Shuffle,
                            "shuffle",
                            Some(shuffle),
                            &p,
                            window,
                            cx,
                        )
                        .on_click(move |_, _, cx| {
                            command(cx, "shuffle", move |e| e.set_shuffle(!shuffle));
                        }),
                    )
                    .child(
                        control("previous", Icon::Previous, "prev", None, &p, window, cx)
                            .tooltip(tip(help("previous track", &Previous, cx)))
                            .on_click(|_, _, cx| command(cx, "previous", |e| e.previous())),
                    )
                    .child(play_pause)
                    .child(
                        control("next", Icon::Next, "next", None, &p, window, cx)
                            .tooltip(tip(help("next track", &Next, cx)))
                            .on_click(|_, _, cx| command(cx, "next", |e| e.next())),
                    )
                    .child(
                        control(
                            "repeat",
                            repeat_icon,
                            repeat_label,
                            Some(now.repeat_mode != RepeatMode::Off),
                            &p,
                            window,
                            cx,
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
                div()
                    .absolute()
                    .top(rows.seek_top)
                    .left_0()
                    .right_0()
                    .h(TransportMetrics::SEEK)
                    .child(
                        self.seek.clone().cached(
                            StyleRefinement::default()
                                .w_full()
                                .h(TransportMetrics::SEEK),
                        ),
                    ),
            );

        let right = div()
            .flex_1()
            .min_w(px(0.))
            .flex()
            .items_center()
            .justify_end()
            .gap(TransportMetrics::trailing_gap(&p.look))
            .children(
                now.format
                    .as_ref()
                    .map(|format| Badge::new(format_badge(format))),
            )
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
            .child(self.control_button(&p, cx))
            .child(self.menu_button(Menu::Output, Icon::Speaker, "output", true, &p, cx))
            // This device's own output's EQ; another device's is chosen there.
            .children(
                self.eq
                    .clone()
                    .filter(|_| {
                        self.outputs
                            .read(cx)
                            .0
                            .as_ref()
                            // Only this device's outputs: with another device
                            // controlled, or its outputs not yet heard, a
                            // preset chosen here would name this device's
                            // output to the other one.
                            .is_some_and(|o| o.owner.is_none())
                    })
                    .map(|eq| self.eq_button(&eq, &p, cx)),
            );

        let ring = crate::components::focus_ring(&self.focus, window, cx);
        div()
            .id("transport")
            .relative()
            .track_focus(&self.focus)
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
            .children(ring)
    }
}

impl Focusable for Transport {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// The seek slider and the times either side of it.
///
/// The position is reckoned from the playhead's anchor. Where the renderer
/// draws shaders, the slider glides: the anchor is handed to it (`Glide`) and
/// the GPU reckons the fill per present, so it moves continuously with no
/// frame drawn, and a new anchor (a seek, a pause, the next track) corrects
/// it. While it runs, one timer is set for the next moment the times would
/// read differently (the next second), and elsewhere for the fill's next
/// pixel too; paused or stopped, nothing is set and nothing presents, so a
/// koan at rest costs nothing.
pub struct Seek {
    playhead: Entity<Playhead>,
    playback: Entity<Playback>,
    /// Whether the window shows: hidden, no timer runs.
    visible: bool,
    tick: Option<Task<()>>,
    /// While the thumb is held: where it is, as a share of the track.
    dragging: Option<f32>,
    /// The slider's width as last painted, for when its fill next moves a
    /// pixel.
    width: std::rc::Rc<std::cell::Cell<f32>>,
    glide: Glide,
    _subscriptions: Vec<Subscription>,
}

impl Seek {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mirror = cx.global::<Mirror>().clone();
        let mut subscriptions = vec![
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
        ];
        // The playing track's download starting or ending shows or takes
        // away how far its bytes reach.
        if let Some(transfers) = crate::transfers::entity(cx) {
            subscriptions.push(cx.observe(&transfers, |_, _, cx| cx.notify()));
        }
        let mut seek = Self {
            _subscriptions: subscriptions,
            visible: window.visibility() == WindowVisibility::Visible,
            playhead: mirror.playhead,
            playback: mirror.playback,
            tick: None,
            dragging: None,
            width: Default::default(),
            glide: Glide::new(),
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
        let moving = self.visible && anchor.playing && duration > 0 && self.dragging.is_none();
        if duration > 0 {
            self.glide.set(
                (anchor.position_ms as f64 / duration as f64) as f32,
                if anchor.playing {
                    1000.0 / duration as f32
                } else {
                    0.0
                },
                anchor.at,
            );
        }
        self.glide.set_moving(moving);
        if !self.visible || !anchor.playing || duration == 0 {
            return;
        }
        let position = anchor.at(Instant::now(), duration);
        if position >= duration {
            return;
        }
        let to_second = 1000 - position % 1000;
        let width = self.width.get();
        let to_pixel = if width > 0.0 && !SHADED {
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
        self.schedule(cx);
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
        let (change, commit) = (cx.entity(), cx.entity());
        let _ = p;
        // The playhead the slider shows comes from the engine, which holds a
        // seek's target until it lands there (`SharedPlayerState::request_seek`,
        // and koan-ffi's `remote_seek` for a device being controlled), so a
        // click never shows the old place again before the new one.
        let width = self.width.clone();
        let measure = canvas(
            move |bounds, _, _| width.set(f32::from(bounds.size.width)),
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        // How far a track still downloading can be played, moving as its
        // bytes arrive; nothing once the file is all here.
        let reach = self
            .playback
            .read(cx)
            .0
            .entry
            .as_ref()
            .and_then(|e| e.track_id)
            .and_then(|track| crate::transfers::reach(track, cx));
        let slider = Slider::new("seek", share)
            .compact()
            // An arrow key moves five seconds, not a twentieth of the track.
            .when(duration > 0, |s| {
                s.step((SEEK_KEY_MS as f32 / duration as f32).min(1.0))
            })
            .when(self.dragging.is_none(), |slider| slider.glide(&self.glide))
            .when_some(reach, |slider, reach| slider.reach(&reach))
            .ends(clock(shown), clock(duration))
            .disabled(duration == 0)
            .on_change(move |share, _, cx| {
                change.update(cx, |this, cx| {
                    let started = this.dragging.replace(*share).is_none();
                    if started {
                        this.schedule(cx);
                    }
                    cx.notify();
                })
            })
            .on_commit(move |_, _, cx| commit.update(cx, |this, cx| this.commit(cx)));
        // The slider's line on the row's middle, where the controls are
        // placed from.
        div()
            .relative()
            .size_full()
            .flex()
            .items_center()
            .child(div().w_full().child(slider))
            .child(measure)
    }
}
