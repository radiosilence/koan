//! A search over everything, centred over the window: Arc's and Dia's
//! command bars. The query large across the top, what it finds under it in
//! headed groups of `SuggestionRow`s, the one lit by the arrows on
//! `surface` and kept in view, and along the foot what the keys do. The
//! window behind is dimmed and takes no clicks; a click on it closes the
//! bar.
//!
//! The bar is two parts its owner draws apart: the query's field
//! (`CommandBar::field`) and what was found (`CommandResults`). An owner
//! keeps each in a view of its own, as a filter field is kept beside the
//! listing it narrows, so a keystroke draws the field and nothing else, and
//! the results are drawn only when an answer lands. The owner holds the
//! query's `InputState`, decides what is found and what each key does.

use crate::components::field::FieldState as InputState;
use gpui::prelude::*;
use gpui::*;

use super::clear_mark::ClearMark;
use super::{Rise, ScrollArea, SuggestionHeading, SuggestionRow, TextField, sheet};
use crate::theme::{Icon, KoanIcon, KoanStyled, Look, Space, Tone, TypeRole, label};

/// A group of results under its heading.
pub struct CommandSection {
    pub title: SharedString,
    pub rows: Vec<SuggestionRow>,
}

/// The command bar over the window: the owner's field and the results
/// under it, closed by a press on the dimmed window behind.
#[derive(IntoElement)]
pub struct CommandBar {
    id: ElementId,
    field: AnyElement,
    results: Option<AnyElement>,
    on_dismiss: Option<super::Handler<()>>,
}

impl CommandBar {
    /// The bar's width at 100 %, and how far down the window it hangs.
    const WIDTH: f32 = 640.0;
    const TOP: f32 = 0.14;

    /// `field` is the query's field as `CommandBar::field` draws it, or a
    /// view drawing it.
    pub fn new(id: impl Into<ElementId>, field: impl IntoElement) -> Self {
        Self {
            id: id.into(),
            field: field.into_any_element(),
            results: None,
            on_dismiss: None,
        }
    }

    /// The query's field: bare, in `title small`, named "search" for a
    /// screen reader, with a clear mark at its trailing edge while it holds
    /// text, as a filter field has. The owner runs the same clear from a key
    /// with `FieldState::clear`.
    pub fn field(input: &Entity<InputState>) -> CommandField {
        CommandField {
            input: input.clone(),
            label: label("search"),
            role: TypeRole::TitleSmall,
            disabled: false,
        }
    }

    /// How tall the field is: what a view drawing it, cached, is sized to.
    pub fn field_height(window: &Window, cx: &App) -> Pixels {
        Look::of(window, cx).line_height(TypeRole::TitleSmall)
    }

    /// What was found and the keys' foot, as `CommandResults` draws them,
    /// or a view drawing them, cached in the room under the query
    /// (`flex_1`, `min_h_0`, `w_full`).
    pub fn results(mut self, results: impl IntoElement) -> Self {
        self.results = Some(results.into_any_element());
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
        let dismiss = self.on_dismiss;
        let height = window.viewport_size().height;
        sheet::scrim_top(self.id.clone(), &look)
            .pt(height * Self::TOP)
            .when_some(dismiss, |d, dismiss| {
                d.on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    dismiss(&(), window, cx)
                })
            })
            .child(Rise::in_place(
                // A dialog to a screen reader, holding the query and its
                // results.
                super::accessible(
                    div().id(super::child(&self.id, "bar")),
                    gpui::Role::Dialog,
                    Some(label("search")),
                )
                .w(look.spaced(Self::WIDTH))
                .max_w(window.viewport_size().width - look.space(Space::M))
                // What the window has below the panel's top, less a margin:
                // in a small window the list scrolls between the query and
                // the keys rather than running past them. The room is
                // fixed, so the results can be a cached view (whose size is
                // its style's alone); the panel's ground and edge are drawn
                // by the query and the results, around what they hold, and
                // the room below them is the dimmed window, which closes
                // the bar.
                .h(height * (1.0 - Self::TOP) - look.space(Space::Xl))
                .flex()
                .flex_col()
                .child(
                    super::accessible(
                        div().id(super::child(&self.id, "query")),
                        gpui::Role::ComboBox,
                        Some(label("search")),
                    )
                    .flex_none()
                    .bg(p.bg.hsla())
                    .border_t(look.hairline)
                    .border_l(look.hairline)
                    .border_r(look.hairline)
                    .border_color(p.rule.hsla())
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .flex()
                    .items_center()
                    .gap(look.space(Space::M))
                    .px(look.space(Space::L))
                    .py(look.space(Space::M))
                    .child(
                        KoanIcon::new(Icon::Search)
                            .role(TypeRole::TitleSmall)
                            .tone(Tone::Muted),
                    )
                    .child(div().flex_1().min_w_0().child(self.field)),
                )
                .children(self.results),
            ))
    }
}

/// The command bar's query field, as `CommandBar::field` makes it: the
/// text, and the clear mark while there is any. Escape is the owner's.
#[derive(IntoElement)]
pub struct CommandField {
    input: Entity<InputState>,
    label: SharedString,
    role: TypeRole,
    disabled: bool,
}

impl CommandField {
    /// What a screen reader names it, in the app's words, in place of
    /// "search": what is searched.
    pub fn label(mut self, text: &str) -> Self {
        self.label = label(text);
        self
    }

    /// Its text and mark in `role` rather than `title small`. A view
    /// drawing it cached is sized to that role's line, not
    /// `CommandBar::field_height`.
    pub fn role(mut self, role: TypeRole) -> Self {
        self.role = role;
        self
    }

    /// Not editable, faded and with no clear mark: while what it feeds is
    /// under way.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl RenderOnce for CommandField {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let target = look.target();
        ClearMark::of(&self.input, self.role, self.disabled)
            .row(
                None,
                TextField::new(&self.input)
                    .label(self.label)
                    .bare()
                    .role(self.role)
                    .disabled(self.disabled),
                // A finger's target under touch, the glyph at the trailing
                // edge and centred on the line, standing over the row's
                // height where it is shorter.
                |mark| {
                    mark.min_w(target)
                        .min_h(target)
                        .flex()
                        .items_center()
                        .justify_end()
                },
                None,
                &look,
                cx,
            )
            .size_full()
            .gap(look.space(Space::M))
    }
}

/// What the bar found, under its query: headed groups of rows, scrolling
/// between the query and the keys when they outgrow the panel, and along the
/// foot what the keys do.
#[derive(IntoElement)]
pub struct CommandResults {
    id: ElementId,
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
}

impl CommandResults {
    /// Results with no sections yet.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            sections: Vec::new(),
            lit: None,
            keys: Vec::new(),
            empty: None,
            last: None,
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

    /// The row the keyboard is on, counted across the sections.
    pub fn lit(mut self, lit: Option<usize>) -> Self {
        self.lit = lit;
        self
    }

    /// A key and what it does, along the foot, in the app's words.
    pub fn key(mut self, keys: impl Into<SharedString>, verb: impl AsRef<str>) -> Self {
        self.keys.push((keys.into(), label(verb)));
        self
    }

    /// What it says when no section has a row.
    pub fn empty(mut self, text: impl AsRef<str>) -> Self {
        self.empty = Some(label(text));
        self
    }
}

impl RenderOnce for CommandResults {
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
            // scrolls within it: each level down to the scroll shrinks with
            // the panel, so the scroll is no taller than the room it has and
            // the wheel moves it.
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .border_t(look.hairline)
                .border_color(p.rule.hsla())
                .pb(look.space(Space::Xs))
                .child(
                    // The results, a list box for a screen reader.
                    super::accessible(
                        div()
                            .id(super::child(&self.id, "list"))
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .flex_col(),
                        gpui::Role::ListBox,
                        Some(label("results")),
                    )
                    .child(
                        ScrollArea::items(super::child(&self.id, "rows"), items)
                            .reveal(reveal)
                            .shrinks(),
                    ),
                )
                .into_any_element()
        } else {
            div()
                .flex_none()
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
        // As tall as what it holds, up to the room under the query: the
        // panel's ground and its edge below the query.
        div()
            .w_full()
            .max_h_full()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(p.bg.hsla())
            .border_l(look.hairline)
            .border_r(look.hairline)
            .border_b(look.hairline)
            .border_color(p.rule.hsla())
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(body)
            .child(foot)
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Modifiers, TestAppContext, TouchEvent, TouchId, TouchPhase, VisualTestContext};

    use super::*;
    use crate::components::field::FieldState;
    use core::prelude::v1::test;

    /// The query's field in a cached view of its own, as `search/bar.rs`
    /// keeps it.
    struct Query {
        input: Entity<InputState>,
        disabled: bool,
    }

    impl Render for Query {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .child(CommandBar::field(&self.input).disabled(self.disabled))
        }
    }

    struct Bar {
        query: Entity<Query>,
    }

    impl Render for Bar {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let line = CommandBar::field_height(window, cx);
            CommandBar::new(
                "bar",
                self.query
                    .clone()
                    .cached(StyleRefinement::default().w_full().h(line)),
            )
        }
    }

    /// How the field is set up in a test.
    #[derive(Default)]
    struct Setup {
        touch: bool,
        /// The cached view redraws when the input changes, as an owner
        /// listening to it does; without, only the field's own notices
        /// reach it.
        observed: bool,
        disabled: bool,
        masked: bool,
    }

    fn open(setup: Setup, cx: &mut TestAppContext) -> (Entity<InputState>, &mut VisualTestContext) {
        cx.update(|cx| {
            crate::theme::init(cx);
            crate::theme::KoanTheme::set_touch(setup.touch, cx);
        });
        let (bar, cx) = cx.add_window_view(|window, cx| {
            let input = cx.new(|cx| {
                FieldState::new(window, cx)
                    .placeholder("search")
                    .masked(setup.masked)
            });
            let query = cx.new(|cx| {
                if setup.observed {
                    cx.observe(&input, |_, _, cx| cx.notify()).detach();
                }
                Query {
                    input,
                    disabled: setup.disabled,
                }
            });
            Bar { query }
        });
        let input = bar.read_with(cx, |b, cx| b.query.read(cx).input.clone());
        cx.run_until_parked();
        (input, cx)
    }

    fn typed(input: &Entity<InputState>, text: &str, cx: &mut VisualTestContext) {
        cx.update(|window, cx| input.update(cx, |i, cx| i.focus(window, cx)));
        cx.simulate_input(text);
        cx.run_until_parked();
    }

    fn value(input: &Entity<InputState>, cx: &mut VisualTestContext) -> String {
        cx.update(|_, cx| input.read(cx).value().to_string())
    }

    fn focused(input: &Entity<InputState>, cx: &mut VisualTestContext) -> bool {
        cx.update(|window, cx| input.read(cx).focus_handle(cx).is_focused(window))
    }

    fn press_mark(cx: &mut VisualTestContext) {
        let clear = cx
            .debug_bounds("field-clear")
            .expect("the mark shows with text");
        cx.simulate_mouse_down(clear.center(), MouseButton::Left, Modifiers::default());
        cx.run_until_parked();
    }

    fn tap(id: u64, at: Point<Pixels>, cx: &mut VisualTestContext) {
        for phase in [TouchPhase::Started, TouchPhase::Ended] {
            cx.simulate_event(TouchEvent {
                id: TouchId(id),
                phase,
                position: at,
                ..Default::default()
            });
        }
        cx.run_until_parked();
    }

    /// Typing shows the clear mark; a press on it empties the query and
    /// leaves the keyboard in it, and the mark goes.
    #[gpui::test]
    fn the_query_shows_a_clear_mark_that_empties_it(cx: &mut TestAppContext) {
        let (input, cx) = open(
            Setup {
                observed: true,
                ..Setup::default()
            },
            cx,
        );
        assert!(cx.debug_bounds("field-clear").is_none());
        typed(&input, "abba", cx);
        press_mark(cx);
        assert_eq!(value(&input, cx), "");
        assert!(focused(&input, cx), "the query keeps the keyboard");
        assert!(cx.debug_bounds("field-clear").is_none());
    }

    /// Under touch the mark is a finger's target, taller than the row, and
    /// a tap anywhere on it, its edge past the row included, empties the
    /// query and keeps the keyboard: the mark is the field's, not somewhere
    /// else to tap.
    #[gpui::test]
    fn a_tap_on_the_clear_mark_keeps_the_keyboard(cx: &mut TestAppContext) {
        let (input, cx) = open(
            Setup {
                touch: true,
                observed: true,
                ..Setup::default()
            },
            cx,
        );
        typed(&input, "abba", cx);
        let clear = cx
            .debug_bounds("field-clear")
            .expect("the mark shows with text");
        let line = cx.update(|window, cx| CommandBar::field_height(window, cx));
        assert!(clear.size.height > line, "{clear:?} over a {line:?} row");
        // A tap in the text first, so the field is drawn under touch.
        tap(1, point(clear.left() - px(40.), clear.center().y), cx);
        let clear = cx.debug_bounds("field-clear").expect("still there");
        // Near the mark's top edge: above the row, which is centred on it.
        let at = point(clear.center().x, clear.top() + px(2.));
        assert!(at.y < clear.center().y - line / 2., "{at:?} is in the row");
        tap(2, at, cx);
        assert_eq!(value(&input, cx), "");
        assert!(
            focused(&input, cx),
            "the tap on the mark put the keyboard away"
        );
    }

    /// A disabled query draws no mark to clear it with.
    #[gpui::test]
    fn a_disabled_query_has_no_clear_mark(cx: &mut TestAppContext) {
        let (input, cx) = open(
            Setup {
                observed: true,
                disabled: true,
                ..Setup::default()
            },
            cx,
        );
        cx.update(|window, cx| input.update(cx, |i, cx| i.set_value("abba", window, cx)));
        cx.run_until_parked();
        assert_eq!(value(&input, cx), "abba");
        assert!(cx.debug_bounds("field-clear").is_none());
    }

    /// The cleared text fades and then goes, redrawing the cached view that
    /// holds the field though its owner does not listen to the input.
    #[gpui::test]
    fn the_cleared_text_goes_from_a_cached_view(cx: &mut TestAppContext) {
        let (input, cx) = open(Setup::default(), cx);
        typed(&input, "abba", cx);
        press_mark(cx);
        assert_eq!(value(&input, cx), "");
        assert!(
            cx.debug_bounds("field-cleared").is_some(),
            "the cleared text fades"
        );
        cx.executor()
            .advance_clock(crate::theme::Motion::SETTLE * 2);
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("field-cleared").is_none(),
            "the cleared text stayed on screen"
        );
    }

    /// Typing while the cleared text fades ends the fade at once, so the
    /// two never overlap.
    #[gpui::test]
    fn typing_ends_the_fade(cx: &mut TestAppContext) {
        let (input, cx) = open(
            Setup {
                observed: true,
                ..Setup::default()
            },
            cx,
        );
        typed(&input, "abba", cx);
        press_mark(cx);
        assert!(cx.update(|_, cx| input.read(cx).ghost().is_some()));
        cx.simulate_input("x");
        cx.run_until_parked();
        assert_eq!(value(&input, cx), "x");
        assert!(!cx.update(|_, cx| input.read(cx).ghost().is_some()));
        assert!(cx.debug_bounds("field-cleared").is_none());
    }

    /// A masked query's cleared text fades as it was drawn, never as
    /// plain text.
    #[gpui::test]
    fn a_masked_query_fades_its_bullets(cx: &mut TestAppContext) {
        let (input, cx) = open(
            Setup {
                observed: true,
                masked: true,
                ..Setup::default()
            },
            cx,
        );
        typed(&input, "abba", cx);
        press_mark(cx);
        let ghost = cx.update(|_, cx| input.read(cx).ghost().map(str::to_owned));
        let ghost = ghost.expect("the cleared text fades");
        assert!(!ghost.contains("abba"), "{ghost:?}");
        assert_eq!(ghost.chars().count(), 4);
    }

    struct Plain {
        input: Entity<InputState>,
    }

    impl Render for Plain {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(300.)).child(super::TextField::new(&self.input))
        }
    }

    /// Any field fades what `clear` took, a clear mark or none.
    #[gpui::test]
    fn a_plain_field_fades_on_clear(cx: &mut TestAppContext) {
        cx.update(|cx| {
            crate::theme::init(cx);
        });
        let (plain, cx) = cx.add_window_view(|window, cx| Plain {
            input: cx.new(|cx| FieldState::new(window, cx).placeholder("name")),
        });
        let cx: &mut VisualTestContext = cx;
        let input = plain.read_with(cx, |p, _| p.input.clone());
        typed(&input, "abba", cx);
        cx.update(|window, cx| input.update(cx, |i, cx| i.clear(window, cx)));
        cx.run_until_parked();
        assert_eq!(value(&input, cx), "");
        assert!(cx.debug_bounds("field-cleared").is_some());
    }

    /// A read-only query shows no mark, and `clear` leaves its text, with
    /// nothing fading over it.
    #[gpui::test]
    fn a_read_only_query_keeps_its_text(cx: &mut TestAppContext) {
        let (input, cx) = open(
            Setup {
                observed: true,
                ..Setup::default()
            },
            cx,
        );
        cx.update(|window, cx| {
            input.update(cx, |i, cx| {
                i.set_value("abba", window, cx);
                i.set_read_only(true, cx);
            })
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("field-clear").is_none());
        cx.update(|window, cx| input.update(cx, |i, cx| i.clear(window, cx)));
        cx.run_until_parked();
        assert_eq!(value(&input, cx), "abba");
        assert!(cx.update(|_, cx| input.read(cx).ghost().is_none()));
        assert!(cx.debug_bounds("field-cleared").is_none());
    }

    /// Under touch the mark's hit area stands past the row, and only it
    /// does: a tap in the band beside it, above the row, puts the keyboard
    /// away.
    #[gpui::test]
    fn a_tap_beside_the_mark_lets_the_keyboard_go(cx: &mut TestAppContext) {
        let (input, cx) = open(
            Setup {
                touch: true,
                observed: true,
                ..Setup::default()
            },
            cx,
        );
        typed(&input, "abba", cx);
        let clear = cx
            .debug_bounds("field-clear")
            .expect("the mark shows with text");
        tap(1, point(clear.left() - px(40.), clear.center().y), cx);
        let clear = cx.debug_bounds("field-clear").expect("still there");
        let line = cx.update(|window, cx| CommandBar::field_height(window, cx));
        let at = point(clear.left() - px(20.), clear.top() + px(2.));
        assert!(at.y < clear.center().y - line / 2., "{at:?} is in the row");
        tap(2, at, cx);
        assert_eq!(value(&input, cx), "abba");
        assert!(
            !focused(&input, cx),
            "a tap beside the mark kept the keyboard"
        );
    }
}
