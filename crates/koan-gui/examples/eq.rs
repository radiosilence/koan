//! The EQ page against a real engine: offscreen into a PNG, dark and light
//! side by side, or with `--window` in a window of its own. It seeds a
//! correction, two EQs and a preset for a device called "Desk DAC" the first
//! time it runs on a configuration, so there is a chain to draw.
//!
//! It opens a real engine, so it refuses to run without a throwaway
//! `KOAN_CONFIG_DIR` whose `config.toml` says `folders = []`, and refuses
//! the user's own.
//!
//! KOAN_CONFIG_DIR=… cargo run -p koan-gui --example eq -- target/eq.png
//! KOAN_CONFIG_DIR=… cargo run -p koan-gui --example eq -- --window
//! KOAN_CONFIG_DIR=… cargo run -p koan-gui --example eq -- --profile Warm target/warm.png
//! KOAN_CONFIG_DIR=… cargo run -p koan-gui --example eq -- --sheet manage target/manage.png
//! KOAN_APPEARANCE__TEXT_SIZE=130 KOAN_CONFIG_DIR=… cargo run -p koan-gui --example eq -- target/eq-130.png
//!
//! `--sheet` opens one of the EQ window's sheets over the page: `manage`
//! (Manage EQ), `autoeq` (the AutoEQ search), `measure` (a measurement's
//! import) or `role` (the question an import asks of what it made).
//! The text size is the configuration's, which `KOAN_APPEARANCE__TEXT_SIZE`
//! sets for a run.
//! KOAN_CONFIG_DIR=… cargo run --release -p koan-gui --example eq -- --measure
//!
//! `--measure` opens Warm's page offscreen, drags its third band's handle
//! across the graph in 600 steps, each delivered to the window as a mouse
//! move, and prints how long each step took on the main thread: the move's
//! handling and the frame drawn after it, with whatever previews landed
//! since the last step applied before it.

use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use koan_gui::dsp::{Dsp, EqPage, profile::ProfilePage};
use koan_gui::theme;
use koan_kit::app::{AppContext, Bounds};
use koan_runtime::{DspStarter, DspTuningEntry, KoanEngine};
// GPUI's start-up and headless rendering, which only the app's entry and tools reach.
use koan_kit::raw::{HeadlessAppContext, WindowBounds, WindowOptions, px, size};

const WIDTH: f32 = 900.0;
const HEIGHT: f32 = 1100.0;
const DEVICE: &str = "Desk DAC";

/// An AutoEQ result as Equalizer APO text: what a correction looks like.
const HD600: &str = "Preamp: -6.2 dB
Filter 1: ON LSC Fc 105 Hz Gain 5.5 dB Q 0.70
Filter 2: ON PK Fc 156 Hz Gain -3.1 dB Q 0.53
Filter 3: ON PK Fc 2416 Hz Gain 3.4 dB Q 1.93
Filter 4: ON PK Fc 3400 Hz Gain -2.5 dB Q 4.00
Filter 5: ON PK Fc 5426 Hz Gain -4.3 dB Q 3.79
Filter 6: ON HSC Fc 10000 Hz Gain -2.4 dB Q 0.70";

/// Refuses a configuration that is not a throwaway one: unset, the user's
/// own, or one naming library folders, which a scan would read.
fn throwaway() -> Result<(), Box<dyn Error>> {
    let dir = std::env::var_os("KOAN_CONFIG_DIR")
        .ok_or("set KOAN_CONFIG_DIR to a throwaway configuration")?;
    let dir = std::path::PathBuf::from(dir);
    if let Some(home) = std::env::var_os("HOME")
        && dir.canonicalize().ok()
            == std::path::Path::new(&home)
                .join(".config/koan")
                .canonicalize()
                .ok()
    {
        return Err("KOAN_CONFIG_DIR is your own configuration; use a throwaway one".into());
    }
    let config = std::fs::read_to_string(dir.join("config.toml")).unwrap_or_default();
    if !config.contains("folders = []") {
        return Err(
            "the throwaway configuration's config.toml must say `folders = []` under [library]"
                .into(),
        );
    }
    Ok(())
}

/// Runs the app until the engine's answers have all landed: each await on
/// the engine is woken from its own threads, so the loop sleeps between
/// turns.
fn settle(
    cx: &mut HeadlessAppContext,
    window: koan_kit::app::AnyWindowHandle,
) -> Result<(), Box<dyn Error>> {
    for _ in 0..40 {
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(50));
    }
    cx.update_window(window, |_, window, _| window.refresh())?;
    cx.run_until_parked();
    Ok(())
}

fn seed(engine: &Arc<KoanEngine>) -> Result<(), Box<dyn Error>> {
    use futures::executor::block_on as block;
    let overview = block(engine.clone().dsp_overview());
    if overview.profiles.iter().any(|p| p.name == "Warm") {
        return Ok(());
    }
    let hd600 = block(
        engine
            .clone()
            .dsp_import_text(HD600.into(), Some("HD 600".into()), None),
    )?;
    block(
        engine
            .clone()
            .dsp_set_role(hd600.clone(), koan_runtime::DspRole::Correction),
    )?;
    block(
        engine
            .clone()
            .dsp_set_made_for(hd600.clone(), Some("harman-over-ear-2018".into())),
    )?;
    block(
        engine
            .clone()
            .dsp_create_eq("Warm".into(), DspStarter::Warm),
    )?;
    block(
        engine
            .clone()
            .dsp_create_eq("Bright".into(), DspStarter::Bright),
    )?;
    block(
        engine
            .clone()
            .dsp_set_tuned_for("Warm".into(), Some("harman-over-ear-2018".into())),
    )?;
    block(engine.clone().dsp_assign_device(DEVICE.into(), Some(hd600)))?;
    block(engine.clone().dsp_set_tunings(
        DEVICE.into(),
        vec![
            DspTuningEntry {
                name: "Warm".into(),
                on: true,
            },
            DspTuningEntry {
                name: "Bright".into(),
                on: true,
            },
        ],
    ))?;
    block(
        engine
            .clone()
            .dsp_save_preset(DEVICE.into(), "Evening".into()),
    )?;
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    koan_core::places::keep_with_config();
    throwaway()?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let window_mode = args.iter().any(|a| a == "--window");
    let measure = args.iter().any(|a| a == "--measure");
    let after = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1).cloned())
    };
    let profile = after("--profile");
    let sheet = after("--sheet");
    let out = args
        .iter()
        .rev()
        .find(|a| a.ends_with(".png"))
        .cloned()
        .unwrap_or_else(|| "target/eq.png".into());

    // SAFETY: set before any other thread starts.
    unsafe {
        std::env::set_var("KOAN_DEVICES__NEARBY", "false");
        std::env::set_var("KOAN_DEVICES__DISCOVERABLE", "false");
    }
    let engine = futures::executor::block_on(KoanEngine::open(None))?;
    seed(&engine)?;

    if window_mode {
        // GPUI's start-up and headless rendering, which only the app's entry and tools reach.
        koan_kit::raw::application()
            .with_assets(theme::icons::Assets)
            .run(move |cx| {
                // GPUI's start-up and headless rendering, which only the app's entry and tools reach.
                koan_kit::raw::init(cx);
                theme::init(cx);
                koan_gui::app::install(&engine, cx);
                let dsp = Dsp::new(engine.clone(), cx);
                let bounds = Bounds::centered(None, size(px(WIDTH), px(HEIGHT)), cx);
                // GPUI's start-up and headless rendering, which only the app's entry and tools reach.
                koan_kit::raw::open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        ..Default::default()
                    },
                    cx,
                    move |window, cx| {
                        cx.new(|cx| EqPage::new(dsp.clone(), Some(DEVICE.into()), window, cx))
                    },
                )
                .expect("the EQ window opens");
                cx.activate(true);
            });
        return Ok(());
    }

    // geometry and colours computed at runtime, not chosen from the theme.
    let text_system = koan_kit::raw::platform::current_platform(true).text_system();
    let mut cx = HeadlessAppContext::with_platform(
        text_system,
        Arc::new(theme::icons::Assets),
        // geometry and colours computed at runtime, not chosen from the theme.
        koan_kit::raw::platform::current_headless_renderer,
    );
    cx.update(|cx| {
        // GPUI's start-up and headless rendering, which only the app's entry and tools reach.
        koan_kit::raw::init(cx);
        theme::init(cx);
        koan_gui::app::install(&engine, cx);
    });
    let dsp = cx.update(|cx| Dsp::new(engine.clone(), cx));
    if sheet.as_deref() == Some("role") {
        cx.update(|cx| {
            dsp.update(cx, |d, cx| {
                d.ask_role = Some(koan_gui::dsp::model::RoleAsk {
                    names: vec!["HD 600".into()],
                    profiles: vec!["HD 600".into()],
                    into: None,
                });
                cx.notify();
            })
        });
    }
    if measure {
        return drag(&mut cx, dsp);
    }
    let mut shots = Vec::new();
    for dark in [true, false] {
        let (dsp, profile, sheet) = (dsp.clone(), profile.clone(), sheet.clone());
        let window = cx.open_window(size(px(WIDTH), px(HEIGHT)), move |window, cx| {
            let page = cx.new(|cx| EqPage::new(dsp.clone(), Some(DEVICE.into()), window, cx));
            if let Some(name) = profile {
                let sheets = page.read(cx).sheets.clone();
                let view = cx.new(|cx| {
                    ProfilePage::new(
                        dsp.clone(),
                        sheets.clone(),
                        name,
                        Some(DEVICE.into()),
                        window,
                        cx,
                    )
                });
                sheets.update(cx, |s, cx| s.view(view, cx));
            }
            let sheets = page.read(cx).sheets.clone();
            let view: Option<koan_kit::app::AnyView> = match sheet.as_deref() {
                Some("manage") => Some(
                    cx.new(|cx| {
                        koan_gui::dsp::manage::ManageEq::new(
                            dsp.clone(),
                            sheets.clone(),
                            Some(DEVICE.into()),
                            window,
                            cx,
                        )
                    })
                    .into(),
                ),
                Some("autoeq") => Some(
                    cx.new(|cx| {
                        koan_gui::dsp::flows::AutoEqSearch::new(
                            dsp.clone(),
                            sheets.clone(),
                            "HD 600".into(),
                            window,
                            cx,
                        )
                    })
                    .into(),
                ),
                Some("measure") => Some(
                    cx.new(|cx| {
                        koan_gui::dsp::flows::MeasurementFlow::new(
                            dsp.clone(),
                            sheets.clone(),
                            "HD 600".into(),
                            window,
                            cx,
                        )
                    })
                    .into(),
                ),
                _ => None,
            };
            if let Some(view) = view {
                sheets.update(cx, |s, cx| s.view(view, cx));
            }
            // gpui-component's focus and click extensions, for the shell's own handling.
            cx.new(|cx| koan_kit::raw::base::Root::new(page, window, cx))
        })?;
        cx.update_window(window.into(), |_, window, cx| {
            theme::pin_appearance(Some(dark), window, cx)
        })?;
        cx.allow_parking();
        settle(&mut cx, window.into())?;
        shots.push(cx.capture_screenshot(window.into())?);
    }
    let (w, h) = (shots[0].width(), shots[0].height());
    let mut sheet = image::RgbaImage::new(w * 2, h);
    for (index, shot) in shots.iter().enumerate() {
        image::imageops::replace(&mut sheet, shot, (index as u32 * w) as i64, 0);
    }
    sheet.save(&out)?;
    println!("{out}");
    Ok(())
}

fn drag(
    cx: &mut HeadlessAppContext,
    dsp: koan_kit::app::Entity<Dsp>,
) -> Result<(), Box<dyn Error>> {
    use koan_kit::raw::{
        Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PlatformInput, point,
    };
    let mut graph = None;
    let window = cx.open_window(size(px(WIDTH), px(HEIGHT)), |window, cx| {
        let page = cx.new(|cx| EqPage::new(dsp.clone(), Some(DEVICE.into()), window, cx));
        let sheets = page.read(cx).sheets.clone();
        let view = cx.new(|cx| {
            ProfilePage::new(
                dsp.clone(),
                sheets.clone(),
                "Warm".into(),
                Some(DEVICE.into()),
                window,
                cx,
            )
        });
        graph = Some(view.read(cx).graph());
        sheets.update(cx, |s, cx| s.view(view, cx));
        // gpui-component's focus and click extensions, for the shell's own handling.
        cx.new(|cx| koan_kit::raw::base::Root::new(page, window, cx))
    })?;
    let graph = graph.expect("the profile page has a graph");
    cx.allow_parking();
    settle(cx, window.into())?;
    cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))?;
    let start = cx
        .update(|cx| graph.read(cx).handle_position(2))
        .ok_or("Warm's third band has no handle")?;
    let mut times = Vec::new();
    cx.update_window(window.into(), |_, window, cx| {
        window.dispatch_event(
            PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Left,
                position: start,
                modifiers: Modifiers::default(),
                click_count: 1,
                first_mouse: false,
            }),
            cx,
        );
    })?;
    for step in 0..600 {
        let t = step as f32 / 600.0;
        let at = start
            + point(
                px(200.0 * (t * std::f32::consts::TAU).sin()),
                px(-60.0 * (t * 2.0 * std::f32::consts::TAU).sin()),
            );
        let began = std::time::Instant::now();
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| {
            window.dispatch_event(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position: at,
                    pressed_button: Some(MouseButton::Left),
                    modifiers: Modifiers::default(),
                }),
                cx,
            );
            window.draw(cx).clear(cx);
        })?;
        let took = began.elapsed();
        times.push(took);
        std::thread::sleep(Duration::from_millis(7));
    }
    cx.update_window(window.into(), |_, window, cx| {
        window.dispatch_event(
            PlatformInput::MouseUp(MouseUpEvent {
                button: MouseButton::Left,
                position: start,
                modifiers: Modifiers::default(),
                click_count: 1,
            }),
            cx,
        );
    })?;
    cx.run_until_parked();
    times.sort();
    let at = |q: f64| times[((times.len() - 1) as f64 * q) as usize].as_secs_f64() * 1000.0;
    println!(
        "drag draw over {} frames: p50 {:.2} ms, p95 {:.2} ms, p99 {:.2} ms, max {:.2} ms",
        times.len(),
        at(0.5),
        at(0.95),
        at(0.99),
        at(1.0)
    );
    Ok(())
}
