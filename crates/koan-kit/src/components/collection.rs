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
    reveal: Option<usize>,
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
            reveal: None,
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
            reveal: None,
        }
    }

    /// Table rows, the full width; each row insets itself by the page's
    /// margin, as `TableRow` does.
    pub fn rows(heading: SectionHeading, rows: impl IntoIterator<Item = AnyElement>) -> Self {
        Self {
            heading,
            items: Items::Rows(rows.into_iter().collect()),
            reveal: None,
        }
    }

    /// Scrolls item `ix` of this section into view when it changes: the
    /// one under the page's keyboard cursor.
    pub fn reveal(mut self, ix: Option<usize>) -> Self {
        self.reveal = ix;
        self
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
            move |bounds, window, _| {
                // A notify from inside the frame that measured is cleared with
                // that frame; the next one is asked for instead, which draws
                // the tiles at the width just found.
                if cell.get() != Some(bounds.size.width) {
                    cell.set(Some(bounds.size.width));
                    window.request_animation_frame();
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();

        let inset = |content: AnyElement| div().w_full().px(margin).child(content);
        let empty = self.sections.is_empty();
        // One line per heading, pill flow, row of tiles and table row, so
        // any of them can be scrolled into view.
        let mut lines: Vec<AnyElement> =
            vec![div().h(RowMetrics::page_top(&look)).into_any_element()];
        lines.extend(self.header.map(|h| {
            inset(h.into_any_element())
                .pb(RowMetrics::header_gap(&look))
                .into_any_element()
        }));
        let mut reveal = None;
        for section in self.sections {
            lines.push(inset(section.heading.into_any_element()).into_any_element());
            let first = lines.len();
            match section.items {
                Items::Pills(pills) => {
                    reveal = reveal.or(section.reveal.map(|_| first));
                    lines.push(
                        inset(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap(Pill::gap(&look))
                                .children(pills)
                                .into_any_element(),
                        )
                        .into_any_element(),
                    );
                }
                Items::Tiles(grid, tiles) => {
                    let columns = grid.columns.max(1);
                    reveal = reveal.or(section.reveal.map(|ix| first + ix / columns));
                    let mut tiles = tiles.into_iter().peekable();
                    while tiles.peek().is_some() {
                        // On the grid's own inset: the page's margin and a
                        // share of the spare width, so the tiles keep their
                        // size as the window resizes.
                        lines.push(
                            div()
                                .w_full()
                                .px(grid.inset)
                                .flex()
                                .gap(grid.gap)
                                .h(grid.row)
                                .children(tiles.by_ref().take(columns))
                                .into_any_element(),
                        );
                    }
                }
                Items::Rows(rows) => {
                    reveal = reveal.or(section.reveal.map(|ix| first + ix));
                    lines.extend(
                        rows.into_iter()
                            .map(|row| div().w_full().child(row).into_any_element()),
                    );
                }
            }
        }
        lines.push(div().h(look.space(Space::Xl)).into_any_element());

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
                        .child(div().w_full().flex().flex_col().children(lines))
                        .child(div().flex_1().flex().child(state)),
                ),
                _ => d.child(ScrollArea::items(self.id, lines).reveal(reveal)),
            })
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::{TestAppContext, VisualTestContext};

    use super::*;
    use core::prelude::v1::test;

    struct Page {
        width: CollectionWidth,
    }

    impl Render for Page {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let look = Look::of(window, cx);
            let mut collection = Collection::new("c", &self.width)
                .header(PageHeader::new("shelf"))
                .section(CollectionSection::rows(
                    SectionHeading::new("nothing"),
                    Vec::new(),
                ))
                .section(CollectionSection::pills(
                    SectionHeading::new("artists"),
                    (0..3usize).map(|i| Pill::new(("pill", i), crate::theme::Icon::Artist, "a")),
                ));
            if let Some(grid) = self.width.grid(&look) {
                let tiles = (0..grid.columns + 1).map(|i| {
                    div()
                        .debug_selector(move || format!("tile-{i}"))
                        .w(grid.side)
                        .h(grid.tile_height())
                        .into_any_element()
                });
                collection = collection.section(CollectionSection::tiles(
                    SectionHeading::new("albums"),
                    grid,
                    tiles,
                ));
            }
            div().w(px(700.)).h(px(900.)).child(collection)
        }
    }

    /// The collection measures itself, so the page's tiles come on the
    /// next frame in as many columns as fit, one more than fits wrapping to
    /// a row of its own.
    #[gpui_kit::test]
    fn tiles_fill_the_measured_columns_and_wrap(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
        });
        let (page, cx) = cx.add_window_view(|_, _| Page {
            width: CollectionWidth::default(),
        });
        let cx: &mut VisualTestContext = cx;
        cx.run_until_parked();
        let columns = cx.update(|window, cx| {
            let look = Look::of(window, cx);
            page.read(cx).width.grid(&look).map(|g| g.columns)
        });
        let columns = columns.expect("measured after the first frame");
        assert!(columns > 1);
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        let first = cx.debug_bounds("tile-0").expect("tiles drawn");
        let second = cx.debug_bounds("tile-1").unwrap();
        let wrapped = cx
            .debug_bounds(Box::leak(format!("tile-{columns}").into_boxed_str()))
            .unwrap();
        assert_eq!(first.origin.y, second.origin.y);
        assert!(second.origin.x > first.origin.x);
        assert!(wrapped.origin.y > first.origin.y);
        assert_eq!(wrapped.origin.x, first.origin.x);
    }
}
