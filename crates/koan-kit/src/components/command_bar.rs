//! A search over everything, centred over the window: Arc's and Dia's
//! command bars. The query large across the top, what it finds under it in
//! headed groups of `SuggestionRow`s, the one lit by the arrows on
//! `surface` and kept in view, and along the foot what the keys do. The
//! window behind is dimmed and takes no clicks; a click on it closes the
//! bar.
//!
//! The owner holds the query's `InputState`, decides what is found and
//! what each key does: the bar draws what it is given.

use gpui_kit::component::input::InputState;
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{ScrollArea, SuggestionHeading, SuggestionRow, TextField, sheet};
use crate::theme::{Icon, KoanIcon, KoanStyled, Look, Space, Tone, TypeRole, label};

/// A group of results under its heading.
pub struct CommandSection {
    pub title: SharedString,
    pub rows: Vec<SuggestionRow>,
}

#[derive(IntoElement)]
pub struct CommandBar {
    id: ElementId,
    input: Entity<InputState>,
    sections: Vec<CommandSection>,
    /// The lit row's place among all the rows, headings left out.
    lit: Option<usize>,
    /// What the keys do, as (keys, verb): "⌘↩", "play next".
    keys: Vec<(SharedString, SharedString)>,
    /// What the body says with nothing found.
    empty: Option<SharedString>,
    /// A row after every section, under no heading ("All results…"); lit as
    /// the row after the last section's last.
    last: Option<SuggestionRow>,
    on_dismiss: Option<super::Handler<()>>,
}

impl CommandBar {
    /// The bar's width at 100 %, and how far down the window it hangs.
    const WIDTH: f32 = 640.0;
    const TOP: f32 = 0.14;

    pub fn new(id: impl Into<ElementId>, input: &Entity<InputState>) -> Self {
        Self {
            id: id.into(),
            input: input.clone(),
            sections: Vec::new(),
            lit: None,
            keys: Vec::new(),
            empty: None,
            last: None,
            on_dismiss: None,
        }
    }

    /// The row after every section, under no heading: what goes further
    /// than the list does ("All results…"). Shown only with results.
    pub fn last(mut self, row: SuggestionRow) -> Self {
        self.last = Some(row);
        self
    }

    /// A group of results; one with none is left out.
    pub fn section(mut self, title: impl AsRef<str>, rows: Vec<SuggestionRow>) -> Self {
        if !rows.is_empty() {
            self.sections.push(CommandSection {
                title: label(title),
                rows,
            });
        }
        self
    }

    pub fn lit(mut self, lit: Option<usize>) -> Self {
        self.lit = lit;
        self
    }

    /// A key and what it does, along the foot, in the app's words.
    pub fn key(mut self, keys: impl Into<SharedString>, verb: impl AsRef<str>) -> Self {
        self.keys.push((keys.into(), label(verb)));
        self
    }

    pub fn empty(mut self, text: impl AsRef<str>) -> Self {
        self.empty = Some(label(text));
        self
    }

    /// A click on the dimmed window around the bar.
    pub fn on_dismiss(mut self, handler: impl Fn(&(), &mut Window, &mut App) + 'static) -> Self {
        self.on_dismiss = Some(std::sync::Arc::new(handler));
        self
    }
}

impl RenderOnce for CommandBar {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let p = look.palette;
        let found = !self.sections.is_empty();
        // Every row and its heading, with where the lit row stands among
        // them for the scroll to keep it in view.
        let mut items: Vec<AnyElement> = Vec::new();
        let mut reveal = None;
        let mut row_ix = 0;
        for section in self.sections {
            items.push(SuggestionHeading::new(&section.title).into_any_element());
            for row in section.rows {
                if self.lit == Some(row_ix) {
                    reveal = Some(items.len());
                }
                items.push(row.lit(self.lit == Some(row_ix)).into_any_element());
                row_ix += 1;
            }
        }
        if let Some(row) = self.last.filter(|_| found) {
            if self.lit == Some(row_ix) {
                reveal = Some(items.len());
            }
            items.push(row.lit(self.lit == Some(row_ix)).into_any_element());
        }
        let body = if found {
            // Takes what the panel has left after the query and the keys, and
            // scrolls within it: the rows never draw past the panel, over
            // the keys or the page behind.
            div()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .pb(look.space(Space::Xs))
                .child(
                    // The results, a list box for a screen reader.
                    super::accessible(
                        div().id(super::child(&self.id, "list")).size_full(),
                        gpui_kit::Role::ListBox,
                        Some(label("results")),
                    )
                    .child(ScrollArea::items(super::child(&self.id, "rows"), items).reveal(reveal)),
                )
                .into_any_element()
        } else {
            div()
                .children(self.empty.map(|text| {
                    div()
                        .px(look.space(Space::L))
                        .py(look.space(Space::M))
                        .text_role(TypeRole::Control, &look)
                        .text_color(p.muted.hsla())
                        .child(text)
                }))
                .into_any_element()
        };
        // Opaque, so nothing of the list shows through it.
        let foot = div()
            .flex_none()
            .bg(p.bg.hsla())
            .flex()
            .flex_wrap()
            .items_center()
            .gap(look.space(Space::L))
            .px(look.space(Space::L))
            .py(look.space(Space::S))
            .border_t(look.hairline)
            .border_color(p.rule.hsla())
            .text_role(TypeRole::Fine, &look)
            .text_color(p.muted.hsla())
            .children(self.keys.into_iter().map(|(keys, verb)| {
                div()
                    .flex()
                    .gap(look.space(Space::Xs))
                    .child(div().text_color(p.ink.hsla()).child(keys))
                    .child(verb)
            }));
        let dismiss = self.on_dismiss;
        let height = window.viewport_size().height;
        sheet::scrim(self.id.clone(), &look)
            .pt(height * Self::TOP)
            .when_some(dismiss, |d, dismiss| {
                d.on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    dismiss(&(), window, cx)
                })
            })
            .child(
                // A dialog to a screen reader, holding the query and its
                // results.
                super::accessible(
                    div().id(super::child(&self.id, "bar")),
                    gpui_kit::Role::Dialog,
                    Some(label("search")),
                )
                .w(look.spaced(Self::WIDTH))
                // What the window has below the panel's top, less a margin:
                // in a small window the list scrolls between the query and
                // the keys rather than running past them.
                .max_h(height * (1.0 - Self::TOP) - look.space(Space::Xl))
                .overflow_hidden()
                .flex()
                .flex_col()
                .bg(p.bg.hsla())
                .border(look.hairline)
                .border_color(p.rule.hsla())
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    super::accessible(
                        div().id(super::child(&self.id, "query")),
                        gpui_kit::Role::ComboBox,
                        Some(label("search")),
                    )
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(look.space(Space::M))
                    .px(look.space(Space::L))
                    .py(look.space(Space::M))
                    .when(found, |d| {
                        d.border_b(look.hairline).border_color(p.rule.hsla())
                    })
                    .child(
                        KoanIcon::new(Icon::Search)
                            .role(TypeRole::TitleSmall)
                            .tone(Tone::Muted),
                    )
                    .child(
                        div().flex_1().min_w_0().child(
                            TextField::new(&self.input)
                                .bare()
                                .role(TypeRole::TitleSmall),
                        ),
                    ),
                )
                .child(body)
                .child(foot),
            )
    }
}
