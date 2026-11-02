//! A long list of rows of differing heights, the queue's and a playlist's:
//! only the rows on screen are made, each measured as it is drawn, with the
//! kōan scrollbar over the list's edge. The state is the page's to keep, so
//! its place survives the page drawing again; rows not yet measured are
//! guessed at from their kind, which sizes the scrollbar from the first
//! frame.

use gpui_kit::base::Scrollbar;
use gpui_kit::*;

use super::{RowKind, RowMetrics};

/// A place in the list: the row at the top and how far into it.
pub use gpui_kit::ListOffset as RowOffset;

/// The list's place and measurements, kept by the page across frames.
#[derive(Clone)]
pub struct RowListState(ListState);

impl Default for RowListState {
    fn default() -> Self {
        Self::new()
    }
}

impl RowListState {
    pub fn new() -> Self {
        // How far past the screen rows are made ahead of a scroll.
        Self(ListState::new(0, ListAlignment::Top, px(400.)))
    }

    /// `count` rows, none measured, each guessed at as `kind`. The place is
    /// kept as far as the new rows reach.
    pub fn reset(&self, count: usize, kind: RowKind) {
        self.0
            .reset_with_uniform_height(count, RowMetrics::estimate(kind));
    }

    pub fn top(&self) -> RowOffset {
        self.0.logical_scroll_top()
    }

    pub fn scroll_to(&self, at: RowOffset) {
        self.0.scroll_to(at);
    }

    pub fn scroll_to_top(&self) {
        self.0.scroll_to(RowOffset::default());
    }

    /// Scrolls as little as shows row `ix` whole.
    pub fn reveal(&self, ix: usize) {
        self.0.scroll_to_reveal_item(ix);
    }

    /// Called as the list scrolls, with the first row on screen. The list's
    /// own state is held while it runs, so it reads the page, not the list.
    pub fn on_scroll(&self, f: impl Fn(usize, &mut Window, &mut App) + 'static) {
        self.0
            .set_scroll_handler(move |event: &ListScrollEvent, window, cx| {
                f(event.visible_range.start, window, cx)
            });
    }
}

#[derive(IntoElement)]
pub struct RowList {
    state: RowListState,
    row: Box<dyn FnMut(usize, &mut Window, &mut App) -> AnyElement>,
}

impl RowList {
    /// `row` makes row `ix` as it comes on screen.
    pub fn new(
        state: &RowListState,
        row: impl FnMut(usize, &mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        Self {
            state: state.clone(),
            row: Box::new(row),
        }
    }
}

impl RenderOnce for RowList {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .relative()
            .size_full()
            .child(list(self.state.0.clone(), self.row).size_full())
            .child(Scrollbar::vertical(&self.state.0))
    }
}
