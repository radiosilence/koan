//! A line across the top of the page column, for something that holds for
//! every page until it is put right: the server this device cannot use. The
//! Mac's `ServerBanner`.

use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{Button, ButtonKind, Handler};
use crate::theme::{
    HAIRLINE, Icon, KoanIcon, KoanStyled, Look, ROW_RULE_OPACITY, Space, Tone, TypeRole,
};

/// The glyph and message in `tone`, `fine`, on the page's ground with a rule
/// beneath; an action at the far edge where there is something to do about
/// it. Two lines at most.
#[derive(IntoElement)]
pub struct Banner {
    icon: Icon,
    message: SharedString,
    tone: Tone,
    action: Option<(SharedString, Handler<ClickEvent>)>,
}

impl Banner {
    pub fn new(icon: Icon, message: impl Into<SharedString>, tone: Tone) -> Self {
        Self {
            icon,
            message: message.into(),
            tone,
            action: None,
        }
    }

    /// What to do about it: a compact button, in the app's words.
    pub fn action(
        mut self,
        label: impl Into<SharedString>,
        run: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.action = Some((label.into(), std::sync::Arc::new(run)));
        self
    }
}

impl RenderOnce for Banner {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let color = look.text_color(self.tone);
        div()
            .flex()
            .items_center()
            .gap(look.space(Space::M))
            .px(look.space(Space::L))
            .py(look.space(Space::S))
            .border_b(HAIRLINE)
            .border_color(look.palette.rule.hsla().opacity(ROW_RULE_OPACITY))
            .text_role(TypeRole::Fine, &look)
            .text_color(color)
            .child(KoanIcon::new(self.icon).role(TypeRole::Fine).color(color))
            .child(
                // The message alone is announced: the action beside it
                // stays a button.
                super::announced(div().id("banner"), self.message.clone(), self.tone)
                    .flex_1()
                    .min_w_0()
                    .line_clamp(2)
                    .child(self.message),
            )
            .children(self.action.map(|(label, run)| {
                Button::new("banner-action", ButtonKind::Compact)
                    .label(label)
                    .on_click(move |e, window, cx| run(e, window, cx))
            }))
    }
}
