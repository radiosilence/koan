//! The kōan theme for GPUI: tokens, Geist Mono, the icon set, the record
//! accent and the wash. docs/design/koan-theme.md is the specification; there
//! is no other look, so nothing here has a "system" answer.
//!
//! What every window shares (the text size, "show icons") is the global
//! `KoanTheme`; the record's accent is an entity, `AccentFeed`, since it
//! changes with every record. What a window looks like is
//! `Look::of(window, cx)`: its appearance, dark or light, and the tokens and
//! accent for it. A component reads the look as it renders. A change of
//! appearance or text size redraws every window past its caches, which
//! `follow_appearance` and the setters here do; a change of accent notifies
//! only the views that observe the feed (`observe_accent`), so a cached view
//! that draws the accent calls it when it is made.

pub mod accent;
pub mod icons;
pub mod tokens;
pub mod wash;

use std::borrow::Cow;
use std::collections::HashMap;
use std::time::Instant;

use gpui_kit::{
    App, AppContext, Context, Entity, Global, Hsla, Pixels, SharedString, Styled, Subscription,
    Window, WindowAppearance, WindowId, px,
};

pub use accent::{Accent, AccentState, Shade};
/// A colour as the theme hands it out, for passing a role's colour along.
pub use gpui_kit::Hsla;
pub use icons::{Icon, KoanIcon};
pub use tokens::{
    DISABLED_OPACITY, FONT_FAMILY, HAIRLINE, HIT, MARK, Motion, Palette, ROW_HOVER_OPACITY,
    ROW_RULE_OPACITY, Scale, Space, TOOLBAR_BUTTON, TypeRole,
};

/// The colours text and glyphs take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Ink,
    Strong,
    Muted,
    Accent,
    Bad,
}

pub struct KoanTheme {
    /// The text size, in percent (80 to 130 in steps of 5).
    pub text_size: u32,
    /// "Show icons": glyphs beside the labels that carry them.
    pub icons: bool,
    /// The wash and the accent take their colour from the record playing;
    /// off, there is no wash and the accent is mint.
    pub record_colours: bool,
    /// "Theme tint", in percent: how far the wash is held to its contrast
    /// limit.
    pub wash_tint: u32,
    /// "Wash the whole window": the wash runs under the sidebar and the
    /// toolbar as well as the page, and they draw no ground of their own.
    pub wash_window: bool,
    /// Low Power Mode, as the system last said.
    low_power: bool,
    /// Windows held to one appearance whatever the system's, as the theme
    /// sheet holds its two.
    pinned: HashMap<WindowId, bool>,
}

impl Global for KoanTheme {}

impl Default for KoanTheme {
    fn default() -> Self {
        Self {
            text_size: 100,
            icons: true,
            record_colours: true,
            wash_tint: 100,
            wash_window: true,
            low_power: false,
            pinned: HashMap::new(),
        }
    }
}

const FACES: [&[u8]; 3] = [
    include_bytes!("../../assets/fonts/GeistMono-ExtraLight.ttf"),
    include_bytes!("../../assets/fonts/GeistMono-Light.ttf"),
    include_bytes!("../../assets/fonts/GeistMono-Regular.ttf"),
];

/// Registers Geist Mono, sets the theme global, reads Reduce Motion and
/// dresses gpui-component's own theme (which the text field draws with) in
/// the tokens. Call after `gpui_kit::init`.
pub fn init(cx: &mut App) {
    cx.text_system()
        .add_fonts(FACES.iter().map(|face| Cow::Borrowed(*face)).collect())
        .expect("Geist Mono is bundled and loads");
    cx.set_global(KoanTheme::default());
    let feed = cx.new(|_| AccentFeed {
        state: AccentState::default(),
    });
    cx.set_global(AccentHandle(feed));
    read_motion_settings(cx);
    #[cfg(target_os = "macos")]
    watch_motion_settings(cx);
    sync_components(is_dark(cx.window_appearance()), cx);
}

/// Keeps a window drawn in its appearance: redrawn when the system's
/// changes. Hold the returned subscription as long as the window.
pub fn follow_appearance<V: 'static>(window: &mut Window, cx: &mut Context<V>) -> Subscription {
    cx.observe_window_appearance(window, |_, window, cx| {
        sync_components(Look::of(window, cx).dark, cx);
        window.refresh();
    })
}

fn is_dark(appearance: WindowAppearance) -> bool {
    matches!(
        appearance,
        WindowAppearance::Dark | WindowAppearance::VibrantDark
    )
}

/// Reduce Motion and Low Power Mode, from the system: GPUI holds the first
/// but no platform sets it, and has no word for the second. Linux has
/// neither to read, and keeps motion on. A change redraws every window, as
/// `set_reduce_motion` does for its own, so a wash at rest, which draws no
/// frames, sees one lifted, and a drifting one stops at once.
fn read_motion_settings(cx: &mut App) {
    #[cfg(target_os = "macos")]
    {
        let reduce =
            objc2_app_kit::NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion();
        cx.set_reduce_motion(reduce);
        let low_power = objc2_foundation::NSProcessInfo::processInfo().isLowPowerModeEnabled();
        if cx.global::<KoanTheme>().low_power != low_power {
            cx.global_mut::<KoanTheme>().low_power = low_power;
            cx.refresh_windows();
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = cx;
}

/// Reads the motion settings again whenever the system says one changed:
/// Low Power Mode through `NSProcessInfoPowerStateDidChangeNotification`,
/// which is posted on whatever thread changed it, and Reduce Motion through
/// the workspace's `NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification`.
/// Each observer only wakes a task on the main thread, which reads them.
#[cfg(target_os = "macos")]
fn watch_motion_settings(cx: &mut App) {
    use std::ptr::NonNull;

    use block2::RcBlock;
    use futures::StreamExt as _;
    use objc2_app_kit::{NSWorkspace, NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification};
    use objc2_foundation::{
        NSNotification, NSNotificationCenter, NSNotificationName,
        NSProcessInfoPowerStateDidChangeNotification,
    };

    let (changed, mut changes) = futures::channel::mpsc::unbounded::<()>();
    let observe = |center: &NSNotificationCenter, name: &NSNotificationName| {
        let changed = changed.clone();
        let block = RcBlock::new(move |_: NonNull<NSNotification>| {
            changed.unbounded_send(()).ok();
        });
        // SAFETY: no object is given, the block runs on the poster's thread,
        // and it holds only a channel sender, which is Send and Sync.
        unsafe { center.addObserverForName_object_queue_usingBlock(Some(name), None, None, &block) }
    };
    // SAFETY: both names are immutable framework constants.
    let (power, display) = unsafe {
        (
            NSProcessInfoPowerStateDidChangeNotification,
            NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification,
        )
    };
    let observers = [
        observe(&NSNotificationCenter::defaultCenter(), power),
        observe(
            &NSWorkspace::sharedWorkspace().notificationCenter(),
            display,
        ),
    ];
    cx.set_global(MotionObservers(observers));
    cx.spawn(async move |cx| {
        while changes.next().await.is_some() {
            cx.update(read_motion_settings);
        }
    })
    .detach();
}

/// The notification observers, held for the life of the app: the centres
/// keep calling a block only while its observer is retained.
#[cfg(target_os = "macos")]
struct MotionObservers(
    #[allow(dead_code)]
    [objc2::rc::Retained<objc2::runtime::ProtocolObject<dyn objc2_foundation::NSObjectProtocol>>;
        2],
);

#[cfg(target_os = "macos")]
impl Global for MotionObservers {}

/// Holds a window to one appearance whatever the system's, or lets it follow
/// the system again with `None`.
pub fn pin_appearance(dark: Option<bool>, window: &mut Window, cx: &mut App) {
    let id = window.window_handle().window_id();
    let pinned = &mut cx.global_mut::<KoanTheme>().pinned;
    match dark {
        Some(dark) => pinned.insert(id, dark),
        None => pinned.remove(&id),
    };
    sync_components(Look::of(window, cx).dark, cx);
    window.refresh();
}

/// gpui-component's theme, in the tokens: the text field is its `Input`, and
/// reads its text colours, caret, selection, type and corners from this one
/// global. It follows the window most recently told to (the system's
/// appearance, or a pinned window's), since it cannot differ per window.
fn sync_components(dark: bool, cx: &mut App) {
    use gpui_kit::component::theme::Theme;
    let koan = cx.global::<KoanTheme>();
    let palette = Palette::of(dark);
    let accent = accent_state(cx).at(Instant::now()).shade(dark).hsla();
    let size = TypeRole::Control.points() * Scale::of(koan.text_size).text;
    sync_scrollbar(&palette, cx);
    let current = Theme::global(cx);
    if current.colors.background == palette.bg.hsla()
        && current.colors.caret == accent
        && current.font_size == px(size)
        && current.font_family.as_ref() == FONT_FAMILY
    {
        return;
    }
    let theme = Theme::global_mut(cx);
    theme.font_family = FONT_FAMILY.into();
    theme.mono_font_family = FONT_FAMILY.into();
    theme.font_size = px(size);
    theme.mono_font_size = px(size);
    theme.radius = px(0.0);
    theme.radius_lg = px(0.0);
    theme.shadow = false;
    theme.colors.foreground = palette.ink.hsla();
    theme.colors.muted_foreground = palette.muted.hsla();
    theme.colors.background = palette.bg.hsla();
    theme.colors.border = palette.muted.hsla();
    theme.colors.input = palette.muted.hsla();
    theme.colors.ring = accent;
    theme.colors.caret = accent;
    theme.colors.selection = accent.opacity(0.3);
}

/// The kōan scrollbar, for gpui-base's `Scrollbar`, which every scrolling
/// surface lays over itself: shown while it scrolls and while the pointer is
/// over its edge, thin and square, `muted` at rest and `ink` held, on no
/// track, taking no width from the page. It fades on the motion tokens.
fn sync_scrollbar(palette: &Palette, cx: &mut App) {
    use gpui_kit::base::{ScrollbarMode, ScrollbarMotion, ScrollbarStyles, ScrollbarTheme};
    let (muted, ink) = (palette.muted.hsla(), palette.ink.hsla());
    let clear = palette.bg.hsla().opacity(0.);
    let thumb = |s: gpui_kit::base::ScrollbarThumbStyle, width: f32, color: Hsla| {
        s.width(px(width))
            .inset(px(3.))
            .radius(px(0.))
            .min_length(px(32.))
            .bg(color)
    };
    let styles = ScrollbarStyles::default()
        .track(|t| t.bg(clear).border_color(clear))
        .track_hover(|t| t.bg(clear).border_color(clear))
        .track_active(|t| t.bg(clear).border_color(clear))
        .thumb(|s| thumb(s, 3., muted.opacity(0.6)))
        .thumb_hover(|s| thumb(s, 5., ink.opacity(0.7)))
        .thumb_active(|s| thumb(s, 5., ink));
    let motion = ScrollbarMotion::default()
        .with_enter(Motion::FAST)
        .with_exit(Motion::NORMAL)
        .with_expand(Motion::FAST);
    gpui_kit::base::Theme::global_mut(cx).scrollbar = ScrollbarTheme::new()
        .with_mode(ScrollbarMode::Hover)
        .with_motion(motion)
        .with_styles(styles);
}

impl KoanTheme {
    /// Whether the system lets things move for their own sake: Reduce
    /// Motion and Low Power Mode off, as the system last said.
    pub fn motion_allowed(cx: &App) -> bool {
        !cx.reduce_motion() && !cx.global::<KoanTheme>().low_power
    }

    /// Moves the accent to a record's. `ease` is false for a colour already
    /// known when the record arrived, which lands with it; with Reduce Motion
    /// every change lands.
    pub fn set_accent(accent: Accent, ease: bool, cx: &mut App) {
        let ease = ease && !cx.reduce_motion();
        let feed = cx.global::<AccentHandle>().0.clone();
        feed.update(cx, |feed, cx| {
            if feed.state.target() != accent {
                feed.state.set(accent, ease, Instant::now());
                cx.notify();
            }
        });
    }

    /// The accent the record has, once it has arrived.
    pub fn accent_target(cx: &App) -> Accent {
        accent_state(cx).target()
    }

    pub fn set_text_size(percent: u32, cx: &mut App) {
        cx.global_mut::<KoanTheme>().text_size =
            percent.clamp(*tokens::TEXT_SIZES.start(), *tokens::TEXT_SIZES.end());
        sync_components(is_dark(cx.window_appearance()), cx);
        cx.refresh_windows();
    }

    pub fn set_icons(icons: bool, cx: &mut App) {
        cx.global_mut::<KoanTheme>().icons = icons;
        cx.refresh_windows();
    }

    /// Whoever draws the wash observes the global for these two. The
    /// record's colours also decide whether the wash runs under the whole
    /// window (`washes_window`), whose layout every window then redraws.
    pub fn set_record_colours(on: bool, cx: &mut App) {
        let before = Self::washes_window(cx);
        cx.global_mut::<KoanTheme>().record_colours = on;
        if Self::washes_window(cx) != before {
            cx.refresh_windows();
        }
    }

    /// Whether the wash runs under the whole window: "wash the whole
    /// window" on, and a wash to run, which "colours from the record" off
    /// takes away.
    pub fn washes_window(cx: &App) -> bool {
        let theme = cx.global::<KoanTheme>();
        theme.wash_window && theme.record_colours
    }

    pub fn set_wash_tint(percent: u32, cx: &mut App) {
        cx.global_mut::<KoanTheme>().wash_tint = percent.min(100);
    }

    /// Moves the window's grounds, which sit in cached views: every window
    /// is drawn again.
    pub fn set_wash_window(on: bool, cx: &mut App) {
        if cx.global::<KoanTheme>().wash_window != on {
            cx.global_mut::<KoanTheme>().wash_window = on;
            cx.refresh_windows();
        }
    }

    /// What a window's root view calls as it renders: dresses gpui-component's
    /// theme for this window (windows render one at a time, so each one's
    /// text fields read their own appearance, and the caret follows the
    /// accent as it settles), and while the accent settles, notifies the
    /// feed once a frame so the views observing it redraw, and only those.
    /// The root asks for its own next frame too, so the ease runs to its end
    /// in a window where no view observes the feed. Costs a few comparisons
    /// once it is at rest.
    pub fn drive(window: &mut Window, cx: &mut App) {
        sync_components(Look::of(window, cx).dark, cx);
        if accent_state(cx).settling(Instant::now()) {
            let feed = cx.global::<AccentHandle>().0.clone();
            window.on_next_frame(move |_, cx| feed.update(cx, |_, cx| cx.notify()));
            window.request_animation_frame();
        }
    }
}

/// The record's accent as it settles. An entity rather than part of the
/// global so that a change reaches the views that draw it, not every window.
pub struct AccentFeed {
    state: AccentState,
}

struct AccentHandle(Entity<AccentFeed>);

impl Global for AccentHandle {}

fn accent_state(cx: &App) -> &AccentState {
    &cx.global::<AccentHandle>().0.read(cx).state
}

/// Redraws a view whenever the accent moves. A cached view that draws the
/// accent holds the subscription for as long as it lives; an uncached one
/// redraws with its parent and needs none.
pub fn observe_accent<V: 'static>(cx: &mut Context<V>) -> Subscription {
    let feed = cx.global::<AccentHandle>().0.clone();
    cx.observe(&feed, |_, _, cx| cx.notify())
}

/// `observe_accent`, running `then` as the accent moves: for a view whose
/// cached children draw it too and are not views of their own making.
pub fn observe_accent_then<V: 'static>(
    cx: &mut Context<V>,
    then: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> Subscription {
    let feed = cx.global::<AccentHandle>().0.clone();
    cx.observe(&feed, move |view, _, cx| {
        cx.notify();
        then(view, cx);
    })
}

/// A window's look: its appearance and everything drawn from it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub dark: bool,
    pub palette: Palette,
    pub accent: Shade,
    pub scale: Scale,
    pub icons: bool,
}

impl Look {
    pub fn of(window: &Window, cx: &App) -> Look {
        let theme = cx.global::<KoanTheme>();
        let dark = theme
            .pinned
            .get(&window.window_handle().window_id())
            .copied()
            .unwrap_or_else(|| is_dark(window.appearance()));
        Look {
            dark,
            palette: Palette::of(dark),
            accent: accent_state(cx).at(Instant::now()).shade(dark),
            scale: Scale::of(theme.text_size),
            icons: theme.icons,
        }
    }

    pub fn color(&self, tone: Tone) -> Hsla {
        let p = self.palette;
        match tone {
            Tone::Ink => p.ink.hsla(),
            Tone::Strong => p.strong.hsla(),
            Tone::Muted => p.muted.hsla(),
            Tone::Bad => p.bad.hsla(),
            Tone::Accent => self.accent.hsla(),
        }
    }

    /// A tone for text: the accent stays off text it cannot reach 4.5:1 as,
    /// which reads in `ink` instead.
    pub fn text_color(&self, tone: Tone) -> Hsla {
        match tone {
            Tone::Accent if !self.accent.reads_as_text => self.palette.ink.hsla(),
            tone => self.color(tone),
        }
    }

    pub fn space(&self, space: Space) -> Pixels {
        px(space.points() * self.scale.space)
    }

    /// A length on spacing's scale for a padding the spec gives in points
    /// that are not steps of the scale (a prominent button's 12 × 20).
    pub fn spaced(&self, points: f32) -> Pixels {
        px(points * self.scale.space)
    }

    pub fn font_size(&self, role: TypeRole) -> Pixels {
        px(self.points(role))
    }

    pub fn line_height(&self, role: TypeRole) -> Pixels {
        px(self.points(role) * role.line_height())
    }

    /// A role's size in points as drawn, which the icon set is sized beside.
    pub fn points(&self, role: TypeRole) -> f32 {
        role.points() * self.scale.text
    }
}

/// Type roles on any element.
pub trait KoanStyled: Styled + Sized {
    fn text_role(self, role: TypeRole, look: &Look) -> Self {
        self.font_family(FONT_FAMILY)
            .text_size(look.font_size(role))
            .font_weight(role.weight())
            .line_height(look.line_height(role))
    }
}

impl<T: Styled> KoanStyled for T {}

/// An app word as the theme writes it: lowercase. Text from the library and
/// from people keeps its own case and does not come through here.
pub fn label(text: impl AsRef<str>) -> SharedString {
    text.as_ref().to_lowercase().into()
}
