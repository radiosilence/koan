//! The form: the Mac's `KoanForm` with its sections and rows. The container
//! decides every gap: the page's padding, the space between sections, a
//! header's space above, the label column and the space between rows. A
//! pane declares sections and rows and nothing about where they sit.
//!
//! On the Mac a form is a column of sections on the ground, no cards or
//! boxes, each row its label in a fixed column and what it holds after it.

use gpui::prelude::*;
use gpui::*;

use super::ScrollArea;
use crate::theme::{Icon, KoanIcon, KoanStyled, Look, Space, Tone, TypeRole, label};

/// Where a form's controls start, before spacing's scale: the Mac's label
/// column.
const LABELS: f32 = 200.0;
/// A compact row's label column: a popover's or a panel's, 300 points or
/// so wide, where 200 would leave the control no room.
const COMPACT_LABELS: f32 = 80.0;
/// A row's least height on the Mac, the Mac toggle's hit height. The whole
/// row takes a toggle's click, so its target is the row's width.
const ROW: f32 = 24.0;

/// A form: sections down a column that scrolls, padded from the window.
pub struct Form {
    id: ElementId,
    sections: Vec<AnyElement>,
}

impl Form {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            sections: Vec::new(),
        }
    }

    pub fn section(mut self, section: impl IntoElement) -> Self {
        self.sections.push(section.into_any_element());
        self
    }

    pub fn sections(mut self, sections: impl IntoIterator<Item = AnyElement>) -> Self {
        self.sections.extend(sections);
        self
    }

    /// The form's column, laid out at the window's look.
    pub fn build(self, look: &Look) -> ScrollArea {
        ScrollArea::new(
            self.id,
            div()
                .flex()
                .flex_col()
                .gap(look.space(Space::L))
                .w_full()
                .p(look.space(Space::Xl))
                .children(self.sections),
        )
    }
}

/// A section: its header, its rows, and a footer saying what it means.
#[derive(IntoElement, Default)]
pub struct FormSection {
    header: Option<SharedString>,
    rows: Vec<AnyElement>,
    footer: Option<SharedString>,
}

impl FormSection {
    pub fn new() -> Self {
        Self::default()
    }

    /// Headed, in the app's words.
    pub fn titled(title: impl AsRef<str>) -> Self {
        Self {
            header: Some(label(title)),
            ..Self::default()
        }
    }

    pub fn row(mut self, row: impl IntoElement) -> Self {
        self.rows.push(row.into_any_element());
        self
    }

    pub fn rows(mut self, rows: impl IntoIterator<Item = AnyElement>) -> Self {
        self.rows.extend(rows);
        self
    }

    pub fn footer(mut self, footer: impl Into<SharedString>) -> Self {
        self.footer = Some(footer.into());
        self
    }
}

impl RenderOnce for FormSection {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        // On the Mac a section in the form's column lays its header, rows
        // and footer out as the column's own children, `l` apart like the
        // sections themselves.
        div()
            .flex()
            .flex_col()
            .gap(look.space(Space::L))
            .children(self.header.map(SectionHeader::data))
            .children(self.rows)
            .children(
                self.footer
                    .map(|footer| form_text(footer, TypeRole::Fine, Tone::Muted, &look)),
            )
    }
}

/// A form's row: the label in `body`, `ink`, lowercase, in the label column,
/// and what it holds after it: a value in `control`, `muted`, a control, or
/// both.
#[derive(IntoElement)]
pub struct FormRow {
    label: SharedString,
    value: Option<SharedString>,
    control: Option<AnyElement>,
    compact: bool,
    stacked: bool,
}

impl FormRow {
    /// A label in the app's words, drawn lowercase.
    pub fn new(text: impl AsRef<str>) -> Self {
        Self::data(label(text))
    }

    /// The narrow label column, for a row in a popover or a small panel,
    /// whose rows line up with each other rather than with a window's form.
    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }

    /// A label that is data rather than the app's words (an AutoEQ maker, a
    /// server extension), which keeps its case.
    pub fn data(text: impl Into<SharedString>) -> Self {
        Self {
            label: text.into(),
            value: None,
            control: None,
            compact: false,
            stacked: false,
        }
    }

    /// A control that wants the row's width, a slider: on a phone it goes
    /// under the label, across the whole row, rather than squeezed beside it.
    pub fn stacked(mut self) -> Self {
        self.stacked = true;
        self
    }

    pub fn value(mut self, value: impl Into<SharedString>) -> Self {
        self.value = Some(value.into());
        self
    }

    pub fn control(mut self, control: impl IntoElement) -> Self {
        self.control = Some(control.into_any_element());
        self
    }
}

impl RenderOnce for FormRow {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let stacked = self.stacked && look.touch;
        div()
            .flex()
            .when(stacked, |row| {
                row.flex_col().items_start().py(look.space(Space::S))
            })
            .when(!stacked, |row| row.items_center())
            .min_h(look.spaced(ROW))
            .child(
                div()
                    .flex_none()
                    // A phone's row is too narrow for the desktop's label
                    // column: the label takes its own width, at most half
                    // the row, and the control the rest.
                    .map(|label| {
                        if stacked {
                            label.w_full()
                        } else if look.touch {
                            label.max_w(relative(0.5))
                        } else {
                            label.w(look.spaced(if self.compact { COMPACT_LABELS } else { LABELS }))
                        }
                    })
                    .pr(look.space(Space::M))
                    .text_role(TypeRole::Body, &look)
                    .text_color(look.palette.ink.hsla())
                    .child(self.label.clone()),
            )
            .child(
                div()
                    .flex()
                    .when(stacked, |control| control.w_full().min_h(look.target()))
                    .when(!stacked, |control| control.flex_1())
                    .min_w_0()
                    .items_center()
                    .gap(look.space(Space::L))
                    .children(self.value.map(|value| {
                        div()
                            .min_w_0()
                            .truncate()
                            .text_role(TypeRole::Control, &look)
                            .text_color(look.palette.muted.hsla())
                            .child(value)
                    }))
                    .children(
                        self.control
                            .map(|control| super::field::labelled(self.label.clone(), control)),
                    ),
            )
    }
}

/// A section's heading: `fine`, `ink`, lowercase, with 16 points above.
#[derive(IntoElement)]
pub struct SectionHeader {
    title: SharedString,
}

impl SectionHeader {
    pub fn new(title: impl AsRef<str>) -> Self {
        Self::data(label(title))
    }

    pub fn data(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
        }
    }
}

impl RenderOnce for SectionHeader {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        div()
            .pt(look.space(Space::L))
            .text_role(TypeRole::Fine, &look)
            .text_color(look.palette.ink.hsla())
            .child(self.title)
    }
}

/// Text in a form, in a role and tone, wrapping: a footer, an explanation.
pub fn form_text(text: impl Into<SharedString>, role: TypeRole, tone: Tone, look: &Look) -> Div {
    div()
        .text_role(role, look)
        .text_color(look.text_color(tone))
        .child(text.into())
}

/// A line with a glyph before it in a form: a warning, a state.
pub fn form_note(text: impl Into<SharedString>, icon: Icon, tone: Tone, look: &Look) -> AnyElement {
    let color = look.text_color(tone);
    div()
        .flex()
        .items_start()
        .gap(look.space(Space::Xs))
        .text_role(TypeRole::Meta, look)
        .text_color(color)
        .child(KoanIcon::new(icon).role(TypeRole::Meta).color(color))
        .child(div().flex_1().child(text.into()))
        .into_any_element()
}

/// A form's buttons side by side, wrapping.
pub fn form_buttons(children: Vec<AnyElement>, look: &Look) -> AnyElement {
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(look.space(Space::M))
        .children(children)
        .into_any_element()
}

/// A field the width of the form with its verbs after it, on one line: the
/// Mac's pair-a-device code and Approve. The field takes what the verbs leave,
/// and the verbs stand as tall as the field, whatever their kind's type.
pub fn form_entry(field: AnyElement, actions: Vec<AnyElement>, look: &Look) -> AnyElement {
    entry(div().flex_1().min_w_0().child(field), actions, look)
        .w_full()
        .into_any_element()
}

/// A field `points` wide at 100 % with its verbs after it, as tall as it: a
/// name to add, an address to add.
pub fn form_field(
    field: AnyElement,
    points: f32,
    actions: Vec<AnyElement>,
    look: &Look,
) -> AnyElement {
    entry(div().w(look.spaced(points)).child(field), actions, look).into_any_element()
}

/// A field and its verbs in a row, stretched to one height: a bordered verb's
/// smaller type no longer leaves it short beside the field.
fn entry(field: Div, actions: Vec<AnyElement>, look: &Look) -> Div {
    div()
        .flex()
        .gap(look.space(Space::M))
        .child(field.flex().flex_col().justify_center())
        .children(
            actions
                .into_iter()
                .map(|a| div().flex_none().flex().child(a)),
        )
}

/// A form's list item: a title and a line under it, and controls at the end.
pub fn form_item(
    title: impl Into<SharedString>,
    detail: Option<SharedString>,
    controls: Vec<AnyElement>,
    look: &Look,
) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(look.space(Space::M))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(form_text(title, TypeRole::Body, Tone::Ink, look).truncate())
                .children(
                    detail.map(|detail| form_text(detail, TypeRole::Fine, Tone::Muted, look)),
                ),
        )
        .children(controls)
        .into_any_element()
}
