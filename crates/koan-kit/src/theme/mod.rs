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

use crate::components::{Repeat as TrackRepeat, Track};
use gpui::{
    App, AppContext, Context, Entity, Global, Pixels, SharedString, Styled, Subscription, Window,
    WindowAppearance, WindowId, px,
};

pub use accent::{Accent, Shade};
/// A colour as the theme hands it out, for passing a role's colour along.
pub use gpui::Hsla;
pub use icons::{Icon, KoanIcon};
pub use tokens::{
    COMPACT_WIDTH,
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
    /// A wash painted from something other than the record (the rainbow's
    /// sheen), which runs whatever "colours from the record" says.
    pub painted_wash: bool,
    /// "Theme tint", in percent: how far the wash is held to its contrast
    /// limit.
    pub wash_tint: u32,
    /// How fast the wash drifts, in percent of its own pace: 0 holds it
    /// still.
    pub wash_drift: u32,
    /// How far the wash turns either way as it drifts, in degrees.
    pub wash_turn: u32,
    /// "Wash the whole window": the wash runs under the sidebar and the
    /// toolbar as well as the page, and they draw no ground of their own.
    pub wash_window: bool,
    /// Touch, not a pointer, is how the app is used: buttons, rows, menu
    /// rows, toggles and segmented controls are at least `HIT` tall, a
    /// finger's target, and a held touch opens context menus. On by default
    /// on phones and tablets; the storybook switches it to preview the look
    /// on a desktop.
    pub touch: bool,
    /// Low Power Mode, as the system last said.
    low_power: bool,
    /// Increase Contrast (macOS) or high contrast (GNOME), as the system
    /// last said: the contrast palettes, thicker rules, the accent at 7:1.
    contrast: bool,
    /// Reduce Transparency (macOS), as the system last said: no wash, and
    /// every ground opaque.
    opaque: bool,
    /// The system's text scaling (GNOME's `text-scaling-factor`), which the
    /// app's own size multiplies; 1 where the system has none.
    system_text: f32,
    /// Windows held to one appearance whatever the system's, as the theme
    /// sheet holds its two.
    pinned: HashMap<WindowId, bool>,
    /// The hidden rainbow appearance, while it is drawn, and whether in
    /// brat's palette: what may carry the palette as a gradient (a progress
    /// fill, the chosen tab) does.
    pub prism: Option<bool>,
}

/// `KoanTheme::prism` as a number, for `prism_now`: 0 put away, 1 the
/// palette, 2 brat's.
static PRISM: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

impl Global for KoanTheme {}

impl Default for KoanTheme {
    fn default() -> Self {
        Self {
            text_size: 100,
            icons: true,
            record_colours: true,
            painted_wash: false,
            wash_tint: 100,
            wash_drift: 100,
            wash_turn: 3,
            wash_window: true,
            touch: cfg!(any(target_os = "ios", target_os = "android")),
            low_power: false,
            contrast: false,
            opaque: false,
            system_text: 1.0,
            pinned: HashMap::new(),
            prism: None,
        }
    }
}

const FACES: [&[u8]; 3] = [
    include_bytes!("../../assets/fonts/GeistMono-Thin.ttf"),
    include_bytes!("../../assets/fonts/GeistMono-ExtraLight.ttf"),
    include_bytes!("../../assets/fonts/GeistMono-Regular.ttf"),
];

/// Registers Geist Mono, sets the theme global, reads the system's motion,
/// contrast, transparency and text size settings (outside tests) and
/// dresses gpui-component's own theme (which the text field draws with) in
/// the tokens. Call after `gpui::init`.
pub fn init(cx: &mut App) {
    cx.text_system()
        .add_fonts(FACES.iter().map(|face| Cow::Borrowed(*face)).collect())
        .expect("Geist Mono is bundled and loads");
    cx.set_global(KoanTheme::default());
    crate::components::install_color_slots();
    let feed = cx.new(|_| AccentFeed::new());
    cx.set_global(AccentHandle(feed));
    // Tests draw as an unconfigured machine would, whatever the host's
    // accessibility settings: a runner asking for less motion would land
    // every transition a test means to catch in flight. On Linux the portal
    // also answers over D-Bus from threads of its own, which the
    // deterministic test scheduler refuses as activity it cannot order.
    if !cx.is_test() {
        read_system_settings(cx);
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        watch_system_settings(cx);
        #[cfg(target_os = "linux")]
        watch_desktop_settings(cx);
    }
    crate::components::field::bind(cx);
    crate::components::bind_root(cx);
}

/// Keeps a window drawn in its appearance: redrawn when the system's
/// changes. Hold the returned subscription as long as the window.
pub fn follow_appearance<V: 'static>(window: &mut Window, cx: &mut Context<V>) -> Subscription {
    cx.observe_window_appearance(window, |_, window, _| {
        window.refresh();
    })
}

fn is_dark(appearance: WindowAppearance) -> bool {
    matches!(
        appearance,
        WindowAppearance::Dark | WindowAppearance::VibrantDark
    )
}

/// Reduce Motion, Increase Contrast, Reduce Transparency and Low Power
/// Mode, from the system: GPUI holds the first but no platform sets it, and
/// has no word for the rest. Linux reads its contrast and text size through
/// the portal instead (`watch_desktop_settings`) and keeps motion on. A
/// change redraws every window, as `set_reduce_motion` does for its own, so
/// a wash at rest, which draws no frames, sees one lifted, and a drifting
/// one stops at once.
fn read_system_settings(cx: &mut App) {
    #[cfg(target_os = "macos")]
    {
        let workspace = objc2_app_kit::NSWorkspace::sharedWorkspace();
        set_system_display(
            Some(workspace.accessibilityDisplayShouldIncreaseContrast()),
            Some(workspace.accessibilityDisplayShouldReduceTransparency()),
            None,
            cx,
        );
        let reduce = workspace.accessibilityDisplayShouldReduceMotion();
        if cx.reduce_motion() != reduce {
            cx.set_reduce_motion(reduce);
            // Touched, so what observes the theme to know whether it may
            // move (a timer of its own) hears of it as of Low Power.
            cx.global_mut::<KoanTheme>();
        }
        let low_power = objc2_foundation::NSProcessInfo::processInfo().isLowPowerModeEnabled();
        if cx.global::<KoanTheme>().low_power != low_power {
            cx.global_mut::<KoanTheme>().low_power = low_power;
            cx.refresh_windows();
        }
    }
    #[cfg(target_os = "ios")]
    {
        use objc2_ui_kit::{
            UIAccessibilityDarkerSystemColorsEnabled, UIAccessibilityIsReduceMotionEnabled,
            UIAccessibilityIsReduceTransparencyEnabled, UIApplication,
        };
        let text = objc2::MainThreadMarker::new().and_then(|main| {
            dynamic_type_factor(
                &UIApplication::sharedApplication(main).preferredContentSizeCategory(),
            )
        });
        set_system_display(
            Some(UIAccessibilityDarkerSystemColorsEnabled()),
            Some(UIAccessibilityIsReduceTransparencyEnabled()),
            text,
            cx,
        );
        let reduce = UIAccessibilityIsReduceMotionEnabled();
        if cx.reduce_motion() != reduce {
            cx.set_reduce_motion(reduce);
            cx.global_mut::<KoanTheme>();
        }
        let low_power = objc2_foundation::NSProcessInfo::processInfo().isLowPowerModeEnabled();
        if cx.global::<KoanTheme>().low_power != low_power {
            cx.global_mut::<KoanTheme>().low_power = low_power;
            cx.refresh_windows();
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    let _ = cx;
}

/// The Dynamic Type size as a factor on kōan's text sizes: the body size
/// each category gives, over Large's 17 pt, which is iOS's default. None for
/// a category with no size (Unspecified), which keeps the last one.
#[cfg(target_os = "ios")]
fn dynamic_type_factor(category: &objc2_ui_kit::UIContentSizeCategory) -> Option<f32> {
    use objc2_ui_kit::*;
    // SAFETY: the categories are immutable framework constants.
    let sizes = unsafe {
        [
            (UIContentSizeCategoryExtraSmall, 14.0),
            (UIContentSizeCategorySmall, 15.0),
            (UIContentSizeCategoryMedium, 16.0),
            (UIContentSizeCategoryLarge, 17.0),
            (UIContentSizeCategoryExtraLarge, 19.0),
            (UIContentSizeCategoryExtraExtraLarge, 21.0),
            (UIContentSizeCategoryExtraExtraExtraLarge, 23.0),
            (UIContentSizeCategoryAccessibilityMedium, 28.0),
            (UIContentSizeCategoryAccessibilityLarge, 33.0),
            (UIContentSizeCategoryAccessibilityExtraLarge, 40.0),
            (UIContentSizeCategoryAccessibilityExtraExtraLarge, 47.0),
            (UIContentSizeCategoryAccessibilityExtraExtraExtraLarge, 53.0),
        ]
    };
    sizes
        .iter()
        .find(|(c, _)| *c == category)
        .map(|(_, size)| size / 17.0)
}

/// The system's contrast, transparency and text size, each where it said
/// one: a change restyles the text field's theme and redraws every window,
/// and observers of the theme hear of it.
fn set_system_display(
    contrast: Option<bool>,
    opaque: Option<bool>,
    text: Option<f32>,
    cx: &mut App,
) {
    let theme = cx.global::<KoanTheme>();
    let contrast = contrast.unwrap_or(theme.contrast);
    let opaque = opaque.unwrap_or(theme.opaque);
    let text = text
        .filter(|t| t.is_finite() && *t > 0.0)
        .unwrap_or(theme.system_text);
    if (contrast, opaque, text) == (theme.contrast, theme.opaque, theme.system_text) {
        return;
    }
    let theme = cx.global_mut::<KoanTheme>();
    theme.contrast = contrast;
    theme.opaque = opaque;
    theme.system_text = text;
    cx.refresh_windows();
}

/// Whether GNOME's text scaling factor is already in the window's scale:
/// GNOME on X11, where gnome-settings-daemon folds it into `Xft.dpi`, which
/// GPUI scales an X11 window by. Other desktops on X11 (XFCE, Cinnamon,
/// MATE) leave `Xft.dpi` alone, and Wayland never folds it in.
#[cfg(target_os = "linux")]
fn text_scaled_by_dpi() -> bool {
    let x11 =
        std::env::var_os("WAYLAND_DISPLAY").is_none() && std::env::var_os("DISPLAY").is_some();
    let gnome = std::env::var("XDG_CURRENT_DESKTOP")
        .is_ok_and(|desktops| desktops.split(':').any(|d| d.eq_ignore_ascii_case("GNOME")));
    x11 && gnome
}

/// High contrast and GNOME's text scaling factor, through the desktop
/// portal's settings, and again whenever the portal says either changed.
/// A desktop without the portal, or without either key, leaves both as
/// they are. Where the factor is already in the window's scale
/// (`text_scaled_by_dpi`), applying the portal's too would scale text twice,
/// so it is left out.
#[cfg(target_os = "linux")]
fn watch_desktop_settings(cx: &mut App) {
    use ashpd::desktop::settings::{Contrast, Settings};
    use futures::StreamExt as _;
    use futures::stream::{BoxStream, empty};

    const INTERFACE: &str = "org.gnome.desktop.interface";
    const SCALING: &str = "text-scaling-factor";
    enum Change {
        Contrast(bool),
        Text(f32),
    }
    cx.spawn(async move |cx| {
        let Ok(settings) = Settings::new().await else {
            return;
        };
        let contrast = settings.contrast().await.ok().map(|c| c == Contrast::High);
        let scaled_by_dpi = text_scaled_by_dpi();
        let text = settings
            .read::<f64>(INTERFACE, SCALING)
            .await
            .ok()
            .map(|t| t as f32)
            .filter(|_| !scaled_by_dpi);
        cx.update(|cx| set_system_display(contrast, None, text, cx));
        let contrasts: BoxStream<Change> = match settings.receive_contrast_changed().await {
            Ok(changes) => changes
                .map(|c| Change::Contrast(c == Contrast::High))
                .boxed(),
            Err(_) => empty().boxed(),
        };
        let texts: BoxStream<Change> = match settings
            .receive_setting_changed_with_args::<f64>(INTERFACE, SCALING)
            .await
        {
            Ok(changes) => changes
                .filter_map(|t| async move { t.ok().map(|t| Change::Text(t as f32)) })
                .boxed(),
            Err(_) => empty().boxed(),
        };
        let mut changes = futures::stream::select(contrasts, texts);
        while let Some(change) = changes.next().await {
            cx.update(|cx| match change {
                Change::Contrast(on) => set_system_display(Some(on), None, None, cx),
                Change::Text(_) if scaled_by_dpi => {}
                Change::Text(factor) => set_system_display(None, None, Some(factor), cx),
            });
        }
    })
    .detach();
}

/// Reads the system's settings again whenever the system says one changed:
/// Low Power Mode through `NSProcessInfoPowerStateDidChangeNotification`,
/// which is posted on whatever thread changed it, and Reduce Motion through
/// the workspace's `NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification`,
/// which also covers Increase Contrast and Reduce Transparency.
/// Each observer only wakes a task on the main thread, which reads them.
#[cfg(target_os = "macos")]
fn watch_system_settings(cx: &mut App) {
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
            cx.update(read_system_settings);
        }
    })
    .detach();
}

/// Reads the system's settings again whenever UIKit says one changed: each
/// of Reduce Motion, Darker System Colors, Reduce Transparency and the
/// Dynamic Type size has a notification of its own, and Low Power Mode is
/// `NSProcessInfoPowerStateDidChangeNotification`, posted on whatever thread
/// changed it. Each observer only wakes a task on the main thread.
#[cfg(target_os = "ios")]
fn watch_system_settings(cx: &mut App) {
    use std::ptr::NonNull;

    use block2::RcBlock;
    use futures::StreamExt as _;
    use objc2_foundation::{
        NSNotification, NSNotificationCenter, NSProcessInfoPowerStateDidChangeNotification,
    };
    use objc2_ui_kit::{
        UIAccessibilityDarkerSystemColorsStatusDidChangeNotification,
        UIAccessibilityReduceMotionStatusDidChangeNotification,
        UIAccessibilityReduceTransparencyStatusDidChangeNotification,
        UIContentSizeCategoryDidChangeNotification,
    };

    let (changed, mut changes) = futures::channel::mpsc::unbounded::<()>();
    let center = NSNotificationCenter::defaultCenter();
    // SAFETY: the names are immutable framework constants.
    let names = unsafe {
        [
            NSProcessInfoPowerStateDidChangeNotification,
            UIAccessibilityReduceMotionStatusDidChangeNotification,
            UIAccessibilityDarkerSystemColorsStatusDidChangeNotification,
            UIAccessibilityReduceTransparencyStatusDidChangeNotification,
            UIContentSizeCategoryDidChangeNotification,
        ]
    };
    let observers = names.map(|name| {
        let changed = changed.clone();
        let block = RcBlock::new(move |_: NonNull<NSNotification>| {
            changed.unbounded_send(()).ok();
        });
        // SAFETY: no object is given, the block runs on the poster's thread,
        // and it holds only a channel sender, which is Send and Sync.
        unsafe { center.addObserverForName_object_queue_usingBlock(Some(name), None, None, &block) }
    });
    cx.set_global(MotionObservers(observers));
    cx.spawn(async move |cx| {
        while changes.next().await.is_some() {
            cx.update(read_system_settings);
        }
    })
    .detach();
}

/// The notification observers, held for the life of the app: the centres
/// keep calling a block only while its observer is retained.
#[cfg(any(target_os = "macos", target_os = "ios"))]
struct MotionObservers<const N: usize>(
    #[allow(dead_code)]
    [objc2::rc::Retained<objc2::runtime::ProtocolObject<dyn objc2_foundation::NSObjectProtocol>>;
        N],
);

#[cfg(any(target_os = "macos", target_os = "ios"))]
impl<const N: usize> Global for MotionObservers<N> {}

/// Holds a window to one appearance whatever the system's, or lets it follow
/// the system again with `None`.
pub fn pin_appearance(dark: Option<bool>, window: &mut Window, cx: &mut App) {
    let id = window.window_handle().window_id();
    let pinned = &mut cx.global_mut::<KoanTheme>().pinned;
    match dark {
        Some(dark) => pinned.insert(id, dark),
        None => pinned.remove(&id),
    };
    window.refresh();
}

/// The ratio the accent is held to on both grounds with more contrast asked
/// for.
const CONTRAST_ACCENT: f64 = 7.0;

/// A veil's opacity with less transparency asked for: the page behind still
/// shows that it is there.
const OPAQUE_VEIL: f32 = 0.95;

impl KoanTheme {
    /// The text size chosen in the app, on top of the system's.
    pub fn scale(&self) -> Scale {
        Scale::at(self.text_size, self.system_text)
    }

    /// Whether the system asks for less transparency: the wash stays off
    /// and every ground is opaque.
    pub fn opaque(cx: &App) -> bool {
        cx.global::<KoanTheme>().opaque
    }

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
        let contrast = cx.global::<KoanTheme>().contrast;
        let feed = cx.global::<AccentHandle>().0.clone();
        feed.update(cx, |feed, cx| {
            if feed.target == accent {
                return;
            }
            feed.target = accent;
            feed.aim(contrast, ease);
            // Everything drawn in the accent names its slots (fills, text,
            // paths, a caret, shader uniforms) and a fade reads a slot as it
            // starts, so a turn redraws no view: the rainbow's every step
            // included. Only without slots does a view draw the colour
            // itself, and need drawing again.
            if feed.slots.is_none() {
                cx.notify();
            }
        });
        // A window at rest presents nothing by itself: one present each, and
        // a window drawing with the slots keeps presenting while they move.
        for window in cx.windows() {
            window
                .update(cx, |_, window, _| window.request_present())
                .ok();
        }
    }

    pub fn set_text_size(percent: u32, cx: &mut App) {
        cx.global_mut::<KoanTheme>().text_size =
            percent.clamp(*tokens::TEXT_SIZES.start(), *tokens::TEXT_SIZES.end());
        cx.refresh_windows();
    }

    /// The rainbow appearance drawn, in brat's palette or not, or put away.
    /// The one place it is kept: the windows' roots observe the theme for it.
    pub fn set_prism(prism: Option<bool>, cx: &mut App) {
        if cx.global::<KoanTheme>().prism != prism {
            PRISM.store(
                match prism {
                    None => 0,
                    Some(false) => 1,
                    Some(true) => 2,
                },
                std::sync::atomic::Ordering::Relaxed,
            );
            cx.global_mut::<KoanTheme>().prism = prism;
            cx.refresh_windows();
        }
    }

    /// Whether the rainbow is drawn, and in brat's palette.
    pub fn prism(cx: &App) -> Option<bool> {
        cx.try_global::<KoanTheme>().and_then(|t| t.prism)
    }

    /// The same, for elements built where the app is not to hand, such as
    /// the playing bars inside a row: they are drawn again whenever it
    /// changes, since it refreshes the windows.
    pub fn prism_now() -> Option<bool> {
        match PRISM.load(std::sync::atomic::Ordering::Relaxed) {
            1 => Some(false),
            2 => Some(true),
            _ => None,
        }
    }

    /// The palette's gradient, where a component may draw one: the rainbow
    /// is drawn, shaders are, and motion is allowed.
    pub fn prism_gradient(cx: &App) -> Option<bool> {
        cx.global::<KoanTheme>()
            .prism
            .filter(|_| crate::components::SHADED && Self::motion_allowed(cx))
    }

    pub fn set_touch(touch: bool, cx: &mut App) {
        cx.global_mut::<KoanTheme>().touch = touch;
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

    /// A wash is painted, or no longer, from something other than the
    /// record: the rainbow's sheen runs with "colours from the record" off.
    pub fn set_painted_wash(on: bool, cx: &mut App) {
        if cx.global::<KoanTheme>().painted_wash == on {
            return;
        }
        let before = Self::washes_window(cx);
        cx.global_mut::<KoanTheme>().painted_wash = on;
        if Self::washes_window(cx) != before {
            cx.refresh_windows();
        }
    }

    /// Whether the wash runs under the whole window: "wash the whole
    /// window" on, and a wash to run: the record's, which "colours from the
    /// record" off takes away, or one painted from elsewhere.
    pub fn washes_window(cx: &App) -> bool {
        let theme = cx.global::<KoanTheme>();
        theme.wash_window && (theme.record_colours || theme.painted_wash) && !theme.opaque
    }

    pub fn set_wash_tint(percent: u32, cx: &mut App) {
        cx.global_mut::<KoanTheme>().wash_tint = percent.min(100);
    }

    /// The wash's drift: its pace in percent (0 still, up to 200) and how far
    /// it turns either way, in degrees (up to 10). The wash reads them when it
    /// is next drawn: whoever calls this draws the windows' washes again, as
    /// koan-gui's roots do when the pair changes.
    pub fn set_wash_drift(percent: u32, degrees: u32, cx: &mut App) {
        let theme = cx.global_mut::<KoanTheme>();
        theme.wash_drift = percent.min(200);
        theme.wash_turn = degrees.min(10);
    }

    /// Moves the window's grounds, which sit in cached views: every window
    /// is drawn again.
    pub fn set_wash_window(on: bool, cx: &mut App) {
        if cx.global::<KoanTheme>().wash_window != on {
            cx.global_mut::<KoanTheme>().wash_window = on;
            cx.refresh_windows();
        }
    }
}

/// The record's accent as it settles. An entity rather than part of the
/// global so that a change reaches the views that draw it, not every window.
pub struct AccentFeed {
    target: Accent,
    /// The accent as drawn: per appearance, a colour slot for fills and one
    /// for text, mixed by the GPU in OKLab as a new record's accent settles,
    /// so the hue turns rather than passing through grey and no frame is
    /// drawn for it. `None` when every slot was taken; the accent then lands
    /// with each record.
    slots: Option<[[crate::components::ColorSlot; 2]; 2]>,
    /// The contrast the slots were last aimed for, and the target as held
    /// for it, dark then light: what every `Look` reads, worked out once per
    /// aim rather than looked up per call.
    contrast: std::cell::Cell<bool>,
    held: std::cell::Cell<[Shade; 2]>,
}

impl AccentFeed {
    fn new() -> Self {
        let target = Accent::MINT;
        let claim = || crate::components::ColorSlot::claim(target.dark.rgba());
        let slots = (|| Some([[claim()?, claim()?], [claim()?, claim()?]]))();
        let feed = Self {
            target,
            slots,
            contrast: std::cell::Cell::new(false),
            held: std::cell::Cell::new([target.shade(true), target.shade(false)]),
        };
        feed.aim(false, false);
        feed
    }

    /// Whether the accent is still turning to its target.
    fn turning(&self) -> bool {
        let now = crate::components::present_now();
        self.slots
            .iter()
            .flatten()
            .flatten()
            .any(|slot| slot.moving(now))
    }

    /// Sends the slots to the target as it is drawn with `contrast`: as the
    /// wash dissolves to the record (`Motion::DISSOLVE`, in-out) when `ease`,
    /// at once otherwise.
    fn aim(&self, contrast: bool, ease: bool) {
        self.contrast.set(contrast);
        self.held
            .set([true, false].map(|dark| held(self.target.shade(dark), dark, contrast)));
        let Some(slots) = &self.slots else {
            return;
        };
        let track = Track {
            from: 0.0,
            to: 1.0,
            start: 0.0,
            length: Motion::DISSOLVE.as_secs_f32(),
            curve: crate::components::curve(crate::components::Ease::InOut),
            repeat: TrackRepeat::Once,
        };
        for ((dark, [fill, text]), shade) in
            [true, false].into_iter().zip(slots).zip(self.held.get())
        {
            let ink = Palette::for_look(dark, contrast).ink.hsla();
            let to = [shade.rgba(), shade.text_hsla(ink).to_rgb()];
            for (slot, to) in [fill, text].into_iter().zip(to) {
                if ease {
                    slot.to_oklab(to, track);
                } else {
                    slot.hold(to);
                }
            }
        }
    }
}

/// `shade` as drawn with `contrast`: held to `CONTRAST_ACCENT` when asked.
fn held(shade: Shade, dark: bool, contrast: bool) -> Shade {
    if contrast {
        shade.held(CONTRAST_ACCENT, dark, Palette::for_look(dark, contrast))
    } else {
        shade
    }
}

/// The accent a window draws, through the feed's slots where it has them.
/// A change of contrast lands the slots on the held shade at once, as does
/// motion stopping (Reduce Motion, Low Power Mode) mid-turn.
fn accent_shade(dark: bool, contrast: bool, cx: &App) -> Shade {
    let feed = cx.global::<AccentHandle>().0.read(cx);
    if feed.contrast.get() != contrast || (!KoanTheme::motion_allowed(cx) && feed.turning()) {
        feed.aim(contrast, false);
    }
    let mut shade = feed.held.get()[usize::from(!dark)];
    shade.slot = feed.slots.as_ref().map(|slots| {
        let [fill, text] = &slots[usize::from(!dark)];
        [fill.hsla(1.0), text.hsla(1.0)]
    });
    shade
}

struct AccentHandle(Entity<AccentFeed>);

impl Global for AccentHandle {}

/// Redraws a view when the accent changes in a way the colour slots cannot
/// carry: where no slots could be claimed, each record's accent. A cached
/// view that draws the accent holds the subscription for as long as it
/// lives; an uncached one redraws with its parent and needs none.
pub fn observe_accent<V: 'static>(cx: &mut Context<V>) -> Subscription {
    let feed = cx.global::<AccentHandle>().0.clone();
    cx.observe(&feed, |_, _, cx| cx.notify())
}

/// `observe_accent`, drawing the view again only while `draws` says it
/// shows the accent: a tile, which draws it only while its record plays.
pub fn observe_accent_when<V: 'static>(
    cx: &mut Context<V>,
    draws: impl Fn(&V) -> bool + 'static,
) -> Subscription {
    let feed = cx.global::<AccentHandle>().0.clone();
    cx.observe(&feed, move |view, _, cx| {
        if draws(view) {
            cx.notify();
        }
    })
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
    /// More contrast asked for: the palette is the contrast variant, the
    /// accent is held to 7:1, and a picked row is outlined as well as filled.
    pub contrast: bool,
    pub palette: Palette,
    pub accent: Shade,
    pub scale: Scale,
    pub icons: bool,
    /// A rule's width: `HAIRLINE`, twice that with more contrast.
    pub hairline: Pixels,
    /// Less transparency asked for: veils over the page are near opaque.
    pub opaque: bool,
    /// Used by touch (`KoanTheme::touch`).
    pub touch: bool,
    /// Under touch, a window narrower than `COMPACT_WIDTH`: a phone, or a
    /// tablet's window in Split View or Stage Manager with a phone's room.
    /// Layout follows this; what touch asks of a control follows `touch`.
    pub compact: bool,
}

impl Look {
    pub fn of(window: &Window, cx: &App) -> Look {
        let theme = cx.global::<KoanTheme>();
        let dark = theme
            .pinned
            .get(&window.window_handle().window_id())
            .copied()
            .unwrap_or_else(|| is_dark(window.appearance()));
        let contrast = theme.contrast;
        let palette = Palette::for_look(dark, contrast);
        let accent = accent_shade(dark, contrast, cx);
        Look {
            dark,
            contrast,
            palette,
            accent,
            scale: theme.scale(),
            icons: theme.icons,
            hairline: if contrast { HAIRLINE * 2.0 } else { HAIRLINE },
            opaque: theme.opaque,
            touch: theme.touch,
            compact: theme.touch && window.viewport_size().width < COMPACT_WIDTH,
        }
    }

    /// The least height of anything pressed: `HIT` under touch, nothing
    /// asked of a pointer.
    pub fn target(&self) -> Pixels {
        if self.touch { HIT } else { px(0.0) }
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

    /// Selected text's ground: the accent at 30 %, at 45 % with more
    /// contrast asked for.
    pub fn text_selection(&self) -> Hsla {
        self.accent
            .hsla()
            .opacity(if self.contrast { 0.45 } else { 0.3 })
    }

    /// A tone for text: the accent stays off text it cannot reach 4.5:1 as,
    /// which reads in `ink` instead.
    pub fn text_color(&self, tone: Tone) -> Hsla {
        match tone {
            Tone::Accent => self.accent.text_hsla(self.palette.ink.hsla()),
            tone => self.color(tone),
        }
    }

    /// The ground drawn over the page behind a sheet or the artwork viewer:
    /// `alpha` of `bg`, all but opaque with less transparency asked for.
    pub fn veil(&self, alpha: f32) -> Hsla {
        self.palette
            .bg
            .hsla()
            .opacity(if self.opaque { OPAQUE_VEIL } else { alpha })
    }

    pub fn space(&self, space: Space) -> Pixels {
        px(space.points() * self.scale.space)
    }

    /// A length on spacing's scale for a padding the spec gives in points
    /// that are not steps of the scale (a prominent button's 12 × 20).
    pub fn spaced(&self, points: f32) -> Pixels {
        px(points * self.scale.space)
    }

    /// A width that holds text, in points at 100 %, grown with the text
    /// size rather than the spacing.
    pub fn sized(&self, points: f32) -> Pixels {
        px(points * self.scale.size)
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
