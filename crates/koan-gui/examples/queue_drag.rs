//! Drags a row of the queue in the real window, headlessly, and checks the
//! queue's order moved: the first track dropped below the third, grouped
//! by record as the queue opens or, with `KOAN_DRAG_FLAT`, flat. The queue
//! is laid out again mid-drag, as a queue sent whole is (#1823). GPUI's
//! headless renderer draws the window and the pointer is synthetic input
//! delivered to it; nothing on the screen is read.
//!
//! It opens a real engine, so it refuses to run without a throwaway
//! `KOAN_CONFIG_DIR`, whose library folders it scans first; the library
//! needs at least four tracks (`just gui-library DIR 2 3`).
//!
//! KOAN_CONFIG_DIR=… cargo run -p koan-gui --example queue_drag

use std::error::Error;
use std::time::Duration;

use koan_gui::mirror::Mirror;
use koan_gui::{app, root::WindowRoot, shell::Shell, theme};
use koan_kit::app::{
    AppContext, Bounds, Entity, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Point,
};
use koan_runtime::KoanEngine;
// GPUI's start-up, headless rendering and synthetic input, which only the
// app's entry and tools reach.
use koan_kit::raw::{AnyWindowHandle, HeadlessAppContext, PlatformInput, point, px, size};

fn main() -> Result<(), Box<dyn Error>> {
    koan_core::places::keep_with_config();
    if std::env::var_os("KOAN_CONFIG_DIR").is_none() {
        return Err("set KOAN_CONFIG_DIR to a throwaway configuration".into());
    }
    unsafe {
        std::env::set_var("KOAN_DEVICES__NEARBY", "false");
        std::env::set_var("KOAN_DEVICES__DISCOVERABLE", "false");
    }
    let engine = futures::executor::block_on(KoanEngine::open_desktop())?;
    futures::executor::block_on(engine.clone().scan_reporting(false, None))?;
    let mut ids: Vec<i64> = futures::executor::block_on(engine.clone().random_tracks(6, None))?
        .iter()
        .map(|track| track.id)
        .collect();
    ids.sort_unstable();
    if ids.len() < 4 {
        return Err("the library needs at least four tracks".into());
    }
    futures::executor::block_on(engine.clone().replace_queue(ids, None, false))?;
    futures::executor::block_on(engine.clone().pause())?;

    let text_system = koan_kit::raw::platform::current_platform(true).text_system();
    let mut cx = HeadlessAppContext::with_platform(
        text_system,
        std::sync::Arc::new(theme::icons::Assets),
        koan_kit::raw::platform::current_headless_renderer,
    );
    cx.update(|cx| {
        theme::init(cx);
        app::install(&engine, cx);
        koan_gui::library::bind_keys(cx);
    });
    let mut shell: Option<Entity<Shell>> = None;
    let window = cx.open_window(size(px(1200.), px(750.)), |window, cx| {
        let made = cx.new(|cx| Shell::new(window, cx));
        shell = Some(made.clone());
        let root = cx.new(|cx| WindowRoot::new(made, window, cx));
        koan_kit::components::host(root, window, cx)
    })?;
    let shell = shell.expect("the window made its shell");
    let window: AnyWindowHandle = window.into();
    cx.allow_parking();
    settle(&mut cx, window)?;

    // Grouped by record, as the queue opens, unless `KOAN_DRAG_FLAT` asks
    // for a row per track.
    let grouped = std::env::var_os("KOAN_DRAG_FLAT").is_none();
    cx.update(|cx| {
        koan_gui::window_state::GROUPED.store(grouped, std::sync::atomic::Ordering::Relaxed);
        let queue = cx.global::<Mirror>().queue.clone();
        koan_gui::mirror::regroup(&queue, grouped, cx);
    });
    settle(&mut cx, window)?;
    let order = |cx: &mut HeadlessAppContext| {
        cx.update(|cx| {
            let queue = cx.global::<Mirror>().queue.clone();
            queue
                .read(cx)
                .items
                .iter()
                .map(|item| item.queue_item_id.clone())
                .collect::<Vec<_>>()
        })
    };
    let before = order(&mut cx);
    // The rows of the queue's first and third tracks, under whatever
    // headings the layout puts above them.
    let row_of = |id: &String, cx: &mut HeadlessAppContext| {
        cx.update(|cx| {
            let queue = cx.global::<Mirror>().queue.clone();
            queue.read(cx).track_rows.get(id).copied()
        })
    };
    let (Some(first), Some(third)) = (row_of(&before[0], &mut cx), row_of(&before[2], &mut cx))
    else {
        return Err("the queue's tracks were not laid out".into());
    };
    let bounds = |row: usize, cx: &mut HeadlessAppContext| -> Option<Bounds<Pixels>> {
        cx.update(|cx| shell.read(cx).queue().read(cx).row_bounds(row))
    };
    let (Some(from), Some(to)) = (bounds(first, &mut cx), bounds(third, &mut cx)) else {
        return Err("the queue's rows were not drawn".into());
    };
    // Below the third row's middle: after it.
    let target = point(to.center().x, to.center().y + to.size.height / 4.);
    // The queue is sent whole while the drag lasts, as a download landing
    // or a rescan sends it: the drop still lands.
    drag(&mut cx, window, from.center(), target, |cx| {
        cx.update(|cx| {
            let queue = cx.global::<Mirror>().queue.clone();
            koan_gui::mirror::regroup(&queue, grouped, cx);
        });
        cx.run_until_parked();
    })?;
    settle(&mut cx, window)?;

    let after = order(&mut cx);
    let mut wanted = before.clone();
    let moved = wanted.remove(0);
    wanted.insert(2, moved);
    if after != wanted {
        eprintln!("before {before:?}\nafter  {after:?}\nwanted {wanted:?}");
        return Err("dragging the first row below the third did not reorder the queue".into());
    }
    println!("the queue reordered: the first row is now the third");
    Ok(())
}

/// Presses at `from`, moves to `to` in steps with the button held, as a
/// hand would, runs `meanwhile`, and lets go there.
fn drag(
    cx: &mut HeadlessAppContext,
    window: AnyWindowHandle,
    from: Point<Pixels>,
    to: Point<Pixels>,
    meanwhile: impl FnOnce(&mut HeadlessAppContext),
) -> Result<(), Box<dyn Error>> {
    let input = |cx: &mut HeadlessAppContext, event: PlatformInput| {
        cx.update_window(window, |_, window, cx| {
            window.dispatch_event(event, cx);
            window.draw(cx).clear(cx);
        })
    };
    input(
        cx,
        PlatformInput::MouseMove(MouseMoveEvent {
            position: from,
            pressed_button: None,
            modifiers: Modifiers::default(),
        }),
    )?;
    input(
        cx,
        PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position: from,
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        }),
    )?;
    const STEPS: usize = 8;
    for step in 1..=STEPS {
        let t = step as f32 / STEPS as f32;
        let at = point(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t);
        input(
            cx,
            PlatformInput::MouseMove(MouseMoveEvent {
                position: at,
                pressed_button: Some(MouseButton::Left),
                modifiers: Modifiers::default(),
            }),
        )?;
        cx.run_until_parked();
    }
    meanwhile(cx);
    input(
        cx,
        PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left,
            position: to,
            modifiers: Modifiers::default(),
            click_count: 1,
        }),
    )?;
    Ok(())
}

/// Lets what arrives from the background land: the queue, its rows.
fn settle(cx: &mut HeadlessAppContext, window: AnyWindowHandle) -> Result<(), Box<dyn Error>> {
    for _ in 0..3 {
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(400));
        cx.update_window(window, |_, window, _| window.refresh())?;
    }
    cx.run_until_parked();
    Ok(())
}
