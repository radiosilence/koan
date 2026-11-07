//! What the EQ pages present over themselves: a stack of sheets, each a
//! square panel in a hairline over the page, the top one taking the pointer.
//! On a phone a view's sheet is a screen over the whole window, on its wash,
//! as the pages the SwiftUI app pushes there are; a confirmation stays a
//! panel of its own size.
//! A confirmation says what a change will do before it is made.

use std::rc::Rc;

use koan_kit::app::*;

use crate::components::sheet::{screen, scrim};
use crate::components::{Button, ButtonKind, Rise, SlideUp};
use crate::theme::{KoanStyled, KoanTheme, Look, Space, Tone, TypeRole};

/// The sheet's parts are koan-kit's.
pub use crate::components::sheet::{body, content, head, head_data, note, panel};

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
    /// While a view's screen comes up on a phone: the one under it is drawn
    /// until it has.
    rising: Option<Task<()>>,
}

impl Sheets {
    pub fn new(cx: &mut App) -> Entity<Self> {
        cx.new(|_| Self::default())
    }

    pub fn push(&mut self, sheet: Sheet, cx: &mut Context<Self>) {
        if matches!(sheet, Sheet::View(_)) {
            self.rising = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(SlideUp::RISE).await;
                this.update(cx, |s, cx| {
                    s.rising = None;
                    cx.notify();
                })
                .ok();
            }));
        }
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
    pub fn close(&mut self, view: koan_kit::app::EntityId, cx: &mut Context<Self>) {
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
    /// beneath. On a phone a view's screen hides everything under it, so
    /// only the top one is drawn, with the questions over it; and the one
    /// beneath while it comes up.
    pub fn render(this: &Entity<Self>, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
        let sheets = this.read(cx);
        if sheets.stack.is_empty() {
            return None;
        }
        let look = Look::of(window, cx);
        let is_view = |s: &Sheet| matches!(s, Sheet::View(_));
        let first = match sheets.stack.iter().rposition(is_view) {
            Some(top) if look.touch => {
                let rising = sheets.rising.is_some() && KoanTheme::motion_allowed(cx);
                match rising.then(|| sheets.stack[..top].iter().rposition(is_view)) {
                    Some(under) => under.unwrap_or(0),
                    None => top,
                }
            }
            _ => 0,
        };
        let stack = sheets.stack.clone();
        let layers = stack
            .into_iter()
            .enumerate()
            .skip(first)
            .map(|(depth, sheet)| {
                // Named for a screen reader: a question by its title, a view by
                // the window it belongs to.
                let id = ("dsp-sheet", depth);
                match sheet {
                    Sheet::View(view) => screen(id, "eq", view, &look, window, cx),
                    Sheet::Confirm(c) => scrim(id, &look, window).child(Rise::new(
                        panel(c.title.clone(), &look).child(confirm_body(c, this.clone(), &look)),
                    )),
                }
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
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

fn confirm_body(c: Confirm, sheets: Entity<Sheets>, look: &Look) -> impl IntoElement {
    let cancel = {
        let sheets = sheets.clone();
        move |_: &koan_kit::app::ClickEvent, _: &mut Window, cx: &mut App| {
            sheets.update(cx, |s, cx| s.pop(cx));
        }
    };
    let go = |run: Run| {
        let sheets = sheets.clone();
        move |_: &koan_kit::app::ClickEvent, window: &mut Window, cx: &mut App| {
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
        .child(crate::components::title_text(c.title, look))
        .child(
            div()
                .text_role(TypeRole::Meta, look)
                .text_color(look.text_color(Tone::Muted))
                .child(c.message),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
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
        // The section's note stands a step off its rows, as a form's does.
        .when_some(footer, |d, f| {
            d.child(note(f, Tone::Muted, look).mt(look.space(Space::S)))
        })
}

/// A row a click acts on: what it is, a line beneath, and what trails, lit
/// under the pointer as the kit's controls are. The fill reaches past the
/// text by the space it is set in by, so the text lines up with the sheet's.
pub fn choice_row(
    id: impl Into<koan_kit::app::ElementId>,
    title: impl Into<SharedString>,
    detail: Option<SharedString>,
    trailing: Option<AnyElement>,
    look: &Look,
    on_click: impl Fn(&koan_kit::app::ClickEvent, &mut Window, &mut App) + 'static,
) -> ChoiceRow {
    ChoiceRow {
        id: id.into(),
        title: title.into(),
        detail,
        trailing,
        look: *look,
        on_click: Box::new(on_click),
    }
}

/// What a choice row runs when it is picked.
type OnChoose = Box<dyn Fn(&koan_kit::app::ClickEvent, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct ChoiceRow {
    id: koan_kit::app::ElementId,
    title: SharedString,
    detail: Option<SharedString>,
    trailing: Option<AnyElement>,
    look: Look,
    on_click: OnChoose,
}

impl RenderOnce for ChoiceRow {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = &self.look;
        let inset = look.space(Space::S);
        crate::components::lit_row(self.id, window, cx)
            .flex()
            .items_center()
            .justify_between()
            .gap(look.space(Space::M))
            .py(inset)
            .px(inset)
            .mx(-inset)
            .on_click(self.on_click)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .child(
                        div()
                            .text_role(TypeRole::Body, look)
                            .text_color(look.palette.ink.hsla())
                            .child(self.title),
                    )
                    .when_some(self.detail, |d, detail| {
                        d.child(note(detail, Tone::Muted, look))
                    }),
            )
            .children(self.trailing)
    }
}
