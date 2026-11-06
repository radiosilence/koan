//! A long list of rows of differing heights, the queue's and a playlist's:
//! only the rows on screen are made, each measured as it is drawn, with the
//! kōan scrollbar over the list's edge. The state is the page's to keep, so
//! its place survives the page drawing again; rows not yet measured are
//! guessed at from their kind, which sizes the scrollbar from the first
//! frame.

use super::Scrollbar;
use gpui::prelude::FluentBuilder;
use gpui::*;

use super::{RowKind, RowMetrics};

/// A place in the list: the row at the top and how far into it.
pub use gpui::ListOffset as RowOffset;

/// The list's place and measurements, kept by the page across frames.
///
/// The list may lead with one item before its rows, a page's header that
/// scrolls with them (`RowList::lead`); row indices count the rows alone.
#[derive(Clone)]
pub struct RowListState(ListState, std::rc::Rc<std::cell::Cell<usize>>);

impl Default for RowListState {
    fn default() -> Self {
        Self::new()
    }
}

impl RowListState {
    pub fn new() -> Self {
        // How far past the screen rows are made ahead of a scroll.
        Self(
            ListState::new(0, ListAlignment::Top, px(400.)),
            Default::default(),
        )
    }

    /// How many items lead the rows: one with a header, none without.
    fn lead(&self) -> usize {
        self.1.get()
    }

    /// Whether the list leads with a header. Rows are re-measured.
    pub fn set_lead(&self, lead: bool, count: usize, kind: RowKind) {
        if self.lead() != lead as usize {
            self.1.set(lead as usize);
            self.reset(count, kind);
        }
    }

    /// `count` rows, none measured, each guessed at as `kind`. The place is
    /// kept as far as the new rows reach.
    pub fn reset(&self, count: usize, kind: RowKind) {
        self.0
            .reset_with_uniform_height(count + self.lead(), RowMetrics::estimate(kind));
    }

    /// The row at the top; within the leading header, the first row.
    pub fn top(&self) -> RowOffset {
        let top = self.0.logical_scroll_top();
        match top.item_ix.checked_sub(self.lead()) {
            Some(item_ix) => RowOffset { item_ix, ..top },
            None => RowOffset::default(),
        }
    }

    pub fn scroll_to(&self, at: RowOffset) {
        if at == RowOffset::default() {
            return self.scroll_to_top();
        }
        self.0.scroll_to(RowOffset {
            item_ix: at.item_ix + self.lead(),
            ..at
        });
    }

    pub fn scroll_to_top(&self) {
        self.0.scroll_to(RowOffset::default());
    }

    /// Scrolls as little as shows row `ix` whole.
    pub fn reveal(&self, ix: usize) {
        self.0.scroll_to_reveal_item(ix + self.lead());
    }

    /// Scrolls the neighbour of row `ix` a screen reader is heading for on
    /// screen, since only rows drawn are in the accessibility tree
    /// (`reveal_around_list`).
    pub fn reveal_around(&self, ix: usize) {
        super::lists::reveal_around_list(&self.0, ix + self.lead());
    }

    /// Where row `ix` was laid out last frame, in the window: for a row
    /// past the viewport's edge, outside it. `None` for a row the list has
    /// not laid out.
    pub fn row_bounds(&self, ix: usize) -> Option<Bounds<Pixels>> {
        self.0.bounds_for_item(ix + self.lead())
    }

    /// Called as the list scrolls, with the first row on screen. The list's
    /// own state is held while it runs, so it reads the page, not the list.
    pub fn on_scroll(&self, f: impl Fn(usize, &mut Window, &mut App) + 'static) {
        let lead = self.1.clone();
        self.0
            .set_scroll_handler(move |event: &ListScrollEvent, window, cx| {
                f(
                    event.visible_range.start.saturating_sub(lead.get()),
                    window,
                    cx,
                )
            });
    }
}

/// Makes row `ix` as it comes on screen.
type MakeRow = Box<dyn FnMut(usize, &mut Window, &mut App) -> AnyElement>;

#[derive(IntoElement)]
pub struct RowList {
    state: RowListState,
    row: MakeRow,
    lead: Option<MakeLead>,
    name: Option<SharedString>,
    under_bar: bool,
}

/// Makes the item leading the rows.
type MakeLead = Box<dyn FnMut(&mut Window, &mut App) -> AnyElement>;

impl RowList {
    /// `row` makes row `ix` as it comes on screen.
    pub fn new(
        state: &RowListState,
        row: impl FnMut(usize, &mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        Self {
            state: state.clone(),
            row: Box::new(row),
            lead: None,
            name: None,
            under_bar: false,
        }
    }

    /// An item before the rows that scrolls with them: a page's header, on a
    /// phone, where a header held above the list leaves it too little of the
    /// screen. The state must have been told (`RowListState::set_lead`).
    pub fn lead(mut self, lead: impl FnMut(&mut Window, &mut App) -> AnyElement + 'static) -> Self {
        self.lead = Some(Box::new(lead));
        self
    }

    /// A page's list that scrolls under a phone's bar: its leading item
    /// starts the bar's height down (`bar_inset`) and rises under the bar as
    /// it scrolls. The state must lead (`RowListState::set_lead`).
    pub fn under_bar(mut self) -> Self {
        self.under_bar = true;
        self
    }

    /// What the list is of, for a screen reader: "queue", a playlist's name.
    pub fn named(mut self, name: impl Into<SharedString>) -> Self {
        self.name = Some(name.into());
        self
    }
}

impl RenderOnce for RowList {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let under_bar = if self.under_bar {
            super::bar_inset(cx)
        } else {
            Pixels::ZERO
        };
        super::accessible(div().id("row-list"), gpui::Role::List, self.name)
            .relative()
            .size_full()
            .child({
                let (mut row, mut lead) = (self.row, self.lead);
                let leads = self.state.lead();
                list(self.state.0.clone(), move |ix, window, cx| {
                    match (ix.checked_sub(leads), lead.as_mut()) {
                        (None, Some(lead)) => div()
                            .pt(under_bar)
                            .child(lead(window, cx))
                            .into_any_element(),
                        (None, None) => div().pt(under_bar).into_any_element(),
                        (Some(ix), _) => row(ix, window, cx),
                    }
                })
                .size_full()
            })
            .child(Scrollbar::vertical(&self.state.0).when(self.under_bar, Scrollbar::under_bar))
    }
}
