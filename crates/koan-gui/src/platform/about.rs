//! Who the app is: kōan → About kōan, and on macOS the name and the icon a
//! build run from source (`cargo run`, outside a bundle) shows in the Dock,
//! the app switcher and the menu bar.

use koan_kit::app::App;

use super::actions::About;

/// The version, and the commit it was built from.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const BUILD: &str = env!("KOAN_BUILD");
const SITE: &str = "https://koan.rocks";
const SOURCE: &str = "https://github.com/radiosilence/koan";

/// The desktop app's icon as a build outside a bundle sets it, with the dev
/// spindle. AppKit draws a set image as it is, so this one sits on the macOS icon grid, where a
/// bundle's `.icns` is fitted to it by the system.
#[cfg(target_os = "macos")]
const ICON: &[u8] = include_bytes!("../../packaging/AppIcon-launch.png");

/// Answers About kōan.
pub fn init(cx: &mut App) {
    cx.on_action(|_: &About, cx| show(cx));
}

/// The name a build outside a bundle shows in the menu bar: set before the
/// application starts, which is when AppKit reads it. A bundle names itself.
#[cfg(target_os = "macos")]
pub fn name_unbundled() {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use objc2_foundation::NSString;
    // SAFETY: messages to AppKit's own classes on the main thread before
    // the application runs. Whether the bundle's dictionary is mutable
    // cannot be asked: a CF dictionary is of the mutable class either way.
    // An immutable one raises on `setObject:forKey:`, which `catch` turns
    // into an error before it unwinds into Rust.
    unsafe {
        let bundle: *mut AnyObject = msg_send![class!(NSBundle), mainBundle];
        let id: *mut AnyObject = msg_send![bundle, bundleIdentifier];
        if !id.is_null() {
            return;
        }
        let info: *mut AnyObject = msg_send![bundle, infoDictionary];
        if info.is_null() {
            return;
        }
        let name = NSString::from_str("kōan");
        let key = NSString::from_str("CFBundleName");
        // Nothing the closure touches outlives it if it unwinds.
        if objc2::exception::catch(std::panic::AssertUnwindSafe(|| {
            let _: () = msg_send![info, setObject: &*name, forKey: &*key];
        }))
        .is_err()
        {
            log::debug!(
                "the bundle's info dictionary is immutable; the menu bar keeps the binary's name"
            );
        }
    }
}

/// The icon a build outside a bundle shows in the Dock and the app switcher:
/// without one it is the system's generic executable. Once the application
/// is running.
#[cfg(target_os = "macos")]
pub fn icon_unbundled() {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    // SAFETY: as `name_unbundled`; the image is checked before it is set.
    unsafe {
        let bundle: *mut AnyObject = msg_send![class!(NSBundle), mainBundle];
        let id: *mut AnyObject = msg_send![bundle, bundleIdentifier];
        if !id.is_null() {
            return;
        }
        let data: *mut AnyObject = msg_send![class!(NSData), dataWithBytes: ICON.as_ptr().cast::<std::ffi::c_void>(), length: ICON.len()];
        let image: *mut AnyObject = msg_send![class!(NSImage), alloc];
        let image: *mut AnyObject = msg_send![image, initWithData: data];
        if image.is_null() {
            return;
        }
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, setApplicationIconImage: image];
        let _: () = msg_send![image, release];
    }
}

/// The standard About panel, as the Mac app shows it: the icon, the name, the
/// version and its build, and where kōan lives.
#[cfg(target_os = "macos")]
fn show(_cx: &mut App) {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use objc2_foundation::NSString;
    let credits = format!(
        "<div style=\"font: 11px -apple-system; text-align: center; color: gray\"><a href=\"{SITE}\">koan.rocks</a> · <a href=\"{SOURCE}\">Source</a></div>"
    );
    // SAFETY: AppKit's own panel, on the main thread, from an action.
    unsafe {
        let html = credits.as_bytes();
        let data: *mut AnyObject = msg_send![class!(NSData), dataWithBytes: html.as_ptr().cast::<std::ffi::c_void>(), length: html.len()];
        let null: *mut AnyObject = std::ptr::null_mut();
        let text: *mut AnyObject = msg_send![class!(NSAttributedString), alloc];
        let text: *mut AnyObject = msg_send![text, initWithHTML: data, documentAttributes: null];
        let keys = [
            NSString::from_str("ApplicationName"),
            NSString::from_str("ApplicationVersion"),
            NSString::from_str("Version"),
        ];
        let values = [
            NSString::from_str("kōan"),
            NSString::from_str(VERSION),
            NSString::from_str(BUILD),
        ];
        let options: *mut AnyObject = msg_send![class!(NSMutableDictionary), dictionary];
        for (k, v) in keys.iter().zip(&values) {
            let _: () = msg_send![options, setObject: &**v, forKey: &**k];
        }
        // The panel would take the bundle's icon, and a build outside one
        // has none to give, so the icon is given here, bundled or not.
        let icon: *mut AnyObject = msg_send![class!(NSData), dataWithBytes: ICON.as_ptr().cast::<std::ffi::c_void>(), length: ICON.len()];
        let image: *mut AnyObject = msg_send![class!(NSImage), alloc];
        let image: *mut AnyObject = msg_send![image, initWithData: icon];
        if !image.is_null() {
            let key = NSString::from_str("ApplicationIcon");
            let _: () = msg_send![options, setObject: image, forKey: &*key];
            let _: () = msg_send![image, release];
        }
        if !text.is_null() {
            let key = NSString::from_str("Credits");
            let _: () = msg_send![options, setObject: text, forKey: &*key];
            let _: () = msg_send![text, release];
        }
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, orderFrontStandardAboutPanelWithOptions: options];
        let _: () = msg_send![app, activateIgnoringOtherApps: true];
    }
}

/// Elsewhere, a window of its own with the same: the name, the version and
/// its build, and the links.
#[cfg(not(target_os = "macos"))]
fn show(cx: &mut App) {
    use koan_kit::app::{AppContext, IntoElement, ParentElement, Render, Styled, div};

    use crate::components::{Button, ButtonKind, SecondaryWindow};
    use crate::theme::{KoanStyled, Look, Space, Tone, TypeRole};

    struct AboutWindow;

    impl Render for AboutWindow {
        fn render(
            &mut self,
            window: &mut koan_kit::app::Window,
            cx: &mut koan_kit::app::Context<Self>,
        ) -> impl IntoElement {
            let look = Look::of(window, cx);
            div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(look.space(Space::M))
                .bg(look.palette.bg.hsla())
                .child(crate::components::title_text("kōan", &look))
                .child(
                    div()
                        .text_role(TypeRole::Meta, &look)
                        .text_color(look.text_color(Tone::Muted))
                        .child(format!("{VERSION} ({BUILD})")),
                )
                .child(
                    div()
                        .flex()
                        .gap(look.space(Space::L))
                        .child(
                            Button::new("about-site", ButtonKind::Link)
                                .text("koan.rocks")
                                .on_click(|_, _, cx| cx.open_url(SITE)),
                        )
                        .child(
                            Button::new("about-source", ButtonKind::Link)
                                .label("Source")
                                .on_click(|_, _, cx| cx.open_url(SOURCE)),
                        ),
                )
        }
    }

    /// The About window while it is open: asked again, it comes to the
    /// front rather than opening another.
    #[derive(Default)]
    struct Open(Option<koan_kit::app::WindowHandle<AboutWindow>>);

    impl koan_kit::app::Global for Open {}

    let open = cx.default_global::<Open>().0;
    if let Some(window) = open.filter(|w| cx.windows().contains(&(*w).into())) {
        window
            .update(cx, |_, window, _| window.activate_window())
            .ok();
        return;
    }
    let options = SecondaryWindow::new("About kōan", crate::app::APP_ID)
        .size(360.0, 220.0)
        .fixed()
        .options(cx);
    match cx.open_window(options, |_, cx| cx.new(|_| AboutWindow)) {
        Ok(window) => {
            cx.set_global(Open(Some(window)));
            crate::platform::residency::settle_dock(cx);
        }
        Err(e) => log::warn!("about: {e}"),
    }
}
