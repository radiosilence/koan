//! What the EQ pages present over themselves: a stack of sheets, each a
//! square panel in a hairline over the page, the top one taking the pointer.
//! A confirmation says what a change will do before it is made.

use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, AnyView, App, AppContext, Context, Entity, InteractiveElement, IntoElement,
    MouseButton, ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::components::{Button, ButtonKind};
use crate::theme::{HAIRLINE, KoanStyled, Look, Space, Tone, TypeRole, label};

/// What a choice or a confirmation does.
pub type Run = Rc<dyn Fn(&mut Window, &mut App)>;

/// A change waiting to be confirmed: what it is, what it will do, and the
/// word that does it.
#[derive(Clone)]
pub struct Confirm {
    pub title: SharedString,
    pub message: SharedString,
    pub action: SharedString,
    pub destructive: bool,
    /// A second way to go ahead, beside the first.
    pub also: Option<(SharedString, Run)>,
    pub run: Run,
}

#[derive(Clone)]
pub enum Sheet {
    View(AnyView),
    Confirm(Confirm),
}

/// The sheets over a page, the last on top.
#[derive(Default)]
pub struct Sheets {
    stack: Vec<Sheet>,
}

impl Sheets {
    pub fn new(cx: &mut App) -> Entity<Self> {
        cx.new(|_| Self::default())
    }

    pub fn push(&mut self, sheet: Sheet, cx: &mut Context<Self>) {
        self.stack.push(sheet);
        cx.notify();
    }

    pub fn view(&mut self, view: impl Into<AnyView>, cx: &mut Context<Self>) {
        self.push(Sheet::View(view.into()), cx);
    }

    pub fn confirm(&mut self, confirm: Confirm, cx: &mut Context<Self>) {
        self.push(Sheet::Confirm(confirm), cx);
    }

    /// Closes the sheet showing `view`, wherever it is in the stack: what
    /// finishes after the person has moved on closes itself and nothing
    /// above or below it.
    pub fn close(&mut self, view: gpui_kit::EntityId, cx: &mut Context<Self>) {
        let before = self.stack.len();
        self.stack
            .retain(|s| !matches!(s, Sheet::View(v) if v.entity_id() == view));
        if self.stack.len() != before {
            cx.notify();
        }
    }

    /// Closes the sheet on top.
    pub fn pop(&mut self, cx: &mut Context<Self>) {
        if self.stack.pop().is_some() {
            cx.notify();
        }
    }

    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }

    /// The sheets drawn over the page, each panel over a scrim of the one
    /// beneath.
    pub fn render(this: &Entity<Self>, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
        let stack = this.read(cx).stack.clone();
        if stack.is_empty() {
            return None;
        }
        let look = Look::of(window, cx);
        let layers = stack.into_iter().enumerate().map(|(depth, sheet)| {
            let body = match sheet {
                Sheet::View(view) => view.into_any_element(),
                Sheet::Confirm(c) => confirm_body(c, this.clone(), &look).into_any_element(),
            };
            div()
                .id(("dsp-sheet", depth))
                .absolute()
                .inset_0()
                .flex()
                .justify_center()
                .items_start()
                .pt(look.space(Space::Xxl))
                .bg(look.palette.bg.hsla().opacity(0.6))
                .occlude()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(panel(&look).child(body))
        });
        Some(
            div()
                .absolute()
                .inset_0()
                .children(layers)
                .into_any_element(),
        )
    }
}

/// A sheet's panel: square, `bg`, a hairline `rule`, 560 wide.
pub fn panel(look: &Look) -> gpui_kit::Div {
    div()
        .flex()
        .flex_col()
        .w(px(560.0))
        .max_h(gpui_kit::relative(0.85))
        .bg(look.palette.bg.hsla())
        .border(HAIRLINE)
        .border_color(look.palette.rule.hsla())
}

/// A sheet's head: its title in the app's words, and its actions trailing.
pub fn head(title: impl AsRef<str>, actions: Vec<AnyElement>, look: &Look) -> impl IntoElement {
    head_data(label(title), actions, look)
}

/// A sheet's head whose title is data, a profile's name, which keeps its
/// case.
pub fn head_data(
    title: impl Into<SharedString>,
    actions: Vec<AnyElement>,
    look: &Look,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(look.space(Space::M))
        .px(look.space(Space::L))
        .py(look.space(Space::M))
        .border_b(HAIRLINE)
        .border_color(look.palette.rule.hsla())
        .child(
            div()
                .text_role(TypeRole::Title, look)
                .text_color(look.palette.strong.hsla())
                .child(title.into()),
        )
        .child(div().flex().gap(look.space(Space::M)).children(actions))
}

/// A sheet's scrolling body.
pub fn body(id: impl Into<gpui_kit::ElementId>, look: &Look) -> gpui_kit::Stateful<gpui_kit::Div> {
    div()
        .id(id.into())
        .flex()
        .flex_col()
        .gap(look.space(Space::S))
        .px(look.space(Space::L))
        .pb(look.space(Space::L))
        // Shrinks below its content inside the panel's height, and scrolls.
        .min_h_0()
        .overflow_y_scroll()
}

fn confirm_body(c: Confirm, sheets: Entity<Sheets>, look: &Look) -> impl IntoElement {
    let cancel = {
        let sheets = sheets.clone();
        move |_: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut App| {
            sheets.update(cx, |s, cx| s.pop(cx));
        }
    };
    let go = |run: Run| {
        let sheets = sheets.clone();
        move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut App| {
            sheets.update(cx, |s, cx| s.pop(cx));
            run(window, cx);
        }
    };
    let also = c.also.clone().map(|(word, run)| {
        Button::new("confirm-also", ButtonKind::Bordered)
            .label(word.as_ref())
            .destructive(c.destructive)
            .on_click(go(run))
    });
    div()
        .flex()
        .flex_col()
        .gap(look.space(Space::M))
        .p(look.space(Space::L))
        .child(
            div()
                .text_role(TypeRole::Title, look)
                .text_color(look.palette.strong.hsla())
                .child(c.title),
        )
        .child(
            div()
                .text_role(TypeRole::Meta, look)
                .text_color(look.text_color(Tone::Muted))
                .child(c.message),
        )
        .child(
            div()
                .flex()
                .justify_end()
                .gap(look.space(Space::M))
                .child(
                    Button::new("confirm-cancel", ButtonKind::Standard)
                        .label("Cancel")
                        .on_click(cancel),
                )
                .children(also)
                .child(
                    Button::new("confirm-go", ButtonKind::Bordered)
                        .label(c.action.as_ref())
                        .destructive(c.destructive)
                        .on_click(go(c.run.clone())),
                ),
        )
}

/// A line of `fine` text in a tone, for footers and notes.
pub fn note(text: impl Into<SharedString>, tone: Tone, look: &Look) -> gpui_kit::Div {
    div()
        .text_role(TypeRole::Fine, look)
        .text_color(look.text_color(tone))
        .child(text.into())
}

/// A section: its heading, its rows, and a footer saying what it means.
pub fn section(
    title: Option<&str>,
    rows: Vec<AnyElement>,
    footer: Option<SharedString>,
    look: &Look,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(look.space(Space::Xs))
        .when_some(title, |d, t| {
            d.child(crate::components::SectionHeader::new(t))
        })
        .children(rows)
        .when_some(footer, |d, f| d.child(note(f, Tone::Muted, look)))
}

/// A row a click acts on: what it is, a line beneath, and what trails.
pub fn choice_row(
    id: impl Into<gpui_kit::ElementId>,
    title: impl Into<SharedString>,
    detail: Option<SharedString>,
    trailing: Option<AnyElement>,
    look: &Look,
    on_click: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id.into())
        .flex()
        .items_center()
        .justify_between()
        .gap(look.space(Space::M))
        .py(look.space(Space::S))
        .cursor_pointer()
        .hover(|s| s.bg(look.palette.surface.hsla()))
        .on_click(on_click)
        .child(
            div()
                .flex()
                .flex_col()
                .min_w_0()
                .child(
                    div()
                        .text_role(TypeRole::Body, look)
                        .text_color(look.palette.ink.hsla())
                        .child(title.into()),
                )
                .when_some(detail, |d, detail| d.child(note(detail, Tone::Muted, look))),
        )
        .children(trailing)
}
