//! A level between two glyphs: a renderer's volume.

use std::sync::Arc;

use gpui_kit::{App, ElementId, IntoElement, ParentElement, RenderOnce, Styled, Window, div};

use super::{Handler, Slider, child};
use crate::theme::{Icon, KoanIcon, Look, Space, Tone};

/// A whole number from 0 to `max` on a `Slider`, `low` and `high` glyphs at
/// its ends in `muted`. While it is dragged it shows where the pointer is and
/// sends nothing; the value is committed once, when the drag is let go or an
/// arrow key steps it. For a value set over the network, where every step of
/// a drag would be a round trip and the value reported back lags the hand.
#[derive(IntoElement)]
pub struct Level {
    id: ElementId,
    value: u32,
    max: u32,
    low: Icon,
    high: Icon,
    numbered: bool,
    on_commit: Option<Handler<u32>>,
}

impl Level {
    pub fn new(id: impl Into<ElementId>, value: u32, max: u32) -> Self {
        Self {
            id: id.into(),
            value: value.min(max),
            max: max.max(1),
            low: Icon::VolumeDown,
            high: Icon::VolumeUp,
            numbered: false,
            on_commit: None,
        }
    }

    /// The glyphs at either end; a volume's speakers by default.
    pub fn ends(mut self, low: Icon, high: Icon) -> Self {
        self.low = low;
        self.high = high;
        self
    }

    /// The value written after the high glyph, such as "40".
    pub fn numbered(mut self) -> Self {
        self.numbered = true;
        self
    }

    pub fn on_commit(mut self, handler: impl Fn(&u32, &mut Window, &mut App) + 'static) -> Self {
        self.on_commit = Some(Arc::new(handler));
        self
    }
}

impl RenderOnce for Level {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        // Where a drag has the slider, until it is let go. Keyed state
        // redraws the view that draws it, and only that view.
        let held = window.use_keyed_state(child(&self.id, "held"), cx, |_, _| None::<u32>);
        let max = self.max;
        let shown = held.read(cx).unwrap_or(self.value);
        let (moving, committing) = (held.clone(), held);
        let fallback = self.value;
        let on_commit = self.on_commit;
        let slider = Slider::new(self.id.clone(), shown as f32 / max as f32)
            .named("volume")
            .step(5.0 / max as f32)
            .compact()
            .on_change(move |fraction, _, cx| {
                let to = (fraction * max as f32).round() as u32;
                moving.update(cx, |held, cx| {
                    if *held != Some(to) {
                        *held = Some(to);
                        cx.notify();
                    }
                });
            })
            .on_commit(move |_, window, cx| {
                let to = committing.update(cx, |held, cx| {
                    cx.notify();
                    held.take()
                });
                if let Some(commit) = &on_commit {
                    commit(&to.unwrap_or(fallback), window, cx);
                }
            });
        div()
            .flex()
            .items_center()
            .gap(look.space(Space::S))
            .child(KoanIcon::new(self.low).tone(Tone::Muted))
            .child(div().flex_1().child(slider))
            .child(KoanIcon::new(self.high).tone(Tone::Muted))
            .children(self.numbered.then(|| {
                div()
                    .flex_none()
                    .min_w(look.spaced(24.))
                    .text_color(look.palette.muted.hsla())
                    .child(shown.to_string())
            }))
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui_kit::{
        AnyWindowHandle, AppContext, Context, Modifiers, MouseButton, MouseDownEvent,
        MouseMoveEvent, MouseUpEvent, PlatformInput, Render, TestAppContext, point, px,
    };

    use super::*;

    struct Owner {
        value: u32,
        commits: Rc<RefCell<Vec<u32>>>,
    }

    impl Render for Owner {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let commits = self.commits.clone();
            div().w(px(400.)).h(px(60.)).flex().items_center().child(
                div().flex_1().child(
                    Level::new("level", self.value, 100)
                        .on_commit(move |to, _, _| commits.borrow_mut().push(*to)),
                ),
            )
        }
    }

    fn down(x: f32) -> PlatformInput {
        PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position: point(px(x), px(30.)),
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        })
    }

    fn held(x: f32) -> PlatformInput {
        PlatformInput::MouseMove(MouseMoveEvent {
            position: point(px(x), px(30.)),
            pressed_button: Some(MouseButton::Left),
            modifiers: Modifiers::default(),
        })
    }

    fn up(x: f32) -> PlatformInput {
        PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left,
            position: point(px(x), px(30.)),
            modifiers: Modifiers::default(),
            click_count: 1,
        })
    }

    /// A drag sends nothing until it is let go, then the value it reached,
    /// once; the slider shows the drag meanwhile, not the value it was given.
    #[gpui_kit::test]
    fn a_drag_commits_once_when_let_go(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
        });
        let commits = Rc::new(RefCell::new(Vec::new()));
        let kept = commits.clone();
        let handle = cx.add_window(move |_, _| Owner {
            value: 20,
            commits: kept,
        });
        let window: AnyWindowHandle = handle.into();
        let send = |cx: &mut TestAppContext, events: Vec<PlatformInput>| {
            cx.update_window(window, |_, window, cx| {
                window.draw(cx).clear(cx);
                for event in events {
                    window.dispatch_event(event, cx);
                    window.draw(cx).clear(cx);
                }
            })
            .unwrap();
        };
        send(
            cx,
            vec![
                PlatformInput::MouseMove(MouseMoveEvent {
                    position: point(px(100.), px(30.)),
                    pressed_button: None,
                    modifiers: Modifiers::default(),
                }),
                down(100.),
            ],
        );
        send(cx, (1..=10).map(|i| held(100. + i as f32 * 15.)).collect());
        assert!(commits.borrow().is_empty(), "a drag in progress committed");
        send(cx, vec![up(250.)]);
        let sent = commits.borrow().clone();
        assert_eq!(sent.len(), 1, "one commit per drag: {sent:?}");
        assert!(sent[0] > 20, "the drag's value was committed: {sent:?}");
    }
}
