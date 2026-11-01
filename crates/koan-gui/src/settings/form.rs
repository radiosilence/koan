//! The pieces every pane is laid out from: the Mac's `KoanForm` sections in
//! the theme, square and stacked with no cards. The layout is the system's
//! (`components::form`); these are the panes' short names for it.

use crate::components::ScrollArea;
use crate::theme::{Icon, KoanIcon, KoanStyled, Look, Space, Tone, TypeRole};
use gpui_kit::prelude::*;
use gpui_kit::*;

pub use crate::components::FormRow as Row;
use crate::components::{Form, FormSection, form_buttons, form_item, form_note, form_text};

/// A pane's page: the system's `Form`.
pub fn page(id: &'static str, look: &Look, sections: Vec<AnyElement>) -> ScrollArea {
    Form::new(id).sections(sections).build(look)
}

/// A section: the system's `FormSection`.
pub fn section(
    header: Option<&str>,
    rows: Vec<AnyElement>,
    footer: Option<SharedString>,
    _: &Look,
) -> AnyElement {
    let mut section = match header {
        Some(title) => FormSection::titled(title),
        None => FormSection::new(),
    }
    .rows(rows);
    if let Some(footer) = footer {
        section = section.footer(footer);
    }
    section.into_any_element()
}

pub fn text(text: impl Into<SharedString>, role: TypeRole, tone: Tone, look: &Look) -> Div {
    form_text(text, role, tone, look)
}

pub fn note(text: impl Into<SharedString>, icon: Icon, tone: Tone, look: &Look) -> AnyElement {
    form_note(text, icon, tone, look)
}

pub fn buttons(children: Vec<AnyElement>, look: &Look) -> AnyElement {
    form_buttons(children, look)
}

pub fn item(
    title: impl Into<SharedString>,
    detail: Option<SharedString>,
    controls: Vec<AnyElement>,
    look: &Look,
) -> AnyElement {
    form_item(title, detail, controls, look)
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
