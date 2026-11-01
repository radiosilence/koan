//! A page that scrolls, with the kōan scrollbar over its edge: shown while
//! it scrolls and while the pointer is over that edge, draggable, taking no
//! width from what scrolls. The bar's look and motion are the theme's
//! (`theme::sync_scrollbar`); a list keeps its own handle and lays a
//! `Scrollbar` over itself the same way.

use gpui_kit::base::Scrollbar;
use gpui_kit::*;

use super::child;

#[derive(IntoElement)]
pub struct ScrollArea {
    id: ElementId,
    content: Div,
}

impl ScrollArea {
    /// `content` is the column that scrolls, styled as the page needs.
    pub fn new(id: impl Into<ElementId>, content: Div) -> Self {
        Self {
            id: id.into(),
            content,
        }
    }
}

impl RenderOnce for ScrollArea {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let handle = window
            .use_keyed_state(child(&self.id, "scroll"), cx, |_, _| ScrollHandle::new())
            .read(cx)
            .clone();
        div()
            .relative()
            .size_full()
            .child(
                div()
                    .id(self.id)
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&handle)
                    .child(self.content),
            )
            .child(Scrollbar::vertical(&handle))
    }
}
