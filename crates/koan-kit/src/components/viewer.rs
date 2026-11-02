//! A picture as large as the window allows, over everything: the Mac's
//! artwork viewer. The cover square, a caption under it (the title in
//! `title_small`, `strong`, and a line in `meta`, `muted`), on the window
//! dimmed. There is no close button on a picture: the owner closes it on a
//! click anywhere and on Escape, which it listens for on the viewer.

use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{Cover, CoverArt};
use crate::theme::{KoanStyled, Look, Space, TypeRole};

#[derive(IntoElement)]
pub struct Viewer {
    base: Stateful<Div>,
    art: CoverArt,
    title: SharedString,
    detail: Option<SharedString>,
}

impl Viewer {
    /// Room for the caption under the cover, and the margin around the lot.
    const CAPTION: f32 = 76.0;
    const MARGIN: f32 = 48.0;
    const LEAST: f32 = 320.0;

    pub fn new(id: impl Into<ElementId>, art: CoverArt, title: impl Into<SharedString>) -> Self {
        Self {
            base: div().id(id),
            art,
            title: title.into(),
            detail: None,
        }
    }

    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// The cover's side in a window of `size`: the largest square that
    /// leaves the margins and the caption, and never smaller than 320. What
    /// the owner decodes the picture at.
    pub fn side(size: Size<Pixels>, look: &Look) -> Pixels {
        Self::fit(size, look.scale.space)
    }

    /// `side` on a spacing scale of `space`.
    fn fit(size: Size<Pixels>, space: f32) -> Pixels {
        let margin = px(Self::MARGIN * space);
        let width = size.width - margin * 2.0;
        let height = size.height - margin * 2.0 - px(Self::CAPTION * space);
        width.min(height).max(px(Self::LEAST * space))
    }
}

impl InteractiveElement for Viewer {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl StatefulInteractiveElement for Viewer {}

impl RenderOnce for Viewer {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let p = look.palette;
        let side = Self::side(window.viewport_size(), &look);
        let line = |text: SharedString, role: TypeRole, color: Hsla| {
            div()
                .w_full()
                .text_center()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_role(role, &look)
                .text_color(color)
                .child(text)
        };
        self.base
            .absolute()
            .inset_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(look.space(Space::L))
            .bg(p.bg.hsla().opacity(0.85))
            .occlude()
            .cursor_pointer()
            .child(Cover::new(self.art, side))
            .child(
                div()
                    .w(side)
                    .flex()
                    .flex_col()
                    .gap(look.spaced(3.0))
                    .child(line(self.title, TypeRole::TitleSmall, p.strong.hsla()))
                    .children(self.detail.map(|d| line(d, TypeRole::Meta, p.muted.hsla()))),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    /// The largest square the margins and caption leave, never below the
    /// least: a wide window is bound by its height, a tiny one by the floor.
    #[test]
    fn the_cover_fills_what_the_window_leaves() {
        let wide = Viewer::fit(size(px(1600.), px(900.)), 1.0);
        assert_eq!(wide, px(900. - 96. - 76.));
        let tall = Viewer::fit(size(px(700.), px(1200.)), 1.0);
        assert_eq!(tall, px(700. - 96.));
        let tiny = Viewer::fit(size(px(200.), px(200.)), 1.0);
        assert_eq!(tiny, px(320.));
    }
}
