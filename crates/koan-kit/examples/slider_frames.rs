//! Times a slider drag, frame by frame, offscreen: a slider over the
//! storybook, dragged across by synthetic mouse moves, each followed by one
//! draw. Prints the draw times and how often the storybook beside the slider
//! rendered, which should be never: a drag is one slider's business.
//!
//! cargo run --release -p koan-kit --example slider_frames

use std::cell::Cell;
use std::error::Error;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use koan_kit::app::*;
use koan_kit::components::Slider;
use koan_kit::storybook::Storybook;
use koan_kit::theme;
// A measuring tool outside the app: GPUI's headless harness and input, driven directly.
use koan_kit::raw::{HeadlessAppContext, PlatformInput, StyleRefinement, point, px, size};

const WIDTH: f32 = 1180.0;
const HEIGHT: f32 = 1500.0;
const BAR: f32 = 60.0;
const MOVES: usize = 240;

/// The slider, its own view, as the transport's seek bar is.
struct Bar {
    value: f32,
}

impl Render for Bar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        div()
            .w_full()
            .h(px(BAR))
            .px(px(20.))
            .flex()
            .items_center()
            .child(
                div()
                    .flex_1()
                    .child(
                        Slider::new("bench", self.value).on_change(move |value, _, cx| {
                            let value = *value;
                            entity.update(cx, |bar, cx| {
                                bar.value = value;
                                cx.notify();
                            })
                        }),
                    ),
            )
    }
}

/// The sheet, counting its renders.
struct Counted {
    sheet: Entity<Storybook>,
    renders: Rc<Cell<usize>>,
}

impl Render for Counted {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        div().size_full().child(self.sheet.clone())
    }
}

struct Bench {
    bar: Entity<Bar>,
    sheet: Entity<Counted>,
}

impl Render for Bench {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                self.bar
                    .clone()
                    .cached(StyleRefinement::default().w_full().h(px(BAR))),
            )
            .child(
                div().flex_1().child(
                    self.sheet
                        .clone()
                        .cached(StyleRefinement::default().size_full()),
                ),
            )
    }
}

fn main() -> Result<(), Box<dyn Error>> {
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
        theme::init(cx);
    });
    let renders = Rc::new(Cell::new(0));
    let counted = renders.clone();
    let bar_handle: Rc<Cell<Option<Entity<Bar>>>> = Rc::new(Cell::new(None));
    let held = bar_handle.clone();
    let window = cx.open_window(size(px(WIDTH), px(HEIGHT)), move |window, cx| {
        let sheet = cx.new(|cx| Storybook::new(window, cx));
        let sheet = cx.new(|_| Counted {
            sheet,
            renders: counted,
        });
        let bar = cx.new(|_| Bar { value: 0.1 });
        held.set(Some(bar.clone()));
        cx.new(|_| Bench { bar, sheet })
    })?;
    cx.allow_parking();
    cx.run_until_parked();
    std::thread::sleep(Duration::from_millis(2500));
    cx.run_until_parked();
    let window: AnyWindowHandle = window.into();

    let y = px(BAR / 2.0);
    let left = 40.0;
    let right = WIDTH - 60.0;
    let mut times = Vec::with_capacity(MOVES);
    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear(cx);
        window.dispatch_event(
            PlatformInput::MouseMove(MouseMoveEvent {
                position: point(px(left), y),
                pressed_button: None,
                modifiers: Modifiers::default(),
            }),
            cx,
        );
        window.draw(cx).clear(cx);
        window.dispatch_event(
            PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Left,
                position: point(px(left), y),
                modifiers: Modifiers::default(),
                click_count: 1,
                first_mouse: false,
            }),
            cx,
        );
        window.draw(cx).clear(cx);
    })?;
    renders.set(0);
    for step in 0..MOVES {
        let x = left + (right - left) * (step as f32 + 1.0) / MOVES as f32;
        let elapsed = cx.update_window(window, |_, window, cx| {
            window.dispatch_event(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position: point(px(x), y),
                    pressed_button: Some(MouseButton::Left),
                    modifiers: Modifiers::default(),
                }),
                cx,
            );
            let start = Instant::now();
            window.draw(cx).clear(cx);
            start.elapsed()
        })?;
        times.push(elapsed);
    }
    cx.update_window(window, |_, window, cx| {
        window.dispatch_event(
            PlatformInput::MouseUp(MouseUpEvent {
                button: MouseButton::Left,
                position: point(px(right), y),
                modifiers: Modifiers::default(),
                click_count: 1,
            }),
            cx,
        );
    })?;

    let bar = bar_handle.take().expect("the bar was made");
    let reached = cx.update(|cx| bar.read(cx).value);
    println!("the slider ended at {reached:.2} (started at 0.10)");
    times.sort();
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let mean = times.iter().map(|d| ms(*d)).sum::<f64>() / times.len() as f64;
    println!(
        "{MOVES} moves: draw mean {mean:.2} ms, p50 {:.2} ms, p95 {:.2} ms, max {:.2} ms; the sheet beside it rendered {} times",
        ms(times[times.len() / 2]),
        ms(times[times.len() * 95 / 100]),
        ms(*times.last().unwrap()),
        renders.get(),
    );
    Ok(())
}
