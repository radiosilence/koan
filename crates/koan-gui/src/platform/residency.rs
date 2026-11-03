//! Staying running with the window closed, and opening at login, as the Mac
//! app's `Residency` does.
//!
//! A computer cannot be woken from another device: once kōan quits it is out
//! of reach until someone opens it. Kept running (`devices.keep_running`),
//! closing the window leaves kōan signed in to the server and listening on
//! the network, so the account's other devices can see and control this one;
//! on macOS it then leaves the Dock and the app switcher and lives in the
//! menu bar (`status_item`), and returns to them when its window opens.
//! Opening at login is the system's to keep (macOS's login items, an XDG
//! autostart entry on Linux), so it is asked of the system rather than kept
//! in the configuration.

use koan_kit::app::*;

/// What residency is now, for the window and the Settings pane.
#[derive(Default)]
pub struct Residency {
    pub keep_running: bool,
    /// Why the system last refused to change the login item, for Settings.
    pub login_error: Option<String>,
    /// Whether the system opens kōan at login, as last read: asking is an
    /// XPC round trip on macOS and a file read on Linux, which no frame
    /// waits on.
    login: Option<Login>,
    /// The last read or change of the login item, which the next waits for:
    /// they reach the system and land in the order asked.
    login_lane: Option<Task<()>>,
    #[cfg(target_os = "macos")]
    status: Option<super::status_item::StatusItem>,
}

impl Global for Residency {}

impl Residency {
    /// The menu bar item, while it shows.
    #[cfg(target_os = "macos")]
    pub(super) fn status_mut(&mut self) -> Option<&mut super::status_item::StatusItem> {
        self.status.as_mut()
    }
}

/// Whether the system opens kōan at login.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Login {
    Off,
    On,
    /// Registered, waiting for approval in System Settings.
    NeedsApproval,
    /// This build cannot be a login item: one run from source, outside a
    /// bundle.
    Unavailable,
}

/// Reads `devices.keep_running` and applies it. Once the engine is open.
pub fn init(engine: &std::sync::Arc<koan_ffi::KoanEngine>, cx: &mut App) {
    cx.set_global(Residency::default());
    set(false, cx);
    read_login(cx);
    let engine = engine.clone();
    cx.spawn(async move |cx| {
        let keep = engine.settings().await.devices_keep_running;
        cx.update(|cx| set(keep, cx));
    })
    .detach();
}

/// Whether kōan keeps running with its window closed.
pub fn keep_running(cx: &App) -> bool {
    cx.try_global::<Residency>().is_some_and(|r| r.keep_running)
}

/// Turns staying running on or off, as Settings writes it: the quit rule,
/// the menu bar item, and with it off and the main window closed, the
/// window, so closing Settings does not quit kōan with nothing to bring
/// back.
pub fn apply(keep: bool, cx: &mut App) {
    set(keep, cx);
    if !keep && crate::app::main_window(cx).is_none() {
        let _ = crate::app::show_window(cx);
    }
}

/// The quit rule and the menu bar item for `keep`.
fn set(keep: bool, cx: &mut App) {
    cx.set_quit_mode(if keep {
        QuitMode::Explicit
    } else {
        QuitMode::LastWindowClosed
    });
    cx.global_mut::<Residency>().keep_running = keep;
    #[cfg(target_os = "macos")]
    {
        let shown = cx.global::<Residency>().status.is_some();
        if keep && !shown {
            let item = super::status_item::StatusItem::install(cx);
            cx.global_mut::<Residency>().status = item;
        } else if !keep && shown {
            cx.global_mut::<Residency>().status = None;
        }
    }
}

/// The main window is closing: kept running, kōan leaves the Dock and the
/// app switcher for the menu bar.
pub fn window_closing(cx: &mut App) {
    if keep_running(cx) {
        log::info!("window closed; staying in the menu bar");
        set_in_dock(false);
    }
}

/// The main window is opening: kōan is in the Dock again.
pub fn window_opening() {
    set_in_dock(true);
}

#[cfg(target_os = "macos")]
fn set_in_dock(shown: bool) {
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    let Some(mtm) = objc2::MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    let policy = if shown {
        NSApplicationActivationPolicy::Regular
    } else {
        NSApplicationActivationPolicy::Accessory
    };
    if app.activationPolicy() != policy {
        app.setActivationPolicy(policy);
    }
}

#[cfg(not(target_os = "macos"))]
fn set_in_dock(_: bool) {}

/// Whether the system opens kōan at login, as it says now: it can be turned
/// off in the system's settings without kōan being told.
pub fn login(cx: &App) -> Login {
    cx.try_global::<Residency>()
        .and_then(|r| r.login.clone())
        .unwrap_or(Login::Unavailable)
}

/// Asks the system whether it opens kōan at login, off the main thread, and
/// redraws what shows it. Settings asks again as it comes to the front: the
/// item can be turned off in the system's settings without kōan being told.
pub fn read_login(cx: &mut App) {
    on_login_lane(None, cx);
}

/// Asks the system to open kōan at login, or not, off the main thread,
/// keeping why it refused for Settings to show, then reads what it says now.
pub fn set_login(on: bool, cx: &mut App) {
    on_login_lane(Some(on), cx);
}

/// A change of the login item (`Some`), or none, then a read of it, after
/// whatever was asked before has landed: two quick toggles reach the system
/// in order, and the status shown is the last one read.
fn on_login_lane(change: Option<bool>, cx: &mut App) {
    if !cx.has_global::<Residency>() {
        return;
    }
    let before = cx.global_mut::<Residency>().login_lane.take();
    let lane = cx.spawn(async move |cx| {
        if let Some(before) = before {
            before.await;
        }
        // Started only now, so the system is asked one thing at a time.
        let (changed, refused, login) = cx
            .background_executor()
            .spawn(async move {
                let refused = change.and_then(|on| login_item::set(on).err());
                (change.is_some(), refused, login_item::status())
            })
            .await;
        cx.update(|cx| {
            if cx.has_global::<Residency>() {
                let residency = cx.global_mut::<Residency>();
                if changed {
                    residency.login_error = refused;
                }
                residency.login = Some(login);
                cx.refresh_windows();
            }
        });
    });
    cx.global_mut::<Residency>().login_lane = Some(lane);
}

/// Why the system last refused to change the login item.
pub fn login_error(cx: &App) -> Option<String> {
    cx.try_global::<Residency>()?.login_error.clone()
}

/// Whether this run was started by the system at login rather than by
/// someone: the one launch that, kept running, starts in the menu bar with
/// no window.
pub fn launched_at_login() -> bool {
    login_item::launched_at_login()
}

#[cfg(target_os = "macos")]
mod login_item {
    //! macOS's login items, through `SMAppService.mainAppService`.
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};

    use super::Login;

    #[link(name = "ServiceManagement", kind = "framework")]
    unsafe extern "C" {}

    fn service() -> *mut AnyObject {
        // SAFETY: a class method of a class the linked framework defines.
        unsafe { msg_send![class!(SMAppService), mainAppService] }
    }

    /// A build run from source has no bundle to register as a login item.
    fn bundled() -> bool {
        // SAFETY: NSBundle's main bundle and its identifier, an NSString or
        // nil.
        unsafe {
            let bundle: *mut AnyObject = msg_send![class!(NSBundle), mainBundle];
            let id: *mut AnyObject = msg_send![bundle, bundleIdentifier];
            !id.is_null()
        }
    }

    /// The launch's Apple event says it was opened as a login item
    /// (`keyAEPropData` holding `keyAELaunchedAsLogInItem`). Read as the app finishes launching,
    /// while that event is still the current one.
    pub fn launched_at_login() -> bool {
        static AT_LOGIN: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *AT_LOGIN.get_or_init(|| {
            // The open-application event carries how it was launched as the
            // enum in its `keyAEPropData` ('prop') parameter, which is
            // `keyAELaunchedAsLogInItem` ('lgit') for a login item.
            const PROP_DATA: u32 = u32::from_be_bytes(*b"prop");
            const LOGIN_ITEM: u32 = u32::from_be_bytes(*b"lgit");
            // SAFETY: the shared Apple event manager's current event, and a
            // parameter of it, each an object or nil; `enumCodeValue` is an
            // OSType (u32) property of a descriptor.
            unsafe {
                let manager: *mut AnyObject =
                    msg_send![class!(NSAppleEventManager), sharedAppleEventManager];
                let event: *mut AnyObject = msg_send![manager, currentAppleEvent];
                if event.is_null() {
                    return false;
                }
                let param: *mut AnyObject = msg_send![event, paramDescriptorForKeyword: PROP_DATA];
                if param.is_null() {
                    return false;
                }
                let code: u32 = msg_send![param, enumCodeValue];
                code == LOGIN_ITEM
            }
        })
    }

    pub fn status() -> Login {
        let service = service();
        if service.is_null() || !bundled() {
            return Login::Unavailable;
        }
        // SAFETY: `status` is an NSInteger property of SMAppService.
        let status: isize = unsafe { msg_send![service, status] };
        match status {
            1 => Login::On,
            2 => Login::NeedsApproval,
            3 => Login::Unavailable,
            _ => Login::Off,
        }
    }

    pub fn set(on: bool) -> Result<(), String> {
        let service = service();
        if service.is_null() {
            return Err("this build cannot open at login".into());
        }
        // SAFETY: SMAppService's register and unregister, which answer NO
        // with an NSError on failure.
        let done: Result<(), objc2::rc::Retained<objc2_foundation::NSError>> = unsafe {
            if on {
                msg_send![service, registerAndReturnError: _]
            } else {
                msg_send![service, unregisterAndReturnError: _]
            }
        };
        let Err(error) = done else {
            return Ok(());
        };
        let described = error.localizedDescription().to_string();
        log::error!(
            "login item: {} failed: {described}",
            if on { "register" } else { "unregister" }
        );
        Err(described)
    }
}

#[cfg(not(target_os = "macos"))]
mod login_item {
    //! An XDG autostart entry, `$XDG_CONFIG_HOME/autostart/koan-gui.desktop`,
    //! which the desktop session opens at login.
    use std::path::PathBuf;

    use super::Login;

    /// What the autostart entry passes, so a login launch is told apart.
    const BACKGROUND: &str = "--background";

    pub fn launched_at_login() -> bool {
        std::env::args().any(|arg| arg == BACKGROUND)
    }

    fn entry() -> Option<PathBuf> {
        Some(
            dirs::config_dir()?
                .join("autostart")
                .join("koan-gui.desktop"),
        )
    }

    /// What the session starts: the AppImage when running from one, since
    /// the path inside it changes each run, else this executable.
    fn exec() -> Option<String> {
        std::env::var("APPIMAGE")
            .ok()
            .or_else(|| Some(std::env::current_exe().ok()?.display().to_string()))
    }

    /// On only where the entry is there, the session has not been told to
    /// skip it (`Hidden`, GNOME's own switch) and it starts this kōan: an
    /// AppImage moved since leaves an entry that starts nothing.
    pub fn status() -> Login {
        let Some(path) = entry() else {
            return Login::Unavailable;
        };
        match std::fs::read_to_string(path) {
            Ok(text) if starts(&text, exec().as_deref()) => Login::On,
            _ => Login::Off,
        }
    }

    /// Whether an entry `text` starts `exec` at login.
    fn starts(text: &str, exec: Option<&str>) -> bool {
        let value = |key: &str| {
            text.lines()
                .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
                .map(str::trim)
        };
        value("Hidden") != Some("true")
            && value("X-GNOME-Autostart-enabled") != Some("false")
            && exec.is_some_and(|exec| {
                value("Exec").is_some_and(|line| line.starts_with(&quoted(exec)))
            })
    }

    /// `path` as a quoted argument of a desktop entry's `Exec`, as the Desktop
    /// Entry specification asks: `"`, `` ` ``, `$` and `\` escaped inside
    /// the quotes and `%` doubled, then every backslash escaped again for the
    /// value's string escaping, which a reader undoes first.
    fn quoted(path: &str) -> String {
        let mut arg = String::from("\"");
        for c in path.chars() {
            match c {
                '"' | '`' | '$' | '\\' => {
                    arg.push('\\');
                    arg.push(c);
                }
                '%' => arg.push_str("%%"),
                c => arg.push(c),
            }
        }
        arg.push('"');
        // The value's own escaping comes first when it is read: every
        // backslash the quoting put in is itself escaped.
        arg.replace('\\', "\\\\")
    }

    pub fn set(on: bool) -> Result<(), String> {
        let path = entry().ok_or("there is no configuration folder to put it in")?;
        if !on {
            return match std::fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.to_string()),
            };
        }
        let exec = exec().ok_or("could not tell where kōan is")?;
        let text = format!(
            "[Desktop Entry]\nType=Application\nName=kōan\nExec={} {BACKGROUND}\nIcon=koan-gui\nX-GNOME-Autostart-enabled=true\n",
            quoted(&exec)
        );
        std::fs::create_dir_all(path.parent().unwrap_or(&path))
            .and_then(|()| std::fs::write(&path, text))
            .map_err(|e| e.to_string())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use core::prelude::v1::test;

        #[test]
        fn an_exec_path_is_quoted_as_the_spec_asks() {
            assert_eq!(quoted("/opt/kōan/koan-gui"), "\"/opt/kōan/koan-gui\"");
            assert_eq!(
                quoted("/home/a b/50%/$x\"`\\"),
                "\"/home/a b/50%%/\\\\$x\\\\\"\\\\`\\\\\\\\\""
            );
        }

        #[test]
        fn an_entry_is_on_only_where_it_starts_this_koan() {
            let exec = "/opt/koan/koan-gui";
            let entry = |extra: &str| {
                format!(
                    "[Desktop Entry]\nExec={} --background\n{extra}",
                    quoted(exec)
                )
            };
            assert!(starts(&entry(""), Some(exec)));
            assert!(!starts(&entry("Hidden=true\n"), Some(exec)));
            assert!(!starts(
                &entry("X-GNOME-Autostart-enabled=false\n"),
                Some(exec)
            ));
            assert!(
                !starts(&entry(""), Some("/elsewhere/koan-gui")),
                "moved since"
            );
        }
    }
}
