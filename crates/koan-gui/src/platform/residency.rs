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
    let engine = engine.clone();
    cx.spawn(async move |cx| {
        let keep = engine
            .settings()
            .await
            .is_ok_and(|s| s.devices_keep_running);
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
        crate::app::show_window(cx);
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
pub fn login() -> Login {
    login_item::status()
}

/// Asks the system to open kōan at login, or not. The error, worded for the
/// person, when it refuses.
pub fn set_login(on: bool) -> Result<(), String> {
    login_item::set(on)
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

    pub fn status() -> Login {
        let service = service();
        if service.is_null() {
            return Login::Unavailable;
        }
        // SAFETY: `status` is an NSInteger property of SMAppService.
        let status: isize = unsafe { msg_send![service, status] };
        match status {
            1 => Login::On,
            2 => Login::NeedsApproval,
            // Not found: no bundle to register, as for a build from source.
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

    pub fn status() -> Login {
        match entry() {
            Some(path) if path.exists() => Login::On,
            Some(_) => Login::Off,
            None => Login::Unavailable,
        }
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
            "[Desktop Entry]\nType=Application\nName=kōan\nExec=\"{exec}\"\nIcon=koan-gui\nX-GNOME-Autostart-enabled=true\n"
        );
        std::fs::create_dir_all(path.parent().unwrap_or(&path))
            .and_then(|()| std::fs::write(&path, text))
            .map_err(|e| e.to_string())
    }
}
