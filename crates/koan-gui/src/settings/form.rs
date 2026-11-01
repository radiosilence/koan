//! The pieces every pane is laid out from: the Mac's `KoanForm` sections in
//! the theme, square and stacked with no cards.

use crate::components::ScrollArea;
use crate::components::SectionHeader;
use crate::theme::{Icon, KoanIcon, KoanStyled, Look, Space, Tone, TypeRole};
use koan_kit::raw::prelude::*;
use koan_kit::raw::*;

/// A pane's page: sections down a column that scrolls, across the window.
pub fn page(id: &'static str, look: &Look, sections: Vec<AnyElement>) -> ScrollArea {
    ScrollArea::new(
        id,
        div()
            .flex()
            .flex_col()
            .gap(look.space(Space::L))
            .w_full()
            .px(look.space(Space::Xl))
            .pb(look.space(Space::Xl))
            .children(sections),
    )
}

/// The width of a form's label column, before spacing's scale: where the
/// Mac's forms start their controls.
const LABELS: f32 = 210.;

/// A form's row in two columns: the label in `body`, `ink`, lowercase, and
/// what it holds starting at the control column: a value in `control`,
/// `muted`, or a control, or both.
#[derive(IntoElement)]
pub struct Row {
    label: SharedString,
    value: Option<SharedString>,
    control: Option<AnyElement>,
}

impl Row {
    pub fn new(text: impl AsRef<str>) -> Self {
        Self {
            label: crate::theme::label(text),
            value: None,
            control: None,
        }
    }

    /// A label that is data, which keeps its case.
    pub fn data(text: impl Into<SharedString>) -> Self {
        Self {
            label: text.into(),
            value: None,
            control: None,
        }
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

impl RenderOnce for Row {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        div()
            .flex()
            .items_center()
            .min_h(crate::theme::HIT)
            .child(
                div()
                    .flex_none()
                    .w(look.spaced(LABELS))
                    .pr(look.space(Space::M))
                    .text_role(TypeRole::Body, &look)
                    .text_color(look.palette.ink.hsla())
                    .child(self.label),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
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
                    .children(self.control),
            )
    }
}

/// A section: its header, its rows, and the footer that explains it.
pub fn section(
    header: Option<&str>,
    rows: Vec<AnyElement>,
    footer: Option<SharedString>,
    look: &Look,
) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(look.space(Space::S))
        .children(header.map(SectionHeader::new))
        .child(div().flex().flex_col().children(rows))
        .children(footer.map(|footer| {
            text(footer, TypeRole::Fine, Tone::Muted, look).pt(look.space(Space::Xs))
        }))
        .into_any_element()
}

/// Text in a role and tone, wrapping.
pub fn text(text: impl Into<SharedString>, role: TypeRole, tone: Tone, look: &Look) -> Div {
    div()
        .text_role(role, look)
        .text_color(look.text_color(tone))
        .child(text.into())
}

/// A line with a glyph before it: a warning, a state.
pub fn note(text: impl Into<SharedString>, icon: Icon, tone: Tone, look: &Look) -> AnyElement {
    let color = look.text_color(tone);
    div()
        .flex()
        .items_start()
        .gap(look.space(Space::Xs))
        .py(look.space(Space::Xs))
        .text_role(TypeRole::Meta, look)
        .text_color(color)
        .child(KoanIcon::new(icon).role(TypeRole::Meta).color(color))
        .child(div().flex_1().child(text.into()))
        .into_any_element()
}

/// Controls side by side, the last pushed to the far end when `split`.
pub fn buttons(children: Vec<AnyElement>, look: &Look) -> AnyElement {
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(look.space(Space::M))
        .py(look.space(Space::S))
        .children(children)
        .into_any_element()
}

/// A row of a list in a form: a title and a line under it, and controls at
/// the end.
pub fn item(
    title: impl Into<SharedString>,
    detail: Option<SharedString>,
    controls: Vec<AnyElement>,
    look: &Look,
) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(look.space(Space::M))
        .py(look.space(Space::Xs))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(text(title, TypeRole::Body, Tone::Ink, look).truncate())
                .children(detail.map(|detail| text(detail, TypeRole::Fine, Tone::Muted, look))),
        )
        .children(controls)
        .into_any_element()
}

/// A value a click copies, such as an address another device asks for:
/// "copied" for a moment after.
pub fn copyable(
    id: &'static str,
    value: String,
    copied: bool,
    on_click: impl Fn(&mut App) + 'static,
    look: &Look,
) -> Stateful<Div> {
    let ink = look.palette.ink.hsla();
    div()
        .id(id)
        .cursor_pointer()
        .text_role(TypeRole::Control, look)
        .text_color(look.palette.muted.hsla())
        .hover(move |s| s.text_color(ink))
        .truncate()
        .child(if copied {
            crate::theme::label("Copied")
        } else {
            value.into()
        })
        .tooltip(|window, cx| crate::components::Tooltip::view("copy".into(), window, cx))
        .on_click(move |_, _, cx| on_click(cx))
}

/// A day, as "1 Nov 2026", in the local time zone.
pub fn day(secs: i64) -> String {
    use chrono::{Local, TimeZone};
    Local
        .timestamp_opt(secs, 0)
        .single()
        .map(|t| t.format("%-d %b %Y").to_string())
        .unwrap_or_default()
}

/// A moment: the time alone today, the day and the time before.
pub fn moment(secs: i64) -> String {
    use chrono::{Local, TimeZone};
    let Some(at) = Local.timestamp_opt(secs, 0).single() else {
        return String::new();
    };
    if at.date_naive() == Local::now().date_naive() {
        at.format("%H:%M").to_string()
    } else {
        at.format("%-d %b %Y, %H:%M").to_string()
    }
}
