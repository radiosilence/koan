//! Controls side by side on one rhythm, each a touch target wide.

use gpui::{
    AnyElement, App, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    StatefulInteractiveElement, Styled, Window, div,
};

use crate::theme::HIT;

/// Controls side by side, each in a slot exactly a touch target wide (`HIT`)
/// with none between, so the targets touch and the glyphs, whatever their
/// widths, sit an even step apart with no gap of their own. A slot never
/// grows with its control, so the group keeps one width whatever the
/// controls show and what sits beside it keeps its room. A press in a
/// slot is the control's: it does not reach what the group sits in, such
/// as a bar that opens on a tap. It never shrinks.
#[derive(IntoElement, Default)]
pub struct ControlGroup {
    controls: Vec<AnyElement>,
}

impl ControlGroup {
    /// An empty group.
    pub fn new() -> Self {
        Self::default()
    }

    /// A control after those already given.
    pub fn control(mut self, control: impl IntoElement) -> Self {
        self.controls.push(control.into_any_element());
        self
    }
}

impl RenderOnce for ControlGroup {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div().flex().items_center().flex_shrink_0().children(
            self.controls.into_iter().enumerate().map(|(i, control)| {
                div()
                    .id(i)
                    .w(HIT)
                    .flex_none()
                    .h(HIT)
                    .flex()
                    .items_center()
                    .justify_center()
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(control)
            }),
        )
    }
}
