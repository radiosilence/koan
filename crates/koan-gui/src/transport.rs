//! The transport: a full-width bar along the foot of the window.
//!
//! Left, the record and track playing; centre, the controls over the seek
//! slider; right, the format, the sleep and output menus, and EQ.

use std::time::Instant;

use koan_kit::app::*;
use koan_runtime::{
    DspOverview, NowPlaying, OutputChoice, PlayState, RepeatMode, SleepTimer, StreamFormat,
};

use crate::components::{
    Badge, Button, Cover, CoverArt, Glide, Level, Menu as ThemeMenu, MenuEntry, MenuSlot, Slider,
    TransportControl, TransportMetrics,
};
use crate::covers::{self, CoversHandle};
use crate::engine::{command, engine};
use crate::links;
use crate::mirror::{Devices, Mirror, Outputs, Playback, Playhead, Renderers, Slice};
use crate::paint::Paint;
use crate::platform::actions::{Next, Previous, ShowEq, help};
use crate::theme::{self, Icon, KoanIcon, KoanStyled, Space, Tone, TypeRole};
use crate::time::clock;

pub const HEIGHT: Pixels = TransportMetrics::HEIGHT;

#[path = "transport_phone.rs"]
mod phone;
pub use phone::{Collapse, Expand, Form, MINI_HEIGHT, ShowInfo};
const COVER: Pixels = TransportMetrics::COVER;

pub struct Transport {
    /// Where F6 brings the keyboard: the bar, from which Tab reaches its
    /// controls.
    focus: FocusHandle,
    /// What had the keyboard when the pointer went down on the bar, given
    /// back once the control under it has taken the focus.
    before_press: std::rc::Rc<std::cell::RefCell<Option<FocusHandle>>>,
    playback: Entity<Playback>,
    outputs: Entity<Outputs>,
    /// The renderer played to, for why its last track was skipped.
    renderers: Entity<Slice<Renderers>>,
    /// The device controlled, to name it while it has not said what it
    /// plays through.
    devices: Entity<Slice<Devices>>,
    /// What the control button shows of the devices, last drawn.
    controlled_shown: ControlledShown,
    /// Where the playing track's names lead.
    nav: Entity<crate::navigator::Navigator>,
    seek: Entity<Seek>,
    menus: MenuSlot<Menu>,
    /// The sleeve on show, held so the cover cache keeps it: the bar is a
    /// cached view and may not draw again for a long while.
    cover: CoverArt,
    /// The library's favourites, for the playing track's heart, once the
    /// library has loaded.
    feed: Option<(Entity<crate::library::feed::Feed>, Subscription)>,
    /// What the library acts through, for Now Playing's menu of the track.
    lib: Option<crate::library::Lib>,
    /// The bar's width as last laid out, which decides what of the trailing
    /// group it has room for.
    width: std::rc::Rc<std::cell::Cell<Option<Pixels>>>,
    /// The display's scale as of the last frame, which sizes the sleeve.
    scale: f32,
    /// What the output in use plays, for the EQ button; `None` while there
    /// are no EQ profiles, or while another device is controlled, whose
    /// presets are chosen under Output there.
    eq: Option<EqState>,
    /// The profile the playing stream named when `eq` was read.
    eq_profile: Option<String>,
    /// The library version the EQ was last read at.
    eq_library: u64,
    _eq_load: Option<Task<()>>,
    /// The bar, or on a phone the mini player or Now Playing.
    form: Form,
    /// The phone's forms follow the playhead themselves.
    playhead_followed: Option<Subscription>,
    /// Now Playing shows the lyrics in place of the art.
    lyrics_shown: bool,
    lyrics: Option<Entity<crate::lyrics::LyricsPanel>>,
    /// The mini player's playhead, gliding on the GPU.
    mini_glide: crate::components::Glide,
    /// How far Now Playing has been pulled down by a finger, while it is.
    pull: Option<Pixels>,
    /// A pull let go: whether it closes Now Playing (held there while the
    /// screen goes down) or eases back up.
    released: Option<bool>,
    /// The pull's offset, which the GPU composes the page at: held by the
    /// finger, eased back up on a release short of closing.
    pulled: crate::components::Follow,
    /// The ease back up, letting the page's layer go once it is in place.
    returning: Option<Task<()>>,
    /// How far a finger has pushed the mini player up, while it does.
    lift: Option<Pixels>,
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
                cx.observe(&mirror.devices, |this, devices, cx| {
                    if let Some(menu) = this.menus.menu(Menu::Devices).cloned() {
                        let view = crate::devices::ControlView::read(cx);
                        menu.update(cx, |menu, cx| {
                            menu.refill(cx, |menu, _| crate::devices::control_menu(menu, &view))
                        });
                    }
                    // Other devices report their playheads through this
                    // slice; the bar redraws only when what it shows moves.
                    let shown = controlled_shown(&devices.read(cx).0);
                    if shown != this.controlled_shown {
                        this.controlled_shown = shown;
                        cx.notify();
                    }
                }),
                // Redrawn when its own sleeve lands, or when the cache has
                // dropped the one shown from the GPU, not for every sleeve
                // the queue fetches as it scrolls.
                cx.observe(&covers, |this, _, cx| {
                    let album = this
                        .playback
                        .read(cx)
                        .0
                        .entry
                        .as_ref()
                        .and_then(|e| e.album_id);
                    let held = covers::image(album, COVER, this.scale, cx);
                    let moved = match (&this.cover, &held) {
                        (CoverArt::Image(shown), Some(held)) => {
                            !std::sync::Arc::ptr_eq(shown, held)
                        }
                        (CoverArt::Image(_), None) => true,
                        (_, held) => held.is_some(),
                    };
                    if moved {
                        cx.notify();
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
            before_press: Default::default(),
            playback: mirror.playback,
            outputs: mirror.outputs,
            renderers: mirror.renderers,
            controlled_shown: controlled_shown(&mirror.devices.read(cx).0),
            devices: mirror.devices,
            menus: MenuSlot::default(),
            cover: CoverArt::Missing,
            feed: None,
            lib: None,
            width: Default::default(),
            scale: 2.0,
            eq: None,
            eq_profile: None,
            eq_library: mirror.library.read(cx).version,
            _eq_load: None,
            form: Form::Bar,
            playhead_followed: None,
            lyrics_shown: false,
            lyrics: None,
            mini_glide: crate::components::Glide::new(),
            pull: None,
            released: None,
            pulled: crate::components::Follow::new(),
            returning: None,
            lift: None,
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
            blocked: self.renderers.read(cx).0.blocked,
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

    #[allow(clippy::too_many_arguments)]
    fn menu_button(
        &self,
        which: Menu,
        icon: Icon,
        label: &'static str,
        help: String,
        enabled: bool,
        p: &Paint,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        self.menus.trigger(
            which,
            text_button(icon, label, p, window, cx)
                .tooltip(tip(help))
                .when(!enabled, |b| b.opacity(0.4)),
            |this: &mut Self| &mut this.menus,
            move |this, window, cx| enabled.then(|| this.build(which, window, cx)).flatten(),
            cx,
        )
    }

    /// The playing track's heart reads and toggles the library's
    /// favourites, and follows them as they change elsewhere.
    /// The library, for Now Playing's menu of the playing track.
    pub fn set_lib(&mut self, lib: crate::library::Lib) {
        self.lib = Some(lib);
    }

    pub fn set_feed(&mut self, feed: Entity<crate::library::feed::Feed>, cx: &mut Context<Self>) {
        let observed = cx.observe(&feed, |_, _, cx| cx.notify());
        self.feed = Some((feed, observed));
        cx.notify();
    }

    /// The device controlled, by name, while one is.
    fn controlled_name(&self, cx: &App) -> Option<String> {
        let view = &self.devices.read(cx).0;
        view.target.as_ref().map(|t| {
            view.devices
                .iter()
                .find(|d| &d.id == t)
                .map_or_else(|| "another device".to_owned(), |d| d.name.clone())
        })
    }

    /// The control button: the devices glyph, and while another device is
    /// controlled its name beside it in an accent badge, since everything the
    /// window shows is then that device's. A device out of reach turns the
    /// glyph into a warning in `bad`.
    fn control_button(&self, p: &Paint, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let away = matches!(
            self.devices.read(cx).0.target_reach,
            Some(koan_runtime::DeviceReach::OutOfReach { .. })
        );
        let Some(name) = self.controlled_name(cx) else {
            return self.menu_button(
                Menu::Devices,
                Icon::Devices,
                "devices",
                "play on another device".to_owned(),
                true,
                p,
                window,
                cx,
            );
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
            trailing_item(div().id("devices"), p, false)
                .tooltip(tip(if away {
                    format!("{name} is out of reach")
                } else {
                    format!("playing on {name}")
                }))
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
    fn eq_button(
        &self,
        eq: &EqState,
        p: &Paint,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let plain = eq.flat || eq.off;
        // The glyph, or with icons off the word, in `ink` or the accent:
        // quieter under the pointer, back over `LEAVE` once it has gone.
        let rest = match (plain, p.look.icons) {
            (true, _) => p.ink,
            (false, true) => p.accent,
            (false, false) => p.accent_text,
        };
        let (base, color) =
            crate::components::hover_tint("eq", rest, rest.opacity(0.8), window, cx);
        let summary: SharedString = help(&eq.summary(), &ShowEq, cx).into();
        let child = if p.look.icons {
            KoanIcon::new(Icon::Filters)
                .points(TransportMetrics::trailing_icon(&p.look))
                .color(color)
                .into_any_element()
        } else {
            div().child("eq").into_any_element()
        };
        self.menus.trigger(
            Menu::Eq,
            trailing_item(base, p, p.look.icons)
                .text_color(color)
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
    outputs: Option<koan_runtime::OutputsInfo>,
    /// Why the renderer played to skipped its last track, until one plays.
    problem: Option<String>,
    /// The EQ profile the stream playing goes through.
    processed: Option<String>,
    /// The device controlled, by name.
    controlled: Option<String>,
    /// macOS refused the last search for renderers the local network.
    blocked: bool,
}

/// Where the device in view plays, as the Mac's output picker lists it: the
/// system default, each of its devices under the glyph of how it is
/// connected with its EQ under its name, then the renderers on the network;
/// the one in use on `surface`, its glyph in the accent, ticked. Each device
/// has its EQ button, in the accent while an EQ is in force there. Under the
/// renderer in use, its own volume and what it is sent.
fn output_menu(menu: ThemeMenu, view: &OutputView) -> ThemeMenu {
    let menu = menu.width(ThemeMenu::PICKER_WIDTH);
    let mut menu = menu.section("output");
    let Some(outputs) = &view.outputs else {
        // This device's own outputs are on their way; another device that
        // never says is one from before outputs were reported.
        return menu.note(match view.controlled.as_deref() {
            Some(name) => {
                format!("{name} has not said what it plays through. It may need updating.")
            }
            None => "This device's outputs are still being read.".to_owned(),
        });
    };
    let choice = |entry: MenuEntry, chosen: bool, choice: OutputChoice| {
        entry
            .checked(chosen)
            .stays_open()
            .on_click(move |_, _, cx| {
                let choice = choice.clone();
                command(cx, "select output", move |e| e.select_output(choice));
            })
    };
    // The EQ of the device a row names, which need not be the one in use;
    // lit while it has one.
    let eq = |entry: MenuEntry, id: String, on: bool| {
        entry.lit_action(Icon::Filters, "eq for this output", on, move |_, _, cx| {
            crate::settings::open_eq(Some(id.clone()), cx);
        })
    };
    let preset = |o: &koan_runtime::OutputInfo| match (&o.preset, o.unsaved) {
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
            eq(
                MenuEntry::data(device.name.clone())
                    .icon(output_icon(&device.kind))
                    .rich(detail)
                    .warns(busy),
                device.id.clone(),
                shaped,
            ),
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
            eq(
                MenuEntry::data(renderer.name.clone())
                    .icon(Icon::Speaker)
                    .reach(Icon::Wifi)
                    .rich(detail)
                    .warns(problem.is_some() || busy),
                renderer.id.clone(),
                shaped,
            ),
            chosen,
            id,
        ));
        if chosen {
            playing = Some(renderer.name.clone());
        }
    }
    if here && view.blocked && outputs.renderers.is_empty() {
        menu = menu.note(blocked_note());
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

/// Why no network speaker shows while macOS keeps the app off the local
/// network: whom to allow, which for a binary started from a terminal is the
/// terminal, since the permission is the app's that started it.
fn blocked_note() -> String {
    let bundled = std::env::current_exe()
        .is_ok_and(|exe| exe.to_string_lossy().contains(".app/Contents/MacOS/"));
    if bundled {
        "macOS isn't letting kōan on the local network, so no network speaker can be found. Allow it in System Settings › Privacy & Security › Local Network.".to_owned()
    } else {
        "macOS isn't letting kōan on the local network, so no network speaker can be found. Started outside its app, it has the permission of the app that started it: allow that app in System Settings › Privacy & Security › Local Network, or run kōan from its bundle.".to_owned()
    }
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
/// which goes back to it. Edit… opens EQ on the device, in Settings.
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
            .on_click(move |_, _, cx| crate::settings::open_eq(Some(device.clone()), cx)),
    )
}

/// How far an arrow key moves the seek bar.
const SEEK_KEY_MS: u64 = 5_000;

/// The theme's text button, or with icons its icon button: `muted`, `ink`
/// under the pointer, leaving over `LEAVE` as the kit's controls do.
fn text_button(
    icon: Icon,
    label: &'static str,
    p: &Paint,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    let rest = if p.look.touch { p.ink } else { p.muted };
    let (base, color) = crate::components::hover_tint(label, rest, p.ink, window, cx);
    let child = if p.look.icons {
        KoanIcon::new(icon)
            .points(TransportMetrics::trailing_icon(&p.look))
            .color(color)
            .into_any_element()
    } else {
        div().child(label).into_any_element()
    };
    trailing_item(base, p, p.look.icons)
        .text_color(color)
        .child(child)
}

/// The trailing group's items, by what they are.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Trailing {
    Format,
    Sleep,
    Devices,
    Output,
    Eq,
}

impl Trailing {
    /// What a bar too narrow for all of them gives up first: the format,
    /// read elsewhere; the sleep timer and the EQ, set now and then; the
    /// device played on; the output last, where the sound goes.
    fn drop_order() -> [Trailing; 5] {
        [
            Self::Format,
            Self::Sleep,
            Self::Eq,
            Self::Devices,
            Self::Output,
        ]
    }
}

/// The least the record's side keeps beside its sleeve, at 100 %: a few
/// letters of its title and its credit.
const LEFT_TEXT_LEAST: f32 = 56.0;
/// The least the centre gives way to, at 100 %: the controls, and a seek bar
/// with its times either side.
const CENTRE_LEAST: f32 = 300.0;

/// One of the trailing group's items, as the Mac lays them out: as wide as
/// what it shows, the full bar's height and half the gap either side to hit.
/// A glyph alone reaches further, over the gap, so it is at least `HIT` wide
/// to click: padding taken back by an equal negative margin, so the item's
/// own bounds grow and nothing moves.
fn trailing_item(base: Stateful<Div>, p: &Paint, glyph_only: bool) -> Stateful<Div> {
    let gap = TransportMetrics::trailing_gap(&p.look) / 2.;
    let glyph = Pixels::from(TransportMetrics::trailing_icon(&p.look));
    let reach = if glyph_only {
        gap.max((theme::HIT - glyph) / 2.)
    } else {
        gap
    };
    // A glyph's reach can pass its neighbour's at small gaps: each item
    // blocks the ones drawn before it, so a press in the overlap goes to
    // one item, never two.
    base.flex()
        .items_center()
        .justify_center()
        .h(theme::HIT)
        .px(reach)
        .mx(-reach)
        .occlude()
        .cursor_pointer()
        .text_role(TypeRole::Meta, &p.look)
}

/// A transport control's tooltip.
fn tip(text: String) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text = SharedString::from(text);
    move |window, cx| crate::components::Tooltip::view(text.clone(), window, cx)
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
        match self.form {
            Form::Mini => return self.mini(window, cx),
            Form::Full => return self.full(window, cx),
            Form::Bar => {}
        }
        let p = Paint::of(window, cx);
        let now: NowPlaying = self.playback.read(cx).0.clone();
        let entry = now.entry.as_ref();
        self.scale = window.scale_factor();
        self.cover = covers::art(entry.and_then(|e| e.album_id), COVER, self.scale, cx);
        // A skip to a track still arriving reports `Stopped` while it waits,
        // and opens playing: it is playing, so play and pause do not fold
        // back and forth for it.
        let playing =
            now.state == PlayState::Playing || (now.state == PlayState::Stopped && now.waiting);
        let left = div()
            .flex_1()
            .min_w_0()
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
                    // Announced, without moving focus, as the track changes:
                    // a leaf holding only the words, since a live region's
                    // descendants are live too, and the links below would be
                    // announced again. VoiceOver reads a live node's value,
                    // Orca its name, so it carries both.
                    .when(p.a11y, |d| {
                        let spoken = crate::components::spoken(
                            entry
                                .map(|e| [e.title.as_str(), e.artist.as_str(), e.album.as_str()])
                                .unwrap_or(["nothing playing", "", ""]),
                        );
                        d.child(
                            div()
                                .id("now-playing")
                                .role(a11y::Role::Status)
                                .aria_label(spoken.clone())
                                .aria_value(spoken)
                                .aria_live(a11y::Live::Polite)
                                .absolute()
                                .size_0(),
                        )
                    })
                    .flex()
                    .flex_col()
                    .min_w_0()
                    // The title and the record lead to the record, each
                    // credited artist to the artist.
                    .children(entry.map(|e| {
                        let album = e.album_id.map(crate::navigator::Route::Album);
                        let names = links::names(
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
                        );
                        // The rainbow's shimmer through the playing title,
                        // over it while the rainbow is drawn; the title
                        // otherwise stands as it is.
                        let title = match crate::theme::KoanTheme::prism(cx) {
                            Some(brat) => div()
                                .relative()
                                .min_w_0()
                                .child(names)
                                .child(
                                    crate::components::Sheen::new("np-title-sheen")
                                        .brat(brat)
                                        .strength(0.35)
                                        .glitch(true),
                                )
                                .into_any_element(),
                            None => names.into_any_element(),
                        };
                        // The playing track's heart, as the Mac's transport
                        // has it: kept when the title is cut, and only for a
                        // track the library holds.
                        let heart = e
                            .track_id
                            .zip(self.feed.as_ref())
                            .map(|(track, (feed, _))| {
                                let item = crate::library::drag::Playable::Track(track);
                                let on = feed.read(cx).is_favourite(item);
                                let feed = feed.clone();
                                div().flex_shrink_0().child(
                                    // Keyed by the track, so its answer plays for a toggle, not
                                    // for the next track's heart standing otherwise.
                                    crate::components::Heart::new(("np-heart", track as u64), on)
                                        .role(TypeRole::Meta)
                                        .on_click(move |_, _, cx| {
                                            feed.update(cx, |feed, cx| {
                                                feed.toggle_favourite(item, cx)
                                            })
                                        }),
                                )
                            });
                        div()
                            .flex()
                            .items_center()
                            .gap(p.space(Space::Xs))
                            .min_w_0()
                            .child(div().min_w_0().child(title))
                            .children(heart)
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
        let play_pause = Button::icon_outlined(
            "play-pause",
            if playing { Icon::Pause } else { Icon::Play },
            help(
                if playing { "pause" } else { "play" },
                &crate::platform::actions::TogglePlayPause,
                cx,
            ),
        )
        .on_click(|_, _, cx| command(cx, "play/pause", |e| e.toggle_play_pause()));

        // The trailing items, each with the width it takes, in the order a
        // narrow bar gives them up (`Trailing::drop_order`).
        let sleeping = now.sleep.is_some();
        let format = now.format.as_ref().map(format_badge);
        let controlled = self.controlled_name(cx);
        // Each item's width, worked out before any is made.
        let (format_width, sleep_width, devices_width, output_width, eq_width) = {
            let words = |text: &str, role: TypeRole| {
                crate::components::text_widths(
                    &[SharedString::from(text.to_owned())],
                    role,
                    window,
                    cx,
                )
                .first()
                .copied()
                .unwrap_or_default()
            };
            let glyph = Pixels::from(TransportMetrics::trailing_icon(&p.look));
            let face = |label: &str| {
                if p.look.icons {
                    glyph
                } else {
                    words(label, TypeRole::Meta)
                }
            };
            (
                format.as_ref().map(|text| {
                    words(text, TypeRole::Meta) + p.look.spaced(12.0) * 2.0 + p.look.hairline * 2.0
                }),
                face(if sleeping { "sleeping" } else { "sleep" }),
                match &controlled {
                    // The glyph and the device's name in a badge.
                    Some(name) => {
                        Pixels::from(crate::theme::icons::side(p.look.points(TypeRole::Body)))
                            + p.space(Space::Xs)
                            + words(name, TypeRole::Fine)
                            + Pixels::from(14.0)
                            + p.look.hairline * 2.0
                    }
                    None => face("devices"),
                },
                face("output"),
                face("eq"),
            )
        };
        let mut trailing: Vec<(Trailing, Pixels, AnyElement)> = Vec::new();
        if let (Some(text), Some(width)) = (format, format_width) {
            trailing.push((Trailing::Format, width, Badge::new(text).into_any_element()));
        }
        trailing.push((
            Trailing::Sleep,
            sleep_width,
            self.menu_button(
                Menu::Sleep,
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
            )
            .into_any_element(),
        ));
        trailing.push((
            Trailing::Devices,
            devices_width,
            self.control_button(&p, window, cx).into_any_element(),
        ));
        trailing.push((
            Trailing::Output,
            output_width,
            self.menu_button(
                Menu::Output,
                Icon::Speaker,
                "output",
                "output".to_owned(),
                true,
                &p,
                window,
                cx,
            )
            .into_any_element(),
        ));
        // This device's own output's EQ; another device's is chosen there.
        if let Some(eq) = self.eq.clone().filter(|_| {
            self.outputs
                .read(cx)
                .0
                .as_ref()
                // Only this device's outputs: with another device controlled,
                // or its outputs not yet heard, a preset chosen here would
                // name this device's output to the other one.
                .is_some_and(|o| o.owner.is_none())
        }) {
            trailing.push((
                Trailing::Eq,
                eq_width,
                self.eq_button(&eq, &p, window, cx).into_any_element(),
            ));
        }

        // The three columns keep their contents. The centre stands at its
        // full width while the sides have room, and gives way down to its
        // least; the record's side keeps its sleeve and a line's start, its
        // names cut; the trailing items keep their size. Narrower still, the
        // trailing items go in `drop_order`, never all at once.
        let trailing_gap = TransportMetrics::trailing_gap(&p.look);
        // The sleeve, a line's start and the heart beside it.
        let left_least = COVER
            + p.space(Space::M)
            + p.look.sized(LEFT_TEXT_LEAST)
            + Pixels::from(crate::theme::icons::side(p.look.points(TypeRole::Meta)))
            + p.space(Space::Xs);
        let centre_least = p.look.sized(CENTRE_LEAST);
        let need = |items: &[(Trailing, Pixels, AnyElement)]| {
            let widths: Pixels = items.iter().map(|(_, w, _)| *w).sum();
            widths + trailing_gap * items.len().saturating_sub(1) as f32
        };
        let inner = self.width.get().map(|w| w - p.space(Space::L) * 4.0);
        if let Some(inner) = inner {
            for kind in Trailing::drop_order() {
                if left_least + centre_least + need(&trailing) <= inner {
                    break;
                }
                trailing.retain(|(k, ..)| *k != kind);
            }
        }
        let centre_width = inner.map_or(TransportMetrics::CENTRE, |inner| {
            (inner - left_least - need(&trailing))
                .min(TransportMetrics::CENTRE)
                .max(centre_least)
        });
        let right = div()
            .flex_1()
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_end()
            .gap(trailing_gap)
            .children(trailing.into_iter().map(|(_, _, item)| item));

        // The controls and the seek bar placed by koan-kit's metrics: centred
        // in the bar as the Mac's, the controls' hit row never reaching the
        // seek row or above the rule.
        let rows = TransportMetrics::centre_rows(&p.look);
        let centre = div()
            .flex_shrink_0()
            .w(centre_width)
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
                    .children(controls(&now, play_pause, cx)),
            )
            // Its own cached view. A second of the clock re-renders the slider and the views
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

        let ring = crate::components::focus_ring(&self.focus, window, cx);
        div()
            .id("transport")
            .relative()
            .track_focus(&self.focus)
            // F6 brings the keyboard here; a click on a control leaves it
            // where it was, with the list it was working. The control takes
            // the focus as the press bubbles up from it, before the bar
            // hears it, so the bar notes the focus on the way down and gives
            // it back on the way up. Preventing the default instead would
            // also keep the controls from drawing as pressed.
            .capture_any_mouse_down({
                let before = self.before_press.clone();
                // The left button alone: a right press opens a menu that
                // takes the keyboard on purpose.
                move |e: &MouseDownEvent, window, cx| {
                    *before.borrow_mut() = (e.button == MouseButton::Left)
                        .then(|| window.focused(cx))
                        .flatten();
                }
            })
            .on_any_mouse_down({
                let (before, bar) = (self.before_press.clone(), self.focus.clone());
                move |_, window, cx| {
                    if let Some(before) = before.borrow_mut().take()
                        && !bar.contains(&before, window)
                    {
                        window.focus(&before, cx);
                        // The bar's own focus-on-press runs after this, on a
                        // press no control took (a gap, a link, the cover).
                        window.prevent_default();
                    }
                }
            })
            .h(HEIGHT)
            .w_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(p.space(Space::L))
            .px(p.space(Space::L))
            // No ground: the shell draws it over the pages, on their frost.
            // The width the bar was laid out at, for the next frame to fit
            // the trailing group to.
            .child(crate::components::width_probe(
                self.width.clone(),
                cx.entity_id(),
            ))
            // Its sleeve and a line's start, whatever the trailing group.
            .child(left.min_w(left_least))
            .child(centre)
            .child(right)
            .children(ring)
            .into_any_element()
    }
}

/// The transport's five controls, in the bar's order: shuffle, previous,
/// `play_pause`, next, repeat. The desktop bar, the phone's Now Playing.
fn controls(now: &NowPlaying, play_pause: impl IntoElement, cx: &App) -> Vec<AnyElement> {
    let shuffle = now.shuffle;
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
    vec![
        TransportControl::new(
            "shuffle",
            Icon::Shuffle,
            "shuffle",
            theme::label(if shuffle {
                "shuffle: on"
            } else {
                "shuffle: off"
            }),
        )
        .on(shuffle)
        .on_click(move |_, _, cx| {
            command(cx, "shuffle", move |e| e.set_shuffle(!shuffle));
        })
        .into_any_element(),
        TransportControl::new(
            "previous",
            Icon::Previous,
            "prev",
            help("previous track", &Previous, cx),
        )
        .answer(crate::components::Answer::Nudge(false))
        .on_click(|_, _, cx| command(cx, "previous", |e| e.previous()))
        .into_any_element(),
        play_pause.into_any_element(),
        TransportControl::new("next", Icon::Next, "next", help("next track", &Next, cx))
            .answer(crate::components::Answer::Nudge(true))
            .on_click(|_, _, cx| command(cx, "next", |e| e.next()))
            .into_any_element(),
        TransportControl::new(
            "repeat",
            repeat_icon,
            repeat_label,
            theme::label(match now.repeat_mode {
                RepeatMode::Off => "repeat: off",
                RepeatMode::Queue => "repeat: the queue",
                RepeatMode::One => "repeat: this track",
            }),
        )
        .on(now.repeat_mode != RepeatMode::Off)
        .on_click(move |_, _, cx| command(cx, "repeat", move |e| e.set_repeat(next_repeat)))
        .into_any_element(),
    ]
}

impl Focusable for Transport {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// How near the engine's whole-second reading must come to the anchor's
/// reckoning to be of the same place.
const NEAR_MS: u64 = 250;

/// Where the times put the playhead: reckoned from the anchor shown, or at
/// the engine's whole second its own reading, so the times step as it says
/// rather than a moment behind. A reading counts only while it agrees with
/// the anchor, within `NEAR_MS`: one from before a seek (backward too), or
/// one in flight as a new anchor arrives, is of another place.
fn shown_position(
    reckoned: u64,
    heard: Option<(u64, Instant)>,
    playing: bool,
    now: Instant,
    duration: u64,
) -> u64 {
    match heard {
        Some((heard, at)) if playing => {
            let heard = (heard + now.duration_since(at).as_millis() as u64).min(duration);
            if heard.abs_diff(reckoned) < NEAR_MS {
                reckoned.max(heard)
            } else {
                reckoned
            }
        }
        _ => reckoned,
    }
}

/// The thumb while the person has it. Held, it follows the pointer. Let go
/// on this device, it stands where it was let go until the engine answers
/// the seek: the first playhead whose `seek` count is past `after`. Let go
/// while another device is controlled, whose reports answer no seek, it
/// stands, marked pending, only until the command is sent (`sent`, the
/// release it belongs to), and the slider then shows what that device
/// reports. What the slider shows otherwise is the engine's playhead, and
/// nothing else: no position is predicted.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Thumb {
    Held(f32),
    LetGo { share: f32, after: u64 },
    Sending { share: f32, sent: u64 },
}

impl Thumb {
    fn share(self) -> f32 {
        match self {
            Thumb::Held(share) | Thumb::LetGo { share, .. } | Thumb::Sending { share, .. } => share,
        }
    }
}

/// The thumb once the engine has published `playhead`: a thumb let go is
/// released by the playhead answering its seek, wherever that put it, and by
/// no playhead from before.
fn thumb_after(thumb: Option<Thumb>, playhead: &Playhead) -> Option<Thumb> {
    match thumb {
        Some(Thumb::LetGo { after, .. }) if playhead.seek > after => None,
        thumb => thumb,
    }
}

/// The count a seek let go now waits past: the engine's, or, while an
/// earlier release here is still unanswered (`waiting`, the count that one
/// waits past), one more than that, so the first seek's answer does not
/// release the thumb onto the first seek's place.
fn answer_after(waiting: Option<u64>, count: u64) -> u64 {
    waiting.map_or(count, |earlier| count.max(earlier + 1))
}

/// The seek slider and the times either side of it.
///
/// The position is reckoned from the engine's playhead anchor, the one source
/// of where the playhead is that the lyrics read too. The slider glides:
/// the anchor is handed to it (`Glide`) and the GPU reckons the fill per
/// present, so it moves with no frame drawn, and a new anchor (a seek, a
/// pause, the next track) corrects it. The times follow the engine's clock
/// (`ClockStream`), which wakes the view at each whole second while the
/// track plays and at each new anchor; paused, stopped or past the track's
/// end it says nothing, so a koan at rest costs nothing.
pub struct Seek {
    playhead: Entity<Playhead>,
    playback: Entity<Playback>,
    /// Whether the window shows: hidden, no timer runs.
    visible: bool,
    /// Following the engine's clock while the window shows.
    clock: Option<Task<()>>,
    /// The playhead as the engine's clock last reported it, and when: the
    /// times show at least this while it agrees with the anchor shown, since
    /// the anchor is stamped a little later than the engine took it.
    heard: Option<(u64, Instant)>,
    /// While the person has the thumb.
    thumb: Option<Thumb>,
    /// The count the latest seek let go here waits past, until a playhead
    /// past it arrives.
    waiting: Option<u64>,
    /// Releases made while another device is controlled, so a send that
    /// finishes late does not take the thumb from a later one.
    sends: u64,
    /// The device controlled when the thumb was last let go: a release on
    /// one device is not held on another.
    target: Option<String>,
    glide: Glide,
    _subscriptions: Vec<Subscription>,
}

impl Seek {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mirror = cx.global::<Mirror>().clone();
        let mut subscriptions = vec![
            cx.observe(&mirror.playhead, |this, playhead, cx| {
                let playhead = playhead.read(cx);
                this.thumb = thumb_after(this.thumb, playhead);
                this.waiting = this.waiting.filter(|after| playhead.seek <= *after);
                this.moved(cx);
            }),
            // Another device taken control of, or given back: a thumb let go
            // for the one before is not this one's to hold.
            cx.observe(&mirror.devices, |this, devices, cx| {
                let target = devices.read(cx).0.target.clone();
                if target != this.target {
                    this.target = target;
                    this.waiting = None;
                    if !matches!(this.thumb, Some(Thumb::Held(_))) {
                        this.thumb = None;
                    }
                    this.moved(cx);
                }
            }),
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
            clock: None,
            heard: None,
            thumb: None,
            waiting: None,
            sends: 0,
            target: mirror.devices.read(cx).0.target.clone(),
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
        let anchor = *self.playhead.read(cx);
        let duration = self.duration_ms(cx);
        let moving = self.visible && anchor.playing && duration > 0 && self.thumb.is_none();
        // The times follow the engine's clock, which wakes this at each
        // whole second while it plays; nothing of its own counts.
        if self.visible != self.clock.is_some() {
            self.clock = self.visible.then(|| {
                let clock = engine(cx).clock_stream();
                cx.spawn(async move |this, cx| {
                    while let Some(position) = clock.next().await {
                        let heard = this.update(cx, |this, cx| {
                            this.heard = Some((position, Instant::now()));
                            cx.notify();
                        });
                        if heard.is_err() {
                            break;
                        }
                    }
                })
            });
        }
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
    }

    fn commit(&mut self, cx: &mut Context<Self>) {
        let Some(Thumb::Held(share)) = self.thumb else {
            return;
        };
        let duration = self.duration_ms(cx);
        if duration == 0 {
            self.thumb = None;
            cx.notify();
            return;
        }
        // Where the engine puts it (clamped to what has arrived of a
        // download, or declined) is its to say; the thumb waits for that.
        let target = (share as f64 * duration as f64) as u64;
        let engine = engine(cx);
        if self.target.is_none() {
            let after = answer_after(self.waiting, engine.seeks());
            self.waiting = Some(after);
            self.thumb = Some(Thumb::LetGo { share, after });
            command(cx, "seek", move |_| engine.seek(target));
        } else {
            self.sends += 1;
            let sent = self.sends;
            self.thumb = Some(Thumb::Sending { share, sent });
            let seek = engine.seek(target);
            cx.spawn(async move |this, cx| {
                if let Err(e) = seek.await {
                    log::warn!("seek: {e}");
                }
                this.update(cx, |this, cx| {
                    if this
                        .thumb
                        .is_some_and(|t| t == Thumb::Sending { share, sent })
                    {
                        this.thumb = None;
                        this.moved(cx);
                    }
                })
                .ok();
            })
            .detach();
        }
        self.schedule(cx);
        cx.notify();
    }
}

impl Render for Seek {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Paint::of(window, cx);
        let duration = self.duration_ms(cx);
        let anchor = *self.playhead.read(cx);
        let now = Instant::now();
        let reckoned = anchor.at(now, duration);
        let position = shown_position(reckoned, self.heard, anchor.playing, now, duration);
        let share = match self.thumb {
            Some(thumb) => thumb.share(),
            None if duration > 0 => (position as f64 / duration as f64) as f32,
            None => 0.0,
        };
        let shown = self
            .thumb
            .map_or(position, |t| (t.share() as f64 * duration as f64) as u64);
        let (change, commit) = (cx.entity(), cx.entity());
        let _ = p;
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
            .named("seek")
            .compact()
            // An arrow key moves five seconds, not a twentieth of the track.
            .when(duration > 0, |s| {
                s.step((SEEK_KEY_MS as f32 / duration as f32).min(1.0))
            })
            .when(self.thumb.is_none(), |slider| slider.glide(&self.glide))
            .pending(matches!(self.thumb, Some(Thumb::Sending { .. })))
            .prism(crate::theme::KoanTheme::prism(cx))
            .when_some(reach, |slider, reach| slider.reach(&reach))
            .ends(clock(shown), clock(duration))
            .disabled(duration == 0)
            .on_change(move |share, _, cx| {
                change.update(cx, |this, cx| {
                    let started = !matches!(this.thumb, Some(Thumb::Held(_)));
                    this.thumb = Some(Thumb::Held(*share));
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
    }
}

/// What the control button draws from the devices: the device controlled,
/// how it can be reached, and its name.
type ControlledShown = (
    Option<String>,
    Option<koan_runtime::DeviceReach>,
    Option<String>,
);

fn controlled_shown(view: &Devices) -> ControlledShown {
    let name = view.target.as_ref().and_then(|t| {
        view.devices
            .iter()
            .find(|d| &d.id == t)
            .map(|d| d.name.clone())
    });
    (view.target.clone(), view.target_reach.clone(), name)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{Thumb, answer_after, shown_position, thumb_after};
    use crate::mirror::Playhead;

    fn playhead(position_ms: u64, seek: u64) -> Playhead {
        Playhead {
            position_ms,
            seekable_ms: TRACK,
            playing: true,
            at: Instant::now(),
            seek,
        }
    }

    /// Let go at a minute, with the engine at its fourth seek: a playhead
    /// published before the seek was taken, however it reads, leaves the
    /// thumb where it was let go.
    #[test]
    fn a_stale_playhead_after_a_seek_does_not_move_the_thumb_back() {
        let let_go = Some(Thumb::LetGo {
            share: 0.25,
            after: 4,
        });
        assert_eq!(thumb_after(let_go, &playhead(12_000, 4)), let_go);
        assert_eq!(thumb_after(let_go, &playhead(12_000, 3)), let_go);
    }

    /// The seek answered short of the thumb (clamped to what has arrived of
    /// a download, or declined): the thumb goes, and the engine's place is
    /// what shows.
    #[test]
    fn a_clamped_seek_settles_where_the_engine_says() {
        let let_go = Some(Thumb::LetGo {
            share: 0.9,
            after: 4,
        });
        assert_eq!(thumb_after(let_go, &playhead(30_000, 5)), None);
    }

    /// Let go twice before the first seek is answered: the second waits past
    /// the first's answer, so the thumb is not released onto the first
    /// seek's place.
    #[test]
    fn a_second_release_waits_for_its_own_answer() {
        assert_eq!(answer_after(None, 4), 4);
        let second = answer_after(Some(4), 4);
        assert_eq!(second, 5);
        let let_go = Some(Thumb::LetGo {
            share: 0.5,
            after: second,
        });
        assert_eq!(thumb_after(let_go, &playhead(12_000, 5)), let_go);
        assert_eq!(thumb_after(let_go, &playhead(60_000, 6)), None);
        // The first already answered by the engine, not yet heard here.
        assert_eq!(answer_after(Some(4), 5), 5);
    }

    /// With nothing let go, every playhead is followed, a seek made elsewhere
    /// (the media keys, another device) included; a thumb still held follows
    /// the pointer.
    #[test]
    fn a_seek_from_elsewhere_is_followed() {
        assert_eq!(thumb_after(None, &playhead(90_000, 7)), None);
        let held = Some(Thumb::Held(0.5));
        assert_eq!(thumb_after(held, &playhead(90_000, 7)), held);
    }

    const TRACK: u64 = 240_000;

    #[test]
    fn the_engines_reading_steps_the_times_when_it_agrees() {
        let now = Instant::now();
        // The anchor is stamped a little late, reckoning 4,960 at the
        // engine's whole second: the times show its 5,000.
        let heard = Some((5_000, now));
        assert_eq!(shown_position(4_960, heard, true, now, TRACK), 5_000);
        // A reading carries on from when it was heard.
        let later = now + Duration::from_millis(300);
        assert_eq!(shown_position(5_260, heard, true, later, TRACK), 5_300);
    }

    #[test]
    fn a_backward_seek_is_not_held_at_the_old_place() {
        let now = Instant::now();
        let heard = Some((90_000, now - Duration::from_millis(200)));
        assert_eq!(shown_position(10_000, heard, true, now, TRACK), 10_000);
    }

    #[test]
    fn a_reading_in_flight_does_not_override_a_new_anchor() {
        let now = Instant::now();
        // Read before a forward seek to 60 s, arriving after it.
        let heard = Some((12_000, now));
        assert_eq!(shown_position(60_000, heard, true, now, TRACK), 60_000);
    }

    #[test]
    fn paused_the_anchor_alone_is_shown() {
        let now = Instant::now();
        let heard = Some((5_000, now - Duration::from_millis(500)));
        assert_eq!(shown_position(4_990, heard, false, now, TRACK), 4_990);
    }

    #[test]
    fn a_reading_never_runs_past_the_track() {
        let now = Instant::now();
        let heard = Some((TRACK - 100, now - Duration::from_millis(400)));
        assert_eq!(shown_position(TRACK - 50, heard, true, now, TRACK), TRACK);
    }
}
