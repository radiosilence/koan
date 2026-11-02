//! A table's row, as the Mac's lists draw theirs: fixed cells leading (a
//! status, a number, a sleeve), the title over its lines of detail flexing
//! and truncating, and fixed figure columns trailing, right-aligned (a
//! codec, a length). Its height, padding, inset and column gap are
//! `RowMetrics`', so every list's rows line up under the page's header.
//!
//! It takes elements, not records: what a cell shows is the page's, where it
//! sits is the row's. Listeners attach to it as to a `div` (clicks, drags,
//! drops, menus), and its selection is drawn here. A row has no hover fill
//! (the user's call over the spec's 30 % `hover`): what shows under the
//! pointer is the play mark and the heart, through `ROW_GROUP`.

use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{RowKind, RowMetrics};
use crate::theme::{KoanStyled, Look, TypeRole};

/// A fixed-width cell, its width in points at 100 % and grown with the text
/// size.
struct Cell {
    width: Width,
    content: AnyElement,
}

enum Width {
    Points(f32),
    Exact(Pixels),
}

#[derive(IntoElement)]
/// A virtualised list of `TableRow`s: only the rows on screen are made, so a
/// table of tens of thousands (a library's organise plan) costs a screenful.
/// `rows` makes the rows of a range; with `cx.processor` it reads the view.
pub fn table_list(
    id: impl Into<ElementId>,
    count: usize,
    rows: impl 'static + Fn(std::ops::Range<usize>, &mut Window, &mut App) -> Vec<TableRow>,
) -> UniformList {
    uniform_list(id, count, rows)
}

pub struct TableRow {
    base: Stateful<Div>,
    kind: RowKind,
    selected: bool,
    leading: Vec<Cell>,
    title: Option<AnyElement>,
    lines: Vec<AnyElement>,
    trailing: Vec<Cell>,
}

impl TableRow {
    pub fn new(id: impl Into<ElementId>, kind: RowKind) -> Self {
        Self {
            base: div().id(id),
            kind,
            selected: false,
            leading: Vec::new(),
            title: None,
            lines: Vec::new(),
            trailing: Vec::new(),
        }
    }

    /// A cell before the title, `points` wide, its content at its trailing
    /// edge so a column of numbers lines up.
    pub fn lead(mut self, points: f32, content: impl IntoElement) -> Self {
        self.leading.push(Cell {
            width: Width::Points(points),
            content: content.into_any_element(),
        });
        self
    }

    /// A leading cell measured by the list, as a number column wide enough
    /// for its longest number.
    pub fn lead_exact(mut self, width: Pixels, content: impl IntoElement) -> Self {
        self.leading.push(Cell {
            width: Width::Exact(width),
            content: content.into_any_element(),
        });
        self
    }

    /// The title, in `body` and `ink` unless the content says otherwise.
    pub fn title(mut self, content: impl IntoElement) -> Self {
        self.title = Some(content.into_any_element());
        self
    }

    /// A line under the title, in `meta` and `muted` unless the content says
    /// otherwise: a credit, a record, a count.
    pub fn line(mut self, content: impl IntoElement) -> Self {
        self.lines.push(content.into_any_element());
        self
    }

    /// A figure column after the title, `points` wide and right-aligned, in
    /// `meta` and `muted`.
    pub fn figure(mut self, points: f32, content: impl IntoElement) -> Self {
        self.trailing.push(Cell {
            width: Width::Points(points),
            content: content.into_any_element(),
        });
        self
    }

    /// Picked: on `selection`, its title, lines and figures in
    /// `on_selection` unless their content says otherwise, which a page
    /// that colours its cells does by `on_selection` too.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }
}

impl InteractiveElement for TableRow {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl StatefulInteractiveElement for TableRow {}

/// For what a row's owner draws on its edges, as a drop's insertion line.
impl Styled for TableRow {
    fn style(&mut self) -> &mut StyleRefinement {
        self.base.style()
    }
}

/// One line of a row: clipped at the column's edge.
fn clipped(content: AnyElement) -> Div {
    div()
        .min_w(px(0.))
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .child(content)
}

/// The hover group every row is, for what shows only under the pointer:
/// the play button over the number, the heart.
pub const ROW_GROUP: &str = "koan-row";

/// What a row's play button runs.
type Play = std::rc::Rc<dyn Fn(&mut Window, &mut App)>;

/// A row's number column: the number, the playing bars in its place while
/// the row is the current track (moving while it plays, flat while it is
/// paused, so the paused track still reads as the current one), and under the pointer the play button, as the Mac's
/// `RowPlayButton`: the play mark in the accent, which plays from the row.
#[derive(IntoElement)]
pub struct RowNumber {
    id: ElementId,
    number: SharedString,
    glyph: Option<crate::theme::Icon>,
    current: bool,
    bars: Option<AnyElement>,
    on_play: Option<Play>,
}

impl RowNumber {
    pub fn new(id: impl Into<ElementId>, number: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            number: number.into(),
            glyph: None,
            current: false,
            bars: None,
            on_play: None,
        }
    }

    /// A row with no number leads with its kind's glyph instead, in
    /// `muted`, as the Mac's browsers do: the microphone for an artist.
    pub fn glyph(id: impl Into<ElementId>, icon: crate::theme::Icon) -> Self {
        Self {
            glyph: Some(icon),
            ..Self::new(id, SharedString::default())
        }
    }

    /// The row is the one playing or paused: its number in the accent.
    pub fn current(mut self, current: bool) -> Self {
        self.current = current;
        self
    }

    /// Drawn in the number's place while the row is current, playing or
    /// paused: the bars fall flat on a pause rather than giving the number
    /// back.
    pub fn bars(mut self, bars: impl IntoElement) -> Self {
        self.bars = Some(bars.into_any_element());
        self
    }

    pub fn on_play(mut self, play: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_play = Some(std::rc::Rc::new(play));
        self
    }
}

impl RenderOnce for RowNumber {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let accent = look.text_color(crate::theme::Tone::Accent);
        if let Some(bars) = self.bars {
            return div().flex().justify_end().child(bars).into_any_element();
        }
        let tone = if self.current {
            accent
        } else {
            look.palette.muted.hsla()
        };
        let number = div()
            .text_role(TypeRole::Meta, &look)
            .text_color(tone)
            .when(self.on_play.is_some(), |d| {
                d.group_hover(ROW_GROUP, |s| s.invisible())
            })
            .map(|d| match self.glyph {
                Some(icon) => d.child(
                    crate::theme::KoanIcon::new(icon)
                        .role(TypeRole::Body)
                        .color(tone),
                ),
                None => d.child(self.number),
            });
        div()
            .relative()
            .flex()
            .justify_end()
            .child(number)
            .when_some(self.on_play, |d, play| {
                d.child(
                    div()
                        .id(self.id)
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_end()
                        .invisible()
                        .group_hover(ROW_GROUP, |s| s.visible())
                        .cursor_pointer()
                        .child(
                            crate::theme::KoanIcon::new(crate::theme::Icon::PlayMark)
                                .role(TypeRole::Body)
                                .color(accent),
                        )
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            play(window, cx)
                        }),
                )
            })
            .into_any_element()
    }
}

impl RenderOnce for TableRow {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let p = look.palette;
        let scale = look.scale.size;
        let selected = self.selected;
        let on = p.on_selection.hsla();
        let cell = |cell: Cell| {
            div()
                .w(match cell.width {
                    Width::Points(points) => px(points * scale),
                    Width::Exact(width) => width,
                })
                .flex_none()
                .flex()
                .items_center()
                .justify_end()
                .child(cell.content)
        };
        self.base
            .group(ROW_GROUP)
            .h(RowMetrics::height(self.kind, &look))
            .w_full()
            .flex()
            .items_center()
            .px(RowMetrics::page_margin(&look))
            .py(RowMetrics::padding(&look))
            .gap(RowMetrics::column_gap(&look))
            .when(self.selected, |d| d.bg(p.selection.hsla()))
            .children(self.leading.into_iter().map(cell))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .justify_center()
                    .children(self.title.map(|title| {
                        clipped(title)
                            .text_role(TypeRole::Body, &look)
                            .text_color(if selected { on } else { p.ink.hsla() })
                    }))
                    .children(self.lines.into_iter().map(|line| {
                        clipped(line)
                            .text_role(TypeRole::Meta, &look)
                            .text_color(if selected { on } else { p.muted.hsla() })
                    })),
            )
            .children(self.trailing.into_iter().map(|figure| {
                cell(figure)
                    .text_role(TypeRole::Meta, &look)
                    .text_color(if selected { on } else { p.muted.hsla() })
            }))
    }
}
