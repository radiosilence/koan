//! A page of different kinds of thing answering one question (what was
//! favourited, played lately, downloaded, or matches a search): the Mac's
//! `MixedCollection`. Sections stacked under the page's header, each with its
//! heading: pills wrapping left to right, record tiles in as many columns as
//! fit, rows the full width.
//!
//! Sections rather than a picker of kinds, for the reason the Mac gives:
//! they are all answers to one question, and a mode to remember is a worse
//! way to find a record.
//!
//! The collection owns the layout: the margins, the space between sections,
//! the pills' flow, the tiles' columns and gaps. The owner gives it the
//! header, each section's heading and its items, and reads the tile grid it
//! should build tiles for from the collection's `CollectionWidth`, which the
//! collection measures and keeps up to date. It scrolls as one column, for
//! the previews it is built for; a listing too long to lay out whole is the
//! browsers' job.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{EmptyState, PageHeader, Pill, RowMetrics, ScrollArea, SectionHeading, TileGrid};
use crate::theme::{Look, Space};

/// The width a collection was last laid out at, for the tile grid its owner
/// builds tiles for. Held by the owning view; the collection notifies that
/// view when the width changes.
#[derive(Clone, Default)]
pub struct CollectionWidth(Rc<Cell<Option<Pixels>>>);

impl CollectionWidth {
    /// The grid the collection lays tiles out on, once it has been measured.
    pub fn grid(&self, look: &Look) -> Option<TileGrid> {
        let width = self.0.get()?;
        Some(TileGrid::inside(
            width,
            RowMetrics::page_margin(look),
            &look.scale,
        ))
    }
}

enum Items {
    Pills(Vec<AnyElement>),
    Tiles(TileGrid, Vec<AnyElement>),
    Rows(Vec<AnyElement>),
}

/// One kind of thing in a collection, under its heading.
pub struct CollectionSection {
    heading: SectionHeading,
    items: Items,
}

impl CollectionSection {
    /// Things named as chips (`Pill`s), wrapping.
    pub fn pills(heading: SectionHeading, pills: impl IntoIterator<Item = Pill>) -> Self {
        Self {
            heading,
            items: Items::Pills(
                pills
                    .into_iter()
                    .map(IntoElement::into_any_element)
                    .collect(),
            ),
        }
    }

    /// Record tiles on `grid`, each `grid.side` wide and `grid.tile_height()`
    /// tall, in rows.
    pub fn tiles(
        heading: SectionHeading,
        grid: TileGrid,
        tiles: impl IntoIterator<Item = AnyElement>,
    ) -> Self {
        Self {
            heading,
            items: Items::Tiles(grid, tiles.into_iter().collect()),
        }
    }

    /// Table rows, the full width; each row insets itself by the page's
    /// margin, as `TableRow` does.
    pub fn rows(heading: SectionHeading, rows: impl IntoIterator<Item = AnyElement>) -> Self {
        Self {
            heading,
            items: Items::Rows(rows.into_iter().collect()),
        }
    }

    fn is_empty(&self) -> bool {
        match &self.items {
            Items::Pills(v) | Items::Tiles(_, v) | Items::Rows(v) => v.is_empty(),
        }
    }
}

#[derive(IntoElement)]
pub struct Collection {
    id: ElementId,
    width: CollectionWidth,
    header: Option<PageHeader>,
    sections: Vec<CollectionSection>,
    empty: Option<EmptyState>,
}

impl Collection {
    pub fn new(id: impl Into<ElementId>, width: &CollectionWidth) -> Self {
        Self {
            id: id.into(),
            width: width.clone(),
            header: None,
            sections: Vec::new(),
            empty: None,
        }
    }

    pub fn header(mut self, header: PageHeader) -> Self {
        self.header = Some(header);
        self
    }

    /// A section; one with nothing in it is left out, heading and all.
    pub fn section(mut self, section: CollectionSection) -> Self {
        if !section.is_empty() {
            self.sections.push(section);
        }
        self
    }

    /// What the page says when no section has anything in it.
    pub fn empty(mut self, empty: EmptyState) -> Self {
        self.empty = Some(empty);
        self
    }
}

impl RenderOnce for Collection {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let margin = RowMetrics::page_margin(&look);
        let cell = self.width.0.clone();
        let measure = canvas(
            move |bounds, window, cx| {
                if cell.get() != Some(bounds.size.width) {
                    cell.set(Some(bounds.size.width));
                    cx.notify(window.current_view());
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();

        let inset = |content: AnyElement| div().w_full().px(margin).child(content);
        let empty = self.sections.is_empty();
        let mut column = div()
            .w_full()
            .flex()
            .flex_col()
            .pt(RowMetrics::page_top(&look))
            .pb(look.space(Space::Xl))
            .children(
                self.header
                    .map(|h| inset(h.into_any_element()).pb(RowMetrics::header_gap(&look))),
            );
        for section in self.sections {
            column = column.child(inset(section.heading.into_any_element()));
            column = column.child(match section.items {
                Items::Pills(pills) => inset(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(Pill::gap(&look))
                        .children(pills)
                        .into_any_element(),
                ),
                Items::Tiles(grid, tiles) => {
                    let mut rows = div().flex().flex_col();
                    let mut tiles = tiles.into_iter().peekable();
                    while tiles.peek().is_some() {
                        rows = rows.child(
                            div()
                                .flex()
                                .gap(grid.gap)
                                .h(grid.row)
                                .children(tiles.by_ref().take(grid.columns)),
                        );
                    }
                    inset(rows.into_any_element())
                }
                Items::Rows(rows) => div().w_full().flex().flex_col().children(rows),
            });
        }

        div()
            .relative()
            .size_full()
            .child(measure)
            .map(|d| match (empty, self.empty) {
                (true, Some(state)) => d.child(
                    div()
                        .size_full()
                        .flex()
                        .flex_col()
                        .child(column)
                        .child(div().flex_1().flex().child(state)),
                ),
                _ => d.child(ScrollArea::new(self.id, column)),
            })
    }
}
