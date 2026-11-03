//! Named values read off something, dense, under a section header: the Mac's
//! track info, where each of a track's sources lists its own fields. The
//! name in a column of its own, in `muted`; the value beside it, in `ink`,
//! wrapping. Both in `fine`.

use gpui::prelude::*;
use gpui::*;

use super::SectionHeader;
use crate::theme::{KoanStyled, Look, Space, TypeRole};

#[derive(IntoElement)]
pub struct DetailGroup {
    title: SharedString,
    rows: Vec<(SharedString, SharedString)>,
}

impl DetailGroup {
    /// The width of the names' column, at 100 %.
    const NAME: f32 = 128.0;

    /// A group whose title is data (a source's kind), which keeps its case.
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            rows: Vec::new(),
        }
    }

    pub fn row(mut self, name: impl Into<SharedString>, value: impl Into<SharedString>) -> Self {
        self.rows.push((name.into(), value.into()));
        self
    }

    pub fn rows(
        mut self,
        rows: impl IntoIterator<Item = (impl Into<SharedString>, impl Into<SharedString>)>,
    ) -> Self {
        self.rows
            .extend(rows.into_iter().map(|(n, v)| (n.into(), v.into())));
        self
    }
}

impl RenderOnce for DetailGroup {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let p = look.palette;
        div()
            .flex()
            .flex_col()
            .gap(look.spaced(3.0))
            .child(SectionHeader::data(self.title))
            .children(self.rows.into_iter().map(|(name, value)| {
                div()
                    .flex()
                    .items_start()
                    .gap(look.space(Space::S))
                    .text_role(TypeRole::Fine, &look)
                    .child(
                        div()
                            .flex_none()
                            .w(look.spaced(Self::NAME))
                            .text_color(p.muted.hsla())
                            .child(name),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(p.ink.hsla())
                            .child(value),
                    )
            }))
    }
}
