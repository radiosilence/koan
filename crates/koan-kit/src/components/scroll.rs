//! A page that scrolls, with the kōan scrollbar over its edge: shown while
//! it scrolls and while the pointer is over that edge, draggable, taking no
//! width from what scrolls (`Scrollbar`); a list keeps its own handle and
//! lays a `Scrollbar` over itself the same way.

use super::Scrollbar;
use gpui::prelude::FluentBuilder;
use gpui::*;

use super::child;

thread_local! {
    /// The field and keyboard height an area has scrolled for, so an area
    /// around it leaves them be.
    static TAKEN: std::cell::RefCell<Option<(FocusHandle, u32)>> =
        const { std::cell::RefCell::new(None) };
}

#[derive(IntoElement)]
pub struct ScrollArea {
    id: ElementId,
    content: Content,
    reveal: Option<usize>,
    follow: Option<super::CursorSpot>,
    shrinks: bool,
    under_bar: bool,
    under_foot: bool,
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
            under_bar: false,
            under_foot: false,
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
            under_bar: false,
            under_foot: false,
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

    /// A page that scrolls under a phone's bar and its foot: its content
    /// starts the bar's height down (`bar_inset`) and ends the foot's height
    /// up from the end of the scroll (`foot_inset`), so at either end
    /// nothing is under them, and passes under both as it scrolls. Not for a
    /// sheet, a menu or a panel, which neither is ever over.
    pub fn under_bar(mut self) -> Self {
        self.under_bar = true;
        self.under_foot = true;
        self
    }

    /// The body of a page whose header holds it clear of the bar: it passes
    /// under the phone's foot alone (`foot_inset`).
    pub fn under_foot(mut self) -> Self {
        self.under_foot = true;
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
        let typing = window
            .use_keyed_state(child(&self.id, "typing"), cx, |_, _| None)
            .clone();
        let under_bar = if self.under_bar {
            super::bar_inset(cx)
        } else {
            Pixels::ZERO
        };
        let under_foot = if self.under_foot {
            super::foot_inset(cx)
        } else {
            Pixels::ZERO
        };
        // The room for the bar and the foot is inside what scrolls, so the
        // content passes under them rather than being clipped at their edges.
        let spaced = under_bar > Pixels::ZERO || under_foot > Pixels::ZERO;
        let spacer = |height: Pixels| (height > Pixels::ZERO).then(|| div().flex_none().h(height));
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
                    // Clear of the bar and the foot the content scrolls under.
                    let top = area.top() + under_bar;
                    let bottom = area.bottom() - under_foot;
                    let by = if at.top() < top {
                        top - at.top()
                    } else if at.bottom() > bottom {
                        bottom - at.bottom()
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
        let scrolling = handle.clone();
        let clear_of_keyboard = canvas(
            move |area, window, cx| {
                let keyboard = window.insets().ime.bottom;
                // The keyboard gone: the next time it comes up is a new one.
                if keyboard <= Pixels::ZERO {
                    if typing.read(cx).is_some() {
                        typing.update(cx, |t, _| *t = None);
                    }
                    TAKEN.with_borrow_mut(|taken| *taken = None);
                    return;
                }
                let Some(super::field::Typing {
                    focus,
                    bounds: field,
                    clip,
                }) = super::field::typing_in()
                else {
                    return;
                };
                // Once for each field and each height the keyboard takes,
                // so a scroll by hand afterwards is left alone; and only by
                // the area the field is drawn in, whose clip holds it, not
                // a page's under a sheet. Of nested areas the inner one,
                // whose canvas comes first, takes it.
                let asked = Some((focus.clone(), f32::from(keyboard).to_bits()));
                let inside = |b: Bounds<Pixels>| {
                    b.left() >= area.left() - px(1.)
                        && b.right() <= area.right() + px(1.)
                        && b.top() >= area.top() - px(1.)
                        && b.bottom() <= area.bottom() + px(1.)
                };
                if !focus.is_focused(window)
                    || *typing.read(cx) == asked
                    || !inside(clip)
                    || field.top() < area.top()
                    || TAKEN.with_borrow(|taken| *taken == asked)
                {
                    return;
                }
                typing.update(cx, |t, _| *t = asked.clone());
                TAKEN.with_borrow_mut(|taken| *taken = asked);
                let margin = px(16.);
                let clear = (window.viewport_size().height - keyboard).min(area.bottom()) - margin;
                if field.bottom() <= clear {
                    return;
                }
                let by = (field.bottom() - clear).min(field.top() - area.top());
                let offset = scrolling.offset();
                scrolling.set_offset(point(offset.x, offset.y - by));
                window.refresh();
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        div()
            .relative()
            .size_full()
            .when(self.shrinks, |d| d.flex_1().min_h_0().flex().flex_col())
            .child(match self.content {
                Content::Column(column) if !spaced => scrolled.child(*column),
                Content::Column(column) => scrolled
                    .flex()
                    .flex_col()
                    .children(spacer(under_bar))
                    .child(column.flex_none())
                    .children(spacer(under_foot)),
                // Each item at its own height: a column taller than the
                // view scrolls, and no item (an empty spacer least of all)
                // is shrunk to fit.
                Content::Items(items) => scrolled
                    .flex()
                    .flex_col()
                    .children(spacer(under_bar))
                    .children(items.into_iter().map(|item| div().flex_none().child(item)))
                    .children(spacer(under_foot)),
            })
            .children(follow)
            .child(clear_of_keyboard)
            .child(
                Scrollbar::vertical(&handle)
                    .when(self.under_bar, Scrollbar::under_bar)
                    .when(self.under_foot, Scrollbar::under_foot),
            )
    }
}
