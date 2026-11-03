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
    content: Content,
    reveal: Option<usize>,
}

enum Content {
    Column(Box<Div>),
    Items(Vec<AnyElement>),
}

impl ScrollArea {
    /// `content` is the column that scrolls, styled as the page needs.
    pub fn new(id: impl Into<ElementId>, content: Div) -> Self {
        Self {
            id: id.into(),
            content: Content::Column(Box::new(content)),
            reveal: None,
        }
    }

    /// A column of items, one under another, any of which `reveal` can
    /// scroll into view: a list the arrow keys move through.
    pub fn items(id: impl Into<ElementId>, items: Vec<AnyElement>) -> Self {
        Self {
            id: id.into(),
            content: Content::Items(items),
            reveal: None,
        }
    }

    /// Scrolls item `ix` into view when it changes, and leaves the scroll
    /// to the pointer otherwise.
    pub fn reveal(mut self, ix: Option<usize>) -> Self {
        self.reveal = ix;
        self
    }
}

impl RenderOnce for ScrollArea {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let handle = window
            .use_keyed_state(child(&self.id, "scroll"), cx, |_, _| ScrollHandle::new())
            .read(cx)
            .clone();
        let revealed = window.use_keyed_state(child(&self.id, "revealed"), cx, |_, _| None);
        if *revealed.read(cx) != self.reveal {
            revealed.update(cx, |r, _| *r = self.reveal);
            if let Some(ix) = self.reveal {
                handle.scroll_to_item(ix);
            }
        }
        let scrolled = div()
            .id(self.id)
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&handle);
        div()
            .relative()
            .size_full()
            .child(match self.content {
                Content::Column(column) => scrolled.child(*column),
                // Each item at its own height: a column taller than the
                // view scrolls, and no item (an empty spacer least of all)
                // is shrunk to fit.
                Content::Items(items) => scrolled
                    .flex()
                    .flex_col()
                    .children(items.into_iter().map(|item| div().flex_none().child(item))),
            })
            .child(Scrollbar::vertical(&handle))
    }
}
