//! "add to playlist…" → "new playlist…" driven headlessly through the real
//! sheet, on the phone shell under touch and on the desktop shell: the
//! question for the name must follow, and answering it must make the
//! playlist and open it. Fails otherwise.
//!
//! It opens a real engine, so it refuses to run without a throwaway
//! `KOAN_CONFIG_DIR`, whose library folders it scans first; with
//! `NEW_PLAYLIST_SERVER=url,user,password` it signs in to that koan server and
//! syncs from it instead.
//!
//! KOAN_CONFIG_DIR=… cargo run -p koan-gui --example new_playlist [desktop]

use std::error::Error;
use std::time::{Duration, Instant};

use koan_gui::mobile::TouchShell;
use koan_gui::navigator::Route;
use koan_gui::shell::Shell;
use koan_gui::{app, root::WindowRoot, theme};
use koan_kit::app::{AppContext, Entity};
use koan_runtime::KoanEngine;
use serde_json::Value;
// GPUI's start-up, headless rendering and accessibility actions, which only
// the app's entry and tools reach.
use koan_kit::raw::accesskit::{Action, ActionData, ActionRequest, NodeId, TreeId};
use koan_kit::raw::{AnyWindowHandle, HeadlessAppContext, px, size};

type Result<T = ()> = std::result::Result<T, Box<dyn Error>>;

const STEP: Duration = Duration::from_secs(10);

fn main() -> Result {
    koan_core::places::keep_with_config();
    if std::env::var_os("KOAN_CONFIG_DIR").is_none() {
        return Err("set KOAN_CONFIG_DIR to a throwaway configuration".into());
    }
    // SAFETY: set before any other thread starts.
    unsafe {
        std::env::set_var("KOAN_DEVICES__NEARBY", "false");
        std::env::set_var("KOAN_DEVICES__DISCOVERABLE", "false");
    }
    let phone = std::env::args().nth(1).as_deref() != Some("desktop");
    if phone {
        // SAFETY: as above.
        unsafe { std::env::set_var("KOAN_GUI_PHONE", "1") };
    }
    let engine = futures::executor::block_on(KoanEngine::open(None))?;
    if let Ok(remote) = std::env::var("NEW_PLAYLIST_SERVER") {
        let mut parts = remote.splitn(3, ',').map(str::to_owned);
        let (url, user, pass) = (parts.next(), parts.next(), parts.next());
        futures::executor::block_on(engine.clone().sign_in_remote(
            url.ok_or("NEW_PLAYLIST_SERVER=url,user,password")?,
            user.ok_or("no user")?,
            pass.ok_or("no password")?,
        ))?;
        let summary = futures::executor::block_on(engine.clone().sync_remote())?;
        println!("synced {} tracks", summary.tracks);
    } else {
        futures::executor::block_on(engine.clone().scan_reporting(false, None))?;
    }
    let tracks: Vec<i64> = futures::executor::block_on(engine.clone().random_tracks(5, None))?
        .iter()
        .map(|t| t.id)
        .collect();
    if tracks.is_empty() {
        return Err("the configuration's library has no tracks".into());
    }
    let before = futures::executor::block_on(engine.clone().playlists())?.len();

    let text_system = koan_kit::raw::platform::current_platform(true).text_system();
    let mut cx = HeadlessAppContext::with_platform(
        text_system,
        std::sync::Arc::new(theme::icons::Assets),
        koan_kit::raw::platform::current_headless_renderer,
    );
    cx.update(|cx| {
        theme::init(cx);
        theme::KoanTheme::set_touch(phone, cx);
        app::install(&engine, cx);
        koan_gui::library::bind_keys(cx);
    });
    cx.allow_parking();

    let (w, h) = if phone { (402., 874.) } else { (1180., 760.) };
    let mut touch = None;
    let mut desk = None;
    let window: AnyWindowHandle = cx
        .open_window(size(px(w), px(h)), |window, cx| {
            if phone {
                let made = cx.new(|cx| TouchShell::new(window, cx));
                touch = Some(made.clone());
                let root = cx.new(|cx| WindowRoot::new(made, window, cx));
                koan_kit::components::host(root, window, cx)
            } else {
                let made = cx.new(|cx| Shell::new(window, cx));
                desk = Some(made.clone());
                let root = cx.new(|cx| WindowRoot::new(made, window, cx));
                koan_kit::components::host(root, window, cx)
            }
        })?
        .into();
    cx.update_window(window, |_, window, _| window.set_a11y_active_for_test(true))?;
    let mut ui = Ui { cx, window };
    ui.settle()?;

    let opened = |ui: &mut Ui, touch: &Option<Entity<TouchShell>>, desk: &Option<Entity<Shell>>| {
        let tracks = tracks.clone();
        ui.cx.update_window(ui.window, |_, window, cx| {
            if let Some(s) = touch {
                s.update(cx, |s, cx| s.walk_playlist_sheet(tracks, window, cx));
            } else if let Some(lib) = desk
                .as_ref()
                .and_then(|s| s.read(cx).library())
                .map(|l| l.read(cx).lib())
            {
                koan_gui::library::pick_playlist::open_tracks(&lib, tracks, window, cx);
            }
        })
    };
    opened(&mut ui, &touch, &desk)?;
    ui.settle()?;
    // As on a display: the frame after the click draws before what the click
    // started in the background lands.
    let target = ui.find("new playlist…", |n| n["aria"]["disabled"] != true)?;
    ui.cx.update_window(ui.window, |_, window, cx| {
        window.dispatch_a11y_action(
            ActionRequest {
                action: Action::Click,
                target_tree: TreeId::ROOT,
                target_node: target,
                data: None,
            },
            cx,
        );
        window.draw(cx).clear(cx);
    })?;
    ui.frame()?;
    let field = ui.find("name", is_field)?;
    ui.send(field, Action::Focus, None)?;
    ui.send(
        field,
        Action::SetValue,
        Some(ActionData::Value("Made here".into())),
    )?;
    ui.press("create")?;
    let start = Instant::now();
    loop {
        ui.frame()?;
        let route = ui.cx.update(|cx| {
            touch.as_ref().map(|s| s.read(cx).route(cx)).or_else(|| {
                let lib = desk.as_ref()?.read(cx).library()?.read(cx).lib();
                let navigator = lib.nav.read(cx).navigator();
                Some(navigator.read(cx).current())
            })
        });
        if let Some(Route::Playlist(id)) = route {
            let after = futures::executor::block_on(engine.clone().playlists())?;
            let made = after
                .iter()
                .find(|p| p.id == id)
                .ok_or("no such playlist")?;
            println!(
                "made “{}” with {} tracks ({} playlists before, {} after)",
                made.name,
                made.track_count,
                before,
                after.len()
            );
            return Ok(());
        }
        if start.elapsed() > STEP {
            return Err(format!("the playlist was not opened: at {route:?}").into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

struct Ui {
    cx: HeadlessAppContext,
    window: AnyWindowHandle,
}

impl Ui {
    fn frame(&mut self) -> Result {
        self.cx.run_until_parked();
        self.cx
            .update_window(self.window, |_, window, cx| window.draw(cx).clear(cx))?;
        self.cx.run_until_parked();
        Ok(())
    }

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

    fn find(&mut self, label: &str, accepts: impl Fn(&Value) -> bool) -> Result<NodeId> {
        let start = Instant::now();
        loop {
            self.frame()?;
            let nodes = self.nodes()?;
            let found = nodes.iter().rfind(|n| {
                n["aria"]["label"]
                    .as_str()
                    .is_some_and(|l| l.eq_ignore_ascii_case(label))
                    && accepts(n)
            });
            if let Some(node) = found {
                let id = node["accesskit_id"]
                    .as_str()
                    .ok_or("a node without an id")?
                    .parse()?;
                return Ok(NodeId(id));
            }
            if start.elapsed() > STEP {
                let mut listed: Vec<String> = nodes
                    .iter()
                    .map(|n| format!("{} {}", n["aria"]["role"], n["aria"]["label"]))
                    .collect();
                listed.sort();
                return Err(
                    format!("no {label:?} to act on, among:\n{}", listed.join("\n")).into(),
                );
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

    fn press(&mut self, label: &str) -> Result {
        let target = self.find(label, |n| n["aria"]["disabled"] != true)?;
        self.send(target, Action::Click, None)
    }
}

fn is_field(node: &Value) -> bool {
    matches!(
        node["aria"]["role"].as_str(),
        Some("TextInput" | "SearchInput")
    )
}
