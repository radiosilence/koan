//! The toolbar's filter field, the sidebar's search field's look: no box,
//! the glyph and a lowercase prompt in `muted`, on the page's ground. The
//! caller owns the `InputState` and listens to it.

use gpui_kit::component::input::InputState;
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::TextField;
use crate::theme::{Icon, KoanIcon, Look, Tone, TypeRole};

#[derive(IntoElement)]
pub struct FilterField {
    input: Entity<InputState>,
}

impl FilterField {
    pub fn new(input: &Entity<InputState>) -> Self {
        Self {
            input: input.clone(),
        }
    }
}

impl RenderOnce for FilterField {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        div()
            .flex()
            .items_center()
            .gap(super::ToolbarMetrics::FIELD_GLYPH_GAP * look.scale.size)
            .w(super::ToolbarMetrics::FIELD * look.scale.size)
            .flex_none()
            .child(
                KoanIcon::new(Icon::Search)
                    .role(TypeRole::Control)
                    .tone(Tone::Muted),
            )
            // The rest of the row: an input is as wide as its parent, which a
            // flex row otherwise sizes to the input's content, nothing.
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(TextField::new(&self.input).bare()),
            )
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::{Modifiers, TestAppContext, VisualTestContext, point};

    use super::*;
    use crate::components::{Toolbar, ToolbarButton, ToolbarGroup};
    use core::prelude::v1::test;

    struct Bar {
        input: Entity<InputState>,
    }

    impl Render for Bar {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            // As the shell builds it: bare space in the middle, the page's
            // group trailing, the field leading the group.
            div().w(px(900.)).child(
                Toolbar::new()
                    .leading(ToolbarButton::new("back", Icon::Back, "back"))
                    .principal(div().id("drag").size_full())
                    .trailing(
                        ToolbarGroup::new()
                            .child(
                                div()
                                    .debug_selector(|| "filter".into())
                                    .child(FilterField::new(&self.input)),
                            )
                            .spacer()
                            .child(ToolbarButton::new("sort", Icon::Sort, "sort")),
                    ),
            )
        }
    }

    /// The field takes its width in a toolbar, and a click on it focuses it.
    #[gpui_kit::test]
    fn the_filter_field_in_a_toolbar_takes_its_width_and_a_click(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
        });
        let (bar, cx) = cx.add_window_view(|window, cx| Bar {
            input: cx.new(|cx| InputState::new(window, cx).placeholder("filter albums")),
        });
        let cx: &mut VisualTestContext = cx;
        cx.run_until_parked();
        let field = cx.debug_bounds("filter").expect("field drawn");
        assert!(field.size.width >= px(160.), "{field:?}");
        let input = bar.read_with(cx, |b, _| b.input.clone());
        let focused = |cx: &mut VisualTestContext| {
            cx.update(|window, cx| input.read(cx).focus_handle(cx).is_focused(window))
        };
        assert!(!focused(cx));
        let at = point(field.right() - px(40.), field.center().y);
        cx.simulate_click(at, Modifiers::default());
        cx.run_until_parked();
        assert!(
            focused(cx),
            "a click at {at:?} on {field:?} focused nothing"
        );
    }
}
