//! A panel beside the page, the Mac's inspector: its title in the app's
//! words with what trails it (a source, a spinner), a hairline under the
//! head, a hairline down its leading edge, and its body. On `bg` unless the
//! wash runs under the whole window, where it is clear over the wash.
//!
//! `SyncedLines` is the body a panel of timed text takes: the lines of a
//! song's lyrics, the one being sung larger and in `strong`, kept in view.

use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{RowMetrics, ScrollArea};
use crate::theme::{KoanStyled, Look, Space, TypeRole, label};

#[derive(IntoElement)]
pub struct Inspector {
    title: SharedString,
    trailing: Option<AnyElement>,
    body: Option<AnyElement>,
    clear: bool,
}

impl Inspector {
    pub fn new(title: impl AsRef<str>) -> Self {
        Self {
            title: label(title),
            trailing: None,
            body: None,
            clear: false,
        }
    }

    /// At the head's far edge: where the content came from, or that it is
    /// still coming.
    pub fn trailing(mut self, trailing: impl IntoElement) -> Self {
        self.trailing = Some(trailing.into_any_element());
        self
    }

    pub fn body(mut self, body: impl IntoElement) -> Self {
        self.body = Some(body.into_any_element());
        self
    }

    /// No ground of its own: the wash runs under the whole window.
    pub fn clear(mut self, clear: bool) -> Self {
        self.clear = clear;
        self
    }
}

impl RenderOnce for Inspector {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let p = look.palette;
        div()
            .w(RowMetrics::inspector(&look))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .when(!self.clear, |d| d.bg(p.bg.hsla()))
            .border_l(look.hairline)
            .border_color(p.rule.hsla())
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(look.space(Space::L))
                    .py(look.spaced(11.0))
                    .border_b(look.hairline)
                    .border_color(p.rule.hsla())
                    .child(
                        div()
                            .text_role(TypeRole::Body, &look)
                            .text_color(p.strong.hsla())
                            .child(self.title),
                    )
                    .children(self.trailing.map(|t| {
                        div()
                            .text_role(TypeRole::Fine, &look)
                            .text_color(p.muted.hsla())
                            .child(t)
                    })),
            )
            .child(div().flex_1().min_h_0().children(self.body))
    }
}

/// Lines of timed text, the one at `active` in `title_small`, `strong`, the
/// rest in `control`, `muted`, the active one kept in view as it moves. A
/// click on a line hands its index to `on_line`.
#[derive(IntoElement)]
pub struct SyncedLines {
    id: ElementId,
    lines: Vec<SharedString>,
    active: Option<usize>,
    on_line: Option<super::Handler<usize>>,
}

impl SyncedLines {
    pub fn new(id: impl Into<ElementId>, lines: Vec<SharedString>, active: Option<usize>) -> Self {
        Self {
            id: id.into(),
            lines,
            active,
            on_line: None,
        }
    }

    pub fn on_line(mut self, handler: impl Fn(&usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_line = Some(std::sync::Arc::new(handler));
        self
    }
}

impl RenderOnce for SyncedLines {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let p = look.palette;
        let pad = look.spaced(18.0);
        // Room above the first line and below the last, so either can sit
        // mid-panel as the song reaches it.
        let air = || div().h(look.spaced(60.0)).into_any_element();
        let mut items = Vec::with_capacity(self.lines.len() + 2);
        items.push(air());
        for (ix, line) in self.lines.into_iter().enumerate() {
            let on = self.active == Some(ix);
            let on_line = self.on_line.clone();
            items.push(
                div()
                    .id(ix)
                    .map(|d| {
                        #[cfg(test)]
                        let d = d.debug_selector(move || format!("synced-line-{ix}"));
                        d
                    })
                    .w_full()
                    .px(pad)
                    .py(look.spaced(5.5))
                    .cursor_pointer()
                    .map(|d| {
                        if on {
                            d.text_role(TypeRole::TitleSmall, &look)
                                .text_color(p.strong.hsla())
                        } else {
                            d.text_role(TypeRole::Control, &look)
                                .text_color(p.muted.hsla())
                        }
                    })
                    .when_some(on_line, |d, f| {
                        d.on_click(move |_, window, cx| f(&ix, window, cx))
                    })
                    // An empty line keeps its height: a pause in the song.
                    .child(if line.is_empty() { " ".into() } else { line })
                    .into_any_element(),
            );
        }
        items.push(air());
        ScrollArea::items(self.id, items).reveal(self.active.map(|ix| ix + 1))
    }
}

/// Text that does not follow the song, in `control`, `ink`, wrapping.
pub fn plain_text(text: impl Into<SharedString>, look: &Look) -> Div {
    div()
        .p(look.spaced(18.0))
        .text_role(TypeRole::Control, look)
        .text_color(look.palette.ink.hsla())
        .child(text.into())
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui_kit::{Modifiers, TestAppContext, VisualTestContext};

    use super::*;
    use core::prelude::v1::test;

    struct Lines {
        clicked: Rc<Cell<Option<usize>>>,
    }

    impl Render for Lines {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let clicked = self.clicked.clone();
            div().w(px(280.)).h(px(400.)).child(
                SyncedLines::new(
                    "lines",
                    vec!["one".into(), "".into(), "three".into()],
                    Some(2),
                )
                .on_line(move |ix, _, _| clicked.set(Some(*ix))),
            )
        }
    }

    /// A click on a line hands its index over, an empty line keeping its
    /// place among the others.
    #[gpui_kit::test]
    fn a_click_names_the_line(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
        });
        let clicked = Rc::new(Cell::new(None));
        let (_, cx) = cx.add_window_view(|_, _| Lines {
            clicked: clicked.clone(),
        });
        let cx: &mut VisualTestContext = cx;
        cx.run_until_parked();
        let at = cx
            .debug_bounds("synced-line-2")
            .expect("the line is drawn")
            .center();
        cx.simulate_click(at, Modifiers::default());
        assert_eq!(clicked.get(), Some(2));
    }
}
