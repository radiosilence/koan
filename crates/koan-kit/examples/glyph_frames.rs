//! Draws each glyph answer (`components::GLYPH`) at points through its run,
//! offscreen, into one PNG: a row per answer, a column per moment, dark
//! ground. The evidence a change to the heart's pop, the play and pause fold,
//! shuffle, repeat and the skip's nudge is checked against. Nothing on the
//! screen is read; GPUI's headless renderer draws it.
//!
//! cargo run -p koan-kit --example glyph_frames -- target/glyph-frames.png

use std::error::Error;
use std::sync::Arc;

use gpui::prelude::*;
use gpui::*;
use koan_kit::components::{Animated, AnimatedQuad, GLYPH};
use koan_kit::theme;

/// The answers, as the program numbers them, and which way a nudge goes.
const ROWS: [(f32, f32, &str); 11] = [
    (10.0, 1.0, "heart fills"),
    (11.0, 1.0, "heart empties"),
    (12.0, 1.0, "play to pause"),
    (13.0, 1.0, "pause to play"),
    (14.0, 1.0, "shuffle on"),
    (15.0, 1.0, "shuffle off"),
    (16.0, 1.0, "repeat"),
    (17.0, 1.0, "repeat one"),
    (18.0, 1.0, "repeat one off"),
    (19.0, 1.0, "next"),
    (19.0, -1.0, "previous"),
];
/// The moments drawn, as a share of the answer's run.
const MOMENTS: [f32; 7] = [0.0, 0.15, 0.3, 0.45, 0.6, 0.75, 0.9];
/// A glyph's side and the room round it, in points.
const SIDE: f32 = 48.0;
const SPILL: f32 = 20.0;

struct Frames;

impl Render for Frames {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let scale = window.scale_factor();
        let cell = |code: f32, heading: f32, t: f32| {
            let animated = Animated::new();
            animated.set_all(0, &[code, heading]);
            animated.set_color(2, hsla(0.0, 0.0, 0.92, 1.0));
            animated.set_all(6, &[SPILL * scale]);
            animated.set_color(8, hsla(0.92, 0.75, 0.62, 1.0));
            animated.set_all(12, &[SIDE * scale]);
            // The answer's time, and the same on a quick-out curve.
            let eased = 1.0 - (1.0 - t).powi(3);
            animated.hold(0, t);
            animated.hold(1, eased);
            div()
                .relative()
                .size(px(SIDE + SPILL * 2.0))
                .child(AnimatedQuad::new(&GLYPH, &animated).absolute().inset_0())
        };
        div()
            .size_full()
            .bg(hsla(0.0, 0.0, 0.08, 1.0))
            .p(px(16.0))
            .flex()
            .flex_col()
            .children(ROWS.iter().map(|(code, heading, _)| {
                div()
                    .flex()
                    .children(MOMENTS.iter().map(|t| cell(*code, *heading, *t)))
            }))
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/glyph-frames.png".into());
    let text_system = gpui_platform::current_platform(true).text_system();
    let mut cx = HeadlessAppContext::with_platform(
        text_system,
        Arc::new(theme::icons::Assets),
        gpui_platform::current_headless_renderer,
    );
    cx.update(theme::init);
    let cell = SIDE + SPILL * 2.0;
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
    for (_, _, name) in ROWS {
        println!("  {name}");
    }
    Ok(())
}
