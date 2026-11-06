//! Draws each gesture (`components::GESTURE`) at points through its run,
//! offscreen, into one PNG: a row per gesture on an icon that uses it, a
//! column per moment, dark ground. The evidence a change to a glyph's press
//! motion is checked against. GPUI's headless renderer draws it.
//!
//! cargo run -p koan-kit --example gesture_frames -- target/gesture-frames.png

use std::error::Error;
use std::sync::Arc;

use gpui::prelude::*;
use gpui::*;
use koan_kit::components::{Animated, AnimatedQuad, GESTURE, Gesture, gesture_raster};
use koan_kit::theme::{self, Icon};

/// An icon for each gesture.
const ROWS: [Icon; 11] = [
    Icon::Queue,
    Icon::PlayNext,
    Icon::Share,
    Icon::Settings,
    Icon::Add,
    Icon::Remove,
    Icon::Rescan,
    Icon::Undo,
    Icon::Back,
    Icon::MoveDown,
    Icon::OpenExternal,
];
/// The moments drawn, as a share of the gesture's run.
const MOMENTS: [f32; 7] = [0.0, 0.15, 0.3, 0.45, 0.6, 0.75, 0.9];
/// The type the glyph sits beside, its side and the room round it, in
/// points.
const POINTS: f32 = 40.0;
const SPILL: f32 = 12.0;

struct Frames;

impl Render for Frames {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let scale = window.scale_factor();
        let side = theme::icons::side(POINTS);
        let mut cell = |icon: Icon, t: f32| {
            let gesture = Gesture::of(icon).expect("a gesture");
            let animated = Animated::new();
            let (code, heading) = (gesture.code(), gesture.heading());
            animated.set_all(0, &[code, heading.0, SPILL * scale, heading.1]);
            animated.set_color(4, hsla(0.0, 0.0, 0.92, 1.0));
            animated.set_color(8, hsla(0.92, 0.75, 0.62, 1.0));
            animated.set_all(12, &[side * scale]);
            let eased = 1.0 - (1.0 - t).powi(3);
            animated.hold(0, t);
            animated.hold(1, eased);
            let image = gesture_raster(icon, POINTS, side * scale, cx).expect("a raster");
            div().relative().size(px(side + SPILL * 2.0)).child(
                AnimatedQuad::new(&GESTURE, &animated)
                    .image(image)
                    .absolute()
                    .inset_0(),
            )
        };
        div()
            .size_full()
            .bg(hsla(0.0, 0.0, 0.08, 1.0))
            .p(px(16.0))
            .flex()
            .flex_col()
            .children(
                ROWS.iter()
                    .map(|icon| {
                        div()
                            .flex()
                            .children(MOMENTS.iter().map(|t| cell(*icon, *t)).collect::<Vec<_>>())
                    })
                    .collect::<Vec<_>>(),
            )
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/gesture-frames.png".into());
    let text_system = gpui_platform::current_platform(true).text_system();
    let mut cx = HeadlessAppContext::with_platform(
        text_system,
        Arc::new(theme::icons::Assets),
        gpui_platform::current_headless_renderer,
    );
    cx.update(theme::init);
    let cell = theme::icons::side(POINTS) + SPILL * 2.0;
    let width = MOMENTS.len() as f32 * cell + 32.0;
    let height = ROWS.len() as f32 * cell + 32.0;
    let window: AnyWindowHandle = cx
        .open_window(size(px(width), px(height)), |_, cx| cx.new(|_| Frames))?
        .into();
    cx.allow_parking();
    cx.run_until_parked();
    cx.update_window(window, |_, window, _| window.refresh())?;
    cx.run_until_parked();
    let shot = cx.capture_screenshot(window)?;
    shot.save(&out)?;
    println!("{out}");
    for icon in ROWS {
        println!("  {icon:?}: {:?}", Gesture::of(icon));
    }
    Ok(())
}
