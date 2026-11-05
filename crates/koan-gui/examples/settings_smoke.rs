//! Settings driven as a person drives it, against a real engine: the
//! sharing account made, sharing started and stopped, the port and where
//! invites go changed, then signing in to a server, out, and in again.
//! Every step goes through the window's own controls, pressed and typed into
//! through its accessibility tree, so the callbacks a click runs are the
//! ones a person's click runs. Exits non-zero on a panic anywhere in the
//! process, or when a step does not land.
//!
//! Headless: no window is shown and nothing on the screen is read. It
//! refuses a configuration that is not a throwaway one. The server it signs
//! in to is a throwaway too, named by `KOAN_SMOKE_SERVER`,
//! `KOAN_SMOKE_USER` and `KOAN_SMOKE_PASSWORD`; `just gui-settings-smoke`
//! starts one and prepares the configuration.

use std::error::Error;
use std::time::{Duration, Instant};

use koan_gui::settings::{Settings, SettingsWindow, Tab};
use koan_gui::{app, theme};
use koan_kit::app::AppContext;
use koan_runtime::KoanEngine;
// GPUI's start-up, headless rendering and the accessibility actions a screen
// reader sends, which only the app's entry and tools reach.
use koan_kit::raw::accesskit::{Action, ActionData, ActionRequest, NodeId, TreeId};
use koan_kit::raw::{AnyWindowHandle, HeadlessAppContext, px, size};
use serde_json::Value;

const WIDTH: f32 = 900.0;
const HEIGHT: f32 = 1400.0;

/// How long one step may take to land: a sign-in, a sync of an empty
/// library, a server starting.
const STEP: Duration = Duration::from_secs(60);
const BUDGET: Duration = Duration::from_secs(600);

type Result<T = ()> = std::result::Result<T, Box<dyn Error>>;

fn main() -> Result {
    koan_core::places::keep_with_config();
    // SAFETY: set before any other thread starts.
    unsafe {
        std::env::set_var("KOAN_DEVICES__NEARBY", "false");
        std::env::set_var("KOAN_DEVICES__DISCOVERABLE", "false");
    }
    throwaway()?;
    let server = std::env::var("KOAN_SMOKE_SERVER")
        .map_err(|_| "set KOAN_SMOKE_SERVER to a throwaway koan server")?;
    let user = std::env::var("KOAN_SMOKE_USER").unwrap_or_else(|_| "owner".into());
    let password = std::env::var("KOAN_SMOKE_PASSWORD")
        .map_err(|_| "set KOAN_SMOKE_PASSWORD to the server account's password")?;
    std::thread::spawn(|| {
        std::thread::sleep(BUDGET);
        eprintln!("settings smoke: still running after {BUDGET:?}: hung");
        std::process::exit(124);
    });
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default(info);
        std::process::exit(101);
    }));

    let engine = futures::executor::block_on(KoanEngine::open(None))?;
    let text_system = koan_kit::raw::platform::current_platform(true).text_system();
    let mut cx = HeadlessAppContext::with_platform(
        text_system,
        std::sync::Arc::new(theme::icons::Assets),
        koan_kit::raw::platform::current_headless_renderer,
    );
    cx.update(|cx| {
        theme::init(cx);
        app::install(&engine, cx);
        koan_gui::settings::resume(cx);
    });
    cx.allow_parking();
    let mut view = None;
    let window: AnyWindowHandle = cx
        .open_window(size(px(WIDTH), px(HEIGHT)), |window, cx| {
            let made = cx.new(|cx| SettingsWindow::new(Tab::Server, window, cx));
            view = Some(made.clone());
            koan_kit::components::host(made, window, cx)
        })?
        .into();
    let view = view.ok_or("the Settings window was not made")?;
    cx.update_window(window, |_, window, _| window.set_a11y_active_for_test(true))?;
    let mut ui = Ui { cx, window };
    ui.frame()?;
    if std::env::var_os("KOAN_SMOKE_TREE").is_some() {
        for node in ui.nodes()? {
            println!("{} {:?}", node["aria"]["role"], node["aria"]["label"]);
        }
        return Ok(());
    }

    // The account the library is shared under, made from the sharing side:
    // creating it starts sharing from inside its own callback (#2088).
    ui.choose("this device", "Shares its library")?;
    ui.type_into("Username", "smoke")?;
    ui.type_into("Password", "smoke-password")?;
    ui.type_into("Again", "smoke-password")?;
    ui.press("Create Account and Share")?;
    ui.until("sharing starts", |cx| serving(cx).is_some())?;
    println!("account made, sharing");

    // Where invites go: a tunnel's address, then this network again. Each
    // restarts the running server.
    ui.choose("invite phones to", "Custom address")?;
    ui.type_into("Address", "https://smoke.example")?;
    ui.commit("Address")?;
    ui.until("the address is written", |cx| {
        settings(cx).is_some_and(|s| s.server_url == "https://smoke.example")
    })?;
    ui.until("still serving after the change", |cx| serving(cx).is_some())?;
    ui.choose("invite phones to", "This network")?;
    ui.until("invites go to this network", |cx| {
        settings(cx).is_some_and(|s| s.share_invite_to == koan_runtime::InviteTo::Network)
    })?;
    println!("invites changed");

    // Another port, which restarts the server on it.
    let port = free_port()?;
    ui.type_into("Port", &port.to_string())?;
    ui.commit("Port")?;
    ui.until("sharing moves to the new port", |cx| {
        serving(cx).is_some_and(|p| p == port)
    })?;
    println!("moved to port {port}");

    // Off, on and off again, each asked of the switch.
    for on in [false, true, false] {
        ui.press("Share this device's library")?;
        if !on {
            ui.press("Stop Sharing")?;
        }
        ui.until(
            if on {
                "sharing starts"
            } else {
                "sharing stops"
            },
            |cx| serving(cx).is_some() == on,
        )?;
    }
    println!("sharing stopped, started and stopped");

    // Signing in to the throwaway server, out, and in again.
    ui.choose("this device", "Connects to a server")?;
    for round in 0..2 {
        ui.type_into("Server URL", &server)?;
        ui.type_into("Username", &user)?;
        ui.type_into("Password", &password)?;
        ui.press("Sign In")?;
        ui.until("signed in", |cx| {
            settings(cx).is_some_and(|s| s.remote_signed_in)
        })?;
        ui.press("Sign Out…")?;
        ui.press("Sign Out")?;
        ui.until("signed out", |cx| {
            settings(cx).is_some_and(|s| !s.remote_signed_in)
        })?;
        println!("signed in and out ({})", round + 1);
    }

    // Every pane drawn once more against what the flows left.
    for tab in [
        Tab::Library,
        Tab::Playback,
        Tab::Devices,
        Tab::Appearance,
        Tab::Server,
    ] {
        ui.cx.update_window(ui.window, |_, window, cx| {
            view.update(cx, |view, cx| view.choose(tab, window, cx))
        })?;
        ui.settle()?;
    }
    ui.cx
        .update_window(ui.window, |_, window, _| window.remove_window())?;
    ui.cx.run_until_parked();
    println!("no panics");
    Ok(())
}

fn settings(cx: &koan_kit::raw::App) -> Option<koan_runtime::Settings> {
    let model = cx.global::<Settings>().model.clone();
    model.read(cx).settings().cloned()
}

/// The port the library is served on, while it is.
fn serving(cx: &koan_kit::raw::App) -> Option<u16> {
    let sharing = cx.global::<Settings>().sharing.clone();
    let sharing = sharing.read(cx);
    sharing.serving.as_ref().map(|s| s.port)
}

fn free_port() -> Result<u16> {
    Ok(std::net::TcpListener::bind("127.0.0.1:0")?
        .local_addr()?
        .port())
}

/// Refuses any configuration but a throwaway one: not the person's own, and
/// holding no library but one `just gui-library` generated.
fn throwaway() -> Result {
    let Some(dir) = std::env::var_os("KOAN_CONFIG_DIR") else {
        return Err(
            "set KOAN_CONFIG_DIR to a throwaway configuration (just gui-settings-smoke)".into(),
        );
    };
    let dir = std::fs::canonicalize(&dir)?;
    let own = std::env::var_os("HOME")
        .map(|home| std::path::Path::new(&home).join(".config/koan"))
        .and_then(|own| std::fs::canonicalize(own).ok());
    if own.as_deref() == Some(dir.as_path()) {
        return Err("KOAN_CONFIG_DIR is your own configuration".into());
    }
    let config = koan_core::config::Config::load()?;
    for folder in &config.library.folders {
        if !folder.join(".koan-generated").exists() {
            return Err(format!(
                "{} is not a library `just gui-library` generated",
                folder.display()
            )
            .into());
        }
    }
    if !config.remote.url.is_empty() {
        return Err("the configuration is signed in to a server already".into());
    }
    Ok(())
}

/// The Settings window, as a screen reader reaches it.
struct Ui {
    cx: HeadlessAppContext,
    window: AnyWindowHandle,
}

impl Ui {
    /// A frame drawn, which rebuilds the accessibility tree.
    fn frame(&mut self) -> Result {
        self.cx.run_until_parked();
        self.cx
            .update_window(self.window, |_, window, cx| window.draw(cx).clear(cx))?;
        self.cx.run_until_parked();
        Ok(())
    }

    /// A few frames apart, for what lands off the main thread.
    fn settle(&mut self) -> Result {
        for _ in 0..5 {
            self.frame()?;
            std::thread::sleep(Duration::from_millis(50));
        }
        Ok(())
    }

    fn nodes(&mut self) -> Result<Vec<Value>> {
        let json = self
            .cx
            .update_window(self.window, |_, window, _| window.debug_a11y_tree_json())?
            .ok_or("no accessibility tree")?;
        let tree: Value = serde_json::from_str(&json)?;
        Ok(tree["nodes"]
            .as_object()
            .ok_or("no nodes")?
            .values()
            .cloned()
            .collect())
    }

    /// The node named `label` that `accepts` takes, waited for: a control
    /// appears once what it follows has landed. Labels are matched without
    /// case, since the theme may set them in lower case.
    fn find(&mut self, label: &str, accepts: impl Fn(&Value) -> bool) -> Result<NodeId> {
        let start = Instant::now();
        loop {
            self.frame()?;
            let nodes = self.nodes()?;
            let found: Vec<&Value> = nodes
                .iter()
                .filter(|n| {
                    n["aria"]["label"]
                        .as_str()
                        .is_some_and(|l| l.eq_ignore_ascii_case(label))
                        && accepts(n)
                })
                .collect();
            if let Some(node) = found.last() {
                let id = node["accesskit_id"]
                    .as_str()
                    .ok_or("a node without an id")?
                    .parse()?;
                return Ok(NodeId(id));
            }
            if start.elapsed() > STEP {
                let mut listed: Vec<String> = nodes
                    .iter()
                    .map(|n| {
                        let aria = &n["aria"];
                        format!(
                            "{} {} value={} disabled={}",
                            aria["role"], aria["label"], aria["value"], aria["disabled"]
                        )
                    })
                    .collect();
                listed.sort();
                return Err(format!("no {label} to act on, among:\n{}", listed.join("\n")).into());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn send(&mut self, target: NodeId, action: Action, data: Option<ActionData>) -> Result {
        self.cx.update_window(self.window, |_, window, cx| {
            window.dispatch_a11y_action(
                ActionRequest {
                    action,
                    target_tree: TreeId::ROOT,
                    target_node: target,
                    data,
                },
                cx,
            )
        })?;
        self.frame()
    }

    /// A button, switch or option pressed, once it is there and enabled.
    fn press(&mut self, label: &str) -> Result {
        let target = self.find(label, |n| n["aria"]["disabled"] != true)?;
        self.send(target, Action::Click, None)
    }

    /// A field's text replaced, as dictation or a screen reader sets it.
    fn type_into(&mut self, label: &str, text: &str) -> Result {
        let target = self.find(label, is_field)?;
        self.send(target, Action::Focus, None)?;
        self.send(
            target,
            Action::SetValue,
            Some(ActionData::Value(text.into())),
        )
    }

    /// A field committed as Return commits it: focus leaving it.
    fn commit(&mut self, label: &str) -> Result {
        let target = self.find(label, is_field)?;
        self.send(target, Action::Focus, None)?;
        self.cx.update_window(self.window, |_, window, cx| {
            window.dispatch_action(Box::new(koan_kit::components::field::Enter), cx)
        })?;
        self.frame()
    }

    /// A picker opened and one of its options chosen.
    fn choose(&mut self, picker: &str, option: &str) -> Result {
        self.press(picker)?;
        self.press(option)
    }

    /// Frames drawn until `done` holds, or the step's time runs out.
    fn until(&mut self, what: &str, done: impl Fn(&koan_kit::raw::App) -> bool) -> Result {
        let start = Instant::now();
        loop {
            self.frame()?;
            if self.cx.update(|cx| done(cx)) {
                return Ok(());
            }
            if start.elapsed() > STEP {
                let error = self.cx.update(|cx| {
                    let model = cx.global::<Settings>().model.clone();
                    let sharing = cx.global::<Settings>().sharing.clone();
                    format!(
                        "settings say {:?}; sharing failed with {:?}",
                        model.read(cx).last_error,
                        sharing.read(cx).failure
                    )
                });
                return Err(format!("{what} did not happen: {error}").into());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

fn is_field(node: &Value) -> bool {
    matches!(
        node["aria"]["role"].as_str(),
        Some("TextInput" | "PasswordInput" | "SearchInput" | "MultilineTextInput")
    )
}
