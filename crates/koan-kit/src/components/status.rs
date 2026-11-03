//! The foot of a navigation list: what the app is connected to, as a line
//! with its glyph, and figures about what it holds, under a faint rule. The
//! block decides its own inset, so it lines up with the rows above it.

use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::theme::{Icon, KoanIcon, KoanStyled, Look, ROW_RULE_OPACITY, Space, Tone, TypeRole};

/// What a line opens when clicked, and the tooltip saying so.
type LineAction = (SharedString, std::rc::Rc<dyn Fn(&mut Window, &mut App)>);

#[derive(IntoElement, Default)]
pub struct StatusBlock {
    lines: Vec<(Icon, SharedString, Tone, Option<LineAction>)>,
    figures: Vec<SharedString>,
}

impl StatusBlock {
    pub fn new() -> Self {
        Self::default()
    }

    /// A state, with its glyph: "connected to …", "sharing on port …".
    pub fn line(mut self, icon: Icon, text: impl Into<SharedString>, tone: Tone) -> Self {
        self.lines.push((icon, text.into(), tone, None));
        self
    }

    /// A state that leads somewhere: a click opens what `help` names ("server
    /// settings"), and the line reads `ink` under the pointer.
    pub fn link(
        mut self,
        icon: Icon,
        text: impl Into<SharedString>,
        tone: Tone,
        help: impl Into<SharedString>,
        open: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.lines.push((
            icon,
            text.into(),
            tone,
            Some((crate::theme::label(help.into()), std::rc::Rc::new(open))),
        ));
        self
    }

    /// A figure, in `meta`, `muted`: "5,123 tracks".
    pub fn figure(mut self, text: impl Into<SharedString>) -> Self {
        self.figures.push(text.into());
        self
    }
}

impl RenderOnce for StatusBlock {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let truncated = |d: Div| {
            d.min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
        };
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(look.spaced(6.0))
            .border_t(look.hairline)
            .border_color(look.palette.ink.hsla().opacity(ROW_RULE_OPACITY))
            .p(look.space(Space::L))
            .children(
                self.lines
                    .into_iter()
                    .enumerate()
                    .map(|(i, (icon, text, tone, action))| {
                        let color = look.text_color(tone);
                        let line = div()
                            .id(("status-line", i))
                            .flex()
                            .items_center()
                            .gap(look.space(Space::Xs))
                            .text_role(TypeRole::Meta, &look)
                            .text_color(color)
                            .child(KoanIcon::new(icon).role(TypeRole::Meta).color(color))
                            .child(truncated(div()).child(text));
                        match action {
                            Some((help, open)) => {
                                let ink = look.palette.ink.hsla();
                                line.cursor_pointer()
                                    .hover(move |s| s.text_color(ink))
                                    .tooltip(move |window, cx| {
                                        super::Tooltip::view(help.clone(), window, cx)
                                    })
                                    .on_click(move |_, window, cx| open(window, cx))
                                    .into_any_element()
                            }
                            None => line.into_any_element(),
                        }
                    }),
            )
            .when(!self.figures.is_empty(), |d| {
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(look.spaced(2.0))
                        .text_role(TypeRole::Meta, &look)
                        .text_color(look.palette.muted.hsla())
                        .children(self.figures.into_iter().map(|f| truncated(div()).child(f))),
                )
            })
    }
}
