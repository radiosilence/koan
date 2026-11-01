//! The EQ window: one for the app, opened from Playback → EQ… (⌥⌘E) on the
//! output in use, or on a device a preset menu names, and brought to the
//! front when it is open already.

use koan_kit::app::{App, AppContext, Bounds, Global, WindowHandle};
// the platform window and its options.
use koan_kit::raw::{TitlebarOptions, WindowBounds, WindowOptions};
// sizes and colours computed at runtime from measurements, not chosen from the theme.
use koan_kit::raw::{px, size};

use super::{Dsp, EqPage};
use crate::engine::engine;
use crate::platform::actions::ShowEq;
use crate::root::WindowRoot;

/// The EQ model, made the first time EQ is opened, and its window while it
/// is open.
struct EqWindow {
    dsp: koan_kit::app::Entity<Dsp>,
    window: Option<(WindowHandle<WindowRoot>, koan_kit::app::Entity<EqPage>)>,
}

impl Global for EqWindow {}

/// Answers Playback → EQ…. Call once the engine and the mirror are
/// installed.
pub fn init(cx: &mut App) {
    cx.on_action(|_: &ShowEq, cx| open(None, cx));
}

/// Shows EQ for `device`, or for the output in use.
pub fn open(device: Option<String>, cx: &mut App) {
    if !cx.has_global::<EqWindow>() {
        let dsp = Dsp::new(engine(cx), cx);
        cx.set_global(EqWindow { dsp, window: None });
    }
    let held = cx.global::<EqWindow>().window.clone();
    if let Some((handle, page)) = held {
        let shown = handle.update(cx, |_, window, cx| {
            window.activate_window();
            page.update(cx, |p, cx| p.show(device.clone(), cx));
        });
        // Closed since: a new one opens.
        if shown.is_ok() {
            return;
        }
    }
    let dsp = cx.global::<EqWindow>().dsp.clone();
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(760.), px(900.)),
            cx,
        ))),
        window_min_size: Some(size(px(560.), px(520.))),
        titlebar: Some(TitlebarOptions {
            title: Some("EQ".into()),
            ..Default::default()
        }),
        app_id: Some("koan-gui".into()),
        ..Default::default()
    };
    let mut made = None;
    let opened = cx.open_window(options, |window, cx| {
        let page = cx.new(|cx| EqPage::new(dsp, device, window, cx));
        made = Some(page.clone());
        cx.new(|cx| WindowRoot::new(page, window, cx))
    });
    match (opened, made) {
        (Ok(handle), Some(page)) => cx.global_mut::<EqWindow>().window = Some((handle, page)),
        (Err(e), _) => log::error!("the EQ window did not open: {e}"),
        _ => {}
    }
}
