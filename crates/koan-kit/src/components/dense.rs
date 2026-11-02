//! A dense table, as a sheet holds one: an EQ's bands, a curve's points.
//! Columns are declared once, fixed or flexing, aligned at their start or
//! their end, in points that grow with the text size; the header and every
//! row lay their cells out on them, so a figure lines up down the table
//! whatever its row holds. A cell takes one column or spans several, and a
//! row may end in a trailing cell (an options menu) past the columns.

use std::rc::Rc;

use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::theme::{KoanStyled, Look, Space, Tone, TypeRole};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Start,
    End,
}

#[derive(Clone, Copy, Debug)]
pub struct Column {
    /// Points at 100 %, or `None` to take what is left.
    width: Option<f32>,
    align: Align,
}

impl Column {
    pub fn fixed(points: f32, align: Align) -> Self {
        Self {
            width: Some(points),
            align,
        }
    }

    pub fn flex() -> Self {
        Self {
            width: None,
            align: Align::Start,
        }
    }
}

/// A row's cells: each takes one column, or `span` of them.
#[derive(Default)]
pub struct DenseRow {
    cells: Vec<(usize, AnyElement)>,
    trailing: Option<AnyElement>,
}

impl DenseRow {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cell(mut self, content: impl IntoElement) -> Self {
        self.cells.push((1, content.into_any_element()));
        self
    }

    /// One cell across `columns` columns.
    pub fn span(mut self, columns: usize, content: impl IntoElement) -> Self {
        self.cells
            .push((columns.max(1), content.into_any_element()));
        self
    }

    /// Past the columns, at the row's end.
    pub fn trailing(mut self, content: impl IntoElement) -> Self {
        self.trailing = Some(content.into_any_element());
        self
    }
}

#[derive(IntoElement)]
pub struct DenseTable {
    columns: Rc<[Column]>,
    header: Option<Vec<SharedString>>,
    rows: Vec<DenseRow>,
}

impl DenseTable {
    pub fn new(columns: impl Into<Rc<[Column]>>) -> Self {
        Self {
            columns: columns.into(),
            header: None,
            rows: Vec::new(),
        }
    }

    /// The columns' titles, in `fine`, `muted`, as written: they are units
    /// ("Hz", "dB") as often as words.
    pub fn header(mut self, titles: impl IntoIterator<Item = impl Into<SharedString>>) -> Self {
        self.header = Some(titles.into_iter().map(Into::into).collect());
        self
    }

    pub fn row(mut self, row: DenseRow) -> Self {
        self.rows.push(row);
        self
    }

    pub fn rows(mut self, rows: impl IntoIterator<Item = DenseRow>) -> Self {
        self.rows.extend(rows);
        self
    }
}

/// `columns` laid out as one cell: its width, or the rest of the row when
/// any of them flexes; aligned as its first column is.
fn cell(columns: &[Column], gap: Pixels, look: &Look, content: AnyElement) -> Div {
    let flexes = columns.iter().any(|c| c.width.is_none());
    let align = columns.first().map_or(Align::Start, |c| c.align);
    let width: f32 = columns.iter().filter_map(|c| c.width).sum();
    let gaps = gap * columns.len().saturating_sub(1) as f32;
    div()
        .min_w_0()
        .flex()
        .items_center()
        .map(|d| {
            if flexes {
                d.flex_1()
            } else {
                d.flex_none().w(px(width * look.scale.size) + gaps)
            }
        })
        .when(align == Align::End, |d| d.justify_end())
        .child(content)
}

fn lay_out(columns: &[Column], row: DenseRow, gap: Pixels, look: &Look) -> Div {
    let mut at = 0;
    let mut cells = Vec::with_capacity(row.cells.len());
    for (span, content) in row.cells {
        let end = (at + span).min(columns.len());
        if at >= end {
            break;
        }
        cells.push(cell(&columns[at..end], gap, look, content));
        at = end;
    }
    div()
        .flex()
        .items_center()
        .gap(gap)
        .children(cells)
        .children(row.trailing.map(|t| div().flex_none().child(t)))
}

impl RenderOnce for DenseTable {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let gap = look.space(Space::S);
        let columns = self.columns.clone();
        let header = self.header.map(|titles| {
            let row = titles
                .into_iter()
                .fold(DenseRow::new(), |row, title| row.cell(title));
            lay_out(&columns, row, gap, &look)
                .text_role(TypeRole::Fine, &look)
                .text_color(look.text_color(Tone::Muted))
        });
        div()
            .flex()
            .flex_col()
            .children(header)
            .children(self.rows.into_iter().map(|row| {
                lay_out(&columns, row, gap, &look)
                    .py(look.spaced(2.0))
                    .text_role(TypeRole::Meta, &look)
                    .text_color(look.palette.ink.hsla())
            }))
    }
}
