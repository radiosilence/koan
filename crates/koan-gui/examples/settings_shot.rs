//! Draws each Settings pane offscreen, dark, into a PNG apiece: the evidence
//! a change to Settings is checked against, beside the Mac app's
//! `SettingsEvidence`. No window is shown and nothing on the screen is read.
//!
//! It opens a real engine, so it refuses to run without a throwaway
//! `KOAN_CONFIG_DIR`. Panes that need a server (Account, People,
//! Integrations) are drawn when that configuration is signed in to one.
//!
//! KOAN_CONFIG_DIR=… cargo run -p koan-gui --example settings_shot -- target/settings

use std::error::Error;
use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::{AppContext, HeadlessAppContext, px, size};
use koan_ffi::KoanEngine;
use koan_gui::settings::{self, SettingsWindow, Tab};
use koan_gui::{app, theme};

const WIDTH: f32 = 900.0;
const HEIGHT: f32 = 820.0;

fn main() -> Result<(), Box<dyn Error>> {
    if std::env::var_os("KOAN_CONFIG_DIR").is_none() {
        return Err("set KOAN_CONFIG_DIR to a throwaway configuration".into());
    }
    let out = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "target/settings".into()),
    );
    std::fs::create_dir_all(&out)?;
    // SAFETY: set before any other thread starts.
    unsafe {
        std::env::set_var("KOAN_DEVICES__NEARBY", "false");
        std::env::set_var("KOAN_DEVICES__DISCOVERABLE", "false");
    }
    let engine = futures::executor::block_on(KoanEngine::open(None))?;

    let text_system = gpui_kit::platform::current_platform(true).text_system();
    let mut cx = HeadlessAppContext::with_platform(
        text_system,
        std::sync::Arc::new(theme::icons::Assets),
        gpui_kit::platform::current_headless_renderer,
    );
    cx.update(|cx| {
        gpui_kit::init(cx);
        theme::init(cx);
        app::install(&engine, cx);
    });
    cx.allow_parking();
    let settle = |cx: &mut HeadlessAppContext| {
        for _ in 0..3 {
            cx.run_until_parked();
            std::thread::sleep(Duration::from_millis(400));
        }
        cx.run_until_parked();
    };
    settle(&mut cx);

    let shoot = |cx: &mut HeadlessAppContext, tab: Tab, name: &str| -> Result<(), Box<dyn Error>> {
        let window = cx.open_window(size(px(WIDTH), px(HEIGHT)), |window, cx| {
            let view = cx.new(|cx| SettingsWindow::new(tab, window, cx));
            cx.new(|cx| gpui_kit::base::Root::new(view, window, cx))
        })?;
        cx.update_window(window.into(), |_, window, cx| {
            theme::pin_appearance(Some(true), window, cx)
        })?;
        settle(cx);
        cx.update_window(window.into(), |_, window, _| window.refresh())?;
        cx.run_until_parked();
        let path = out.join(format!("settings-{name}.png"));
        cx.capture_screenshot(window.into())?.save(&path)?;
        println!("{}", path.display());
        cx.update_window(window.into(), |_, window, _| window.remove_window())?;
        Ok(())
    };
    for (tab, name) in [
        (Tab::Library, "library"),
        (Tab::Server, "server"),
        (Tab::Account, "account"),
        (Tab::People, "people"),
        (Tab::Playback, "playback"),
        (Tab::Devices, "devices"),
        (Tab::Integrations, "integrations"),
        (Tab::Appearance, "appearance"),
    ] {
        // A tab the window does not offer opens as Server instead.
        let signed_in = cx.update(|cx| {
            let model = cx.global::<settings::Settings>().model.clone();
            model
                .read(cx)
                .settings()
                .is_some_and(|s| s.remote_signed_in)
        });
        if matches!(tab, Tab::Account | Tab::People | Tab::Integrations) && !signed_in {
            continue;
        }
        shoot(&mut cx, tab, name)?;
    }
    // The other side of the Server pane: the setting chooses which opens.
    cx.update(|cx| {
        let model = cx.global::<settings::Settings>().model.clone();
        model.update(cx, |m, cx| m.edit(cx, |s| s.share_enabled = true));
    });
    settle(&mut cx);
    shoot(&mut cx, Tab::Server, "server-share")?;
    cx.update(|cx| {
        let model = cx.global::<settings::Settings>().model.clone();
        model.update(cx, |m, cx| m.edit(cx, |s| s.share_enabled = false));
    });
    settle(&mut cx);
    Ok(())
}
