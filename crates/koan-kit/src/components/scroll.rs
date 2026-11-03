//! A page that scrolls, with the kōan scrollbar over its edge: shown while
//! it scrolls and while the pointer is over that edge, draggable, taking no
//! width from what scrolls (`Scrollbar`); a list keeps its own handle and
//! lays a `Scrollbar` over itself the same way.

use super::Scrollbar;
use gpui::prelude::FluentBuilder;
use gpui::*;

use super::child;

#[derive(IntoElement)]
pub struct ScrollArea {
    id: ElementId,
    content: Content,
    reveal: Option<usize>,
    follow: Option<super::CursorSpot>,
    shrinks: bool,
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
            follow: None,
            shrinks: false,
        }
    }

    /// A column of items, one under another, any of which `reveal` can
    /// scroll into view: a list the arrow keys move through.
    pub fn items(id: impl Into<ElementId>, items: Vec<AnyElement>) -> Self {
        Self {
            id: id.into(),
            content: Content::Items(items),
            reveal: None,
            follow: None,
            shrinks: false,
        }
    }

    /// In a flex column whose height is only capped (a panel with a
    /// `max_h`): takes the room the column has left rather than its
    /// content's height, so it scrolls within the cap instead of being
    /// clipped by it.
    pub fn shrinks(mut self) -> Self {
        self.shrinks = true;
        self
    }

    /// Scrolls item `ix` into view when it changes, and leaves the scroll
    /// to the pointer otherwise.
    pub fn reveal(mut self, ix: Option<usize>) -> Self {
        self.reveal = ix;
        self
    }

    /// Brings a keyboard cursor into view each time the keys move it,
    /// wherever it is laid out inside the content (a pill in a flow, a tile
    /// in a row), the same item again included; the pointer's scrolling is
    /// left alone otherwise.
    pub fn follow(mut self, spot: &super::CursorSpot) -> Self {
        self.follow = Some(spot.clone());
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
        let followed = self.follow.is_some().then(|| {
            window
                .use_keyed_state(child(&self.id, "followed"), cx, |_, _| 0u64)
                .clone()
        });
        let scrolled = div()
            .id(self.id)
            .size_full()
            .when(self.shrinks, |d| d.flex_1().min_h_0())
            .overflow_y_scroll()
            .track_scroll(&handle);
        // After the content, so its cursor has been laid out this frame.
        let follow = self.follow.zip(followed).map(|(spot, followed)| {
            let handle = handle.clone();
            canvas(
                move |area, window, cx| {
                    let (moves, Some(at)) = (spot.moves(), spot.bounds()) else {
                        return;
                    };
                    if *followed.read(cx) == moves {
                        return;
                    }
                    followed.update(cx, |f, _| *f = moves);
                    let offset = handle.offset();
                    let by = if at.top() < area.top() {
                        area.top() - at.top()
                    } else if at.bottom() > area.bottom() {
                        area.bottom() - at.bottom()
                    } else {
                        return;
                    };
                    handle.set_offset(point(offset.x, offset.y + by));
                    window.refresh();
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full()
        });
        div()
            .relative()
            .size_full()
            .when(self.shrinks, |d| d.flex_1().min_h_0().flex().flex_col())
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
            .children(follow)
            .child(Scrollbar::vertical(&handle))
    }
}
