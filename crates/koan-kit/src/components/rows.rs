use std::sync::Arc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, ClickEvent, Div, ElementId, InteractiveElement, Interactivity, IntoElement,
    ParentElement, RenderOnce, SharedString, Stateful, StatefulInteractiveElement, Styled, Window,
    div,
};

use super::{Handler, accessible, answers_click, child, fade, fill, transition};
use crate::theme::{HIT, Icon, KoanIcon, KoanStyled, Look, MARK, Motion, Space, TypeRole, label};

/// The hover group a nav row's glyph brightens by.
const NAV_ROW: &str = "koan-nav-row";

/// A navigation row, as the sidebar's: the label in `body`, `muted`, after
/// its glyph when icons are on; where you are, in the accent with a 2-point
/// accent rule on the leading edge, and no fill.
///
/// A place with a face of its own (a playlist) carries its art before the
/// label and a line of detail under it. Listeners attach as to a `div`, so
/// a row can take drops.
#[derive(IntoElement)]
pub struct NavRow {
    id: ElementId,
    base: Stateful<Div>,
    label: SharedString,
    icon: Option<Icon>,
    art: Option<AnyElement>,
    detail: Option<SharedString>,
    selected: bool,
    /// Under the keyboard's cursor while its list holds the keyboard.
    cursor: bool,
    on_click: Option<Handler<ClickEvent>>,
}

impl InteractiveElement for NavRow {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl StatefulInteractiveElement for NavRow {}

impl NavRow {
    pub fn new(id: impl Into<ElementId>, text: impl AsRef<str>) -> Self {
        Self::data(id, label(text))
    }

    /// A place named by data (a playlist), which keeps its case.
    pub fn data(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Self {
        let id = id.into();
        Self {
            base: div().id(id.clone()),
            id,
            label: text.into(),
            icon: None,
            art: None,
            detail: None,
            selected: false,
            cursor: false,
            on_click: None,
        }
    }

    /// Under the keyboard's cursor: its list holds the keyboard and the
    /// arrows have come to it. Drawn with the focus ring while the keyboard
    /// was used last.
    pub fn cursor(mut self, cursor: bool) -> Self {
        self.cursor = cursor;
        self
    }

    /// The place's own face, square, before the label: a playlist's sleeve.
    /// Drawn whatever "show icons" says, as covers are.
    pub fn art(mut self, art: impl IntoElement) -> Self {
        self.art = Some(art.into_any_element());
        self
    }

    /// A line under the label, in `meta`, `muted`: a playlist's count.
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Arc::new(handler));
        self
    }
}

impl RenderOnce for NavRow {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let on = fade(
            child(&self.id, "selected"),
            self.selected,
            Motion::FAST,
            window,
            cx,
        );
        let color = super::mix(
            look.palette.muted.hsla(),
            look.text_color(crate::theme::Tone::Accent),
            on,
        );
        let on_click = self.on_click;
        let ink = look.palette.ink.hsla();
        let name = self.label.clone();
        let label = div()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .child(self.label);
        let selected = self.selected;
        // A link to a screen reader: to its place, selected where it is.
        accessible(self.base, gpui_kit::Role::Link, Some(name))
            .aria_selected(selected)
            .group(NAV_ROW)
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .gap(look.space(Space::S))
            .px(look.space(Space::L))
            .h(super::RowMetrics::nav_height(self.detail.is_some(), &look))
            .text_role(TypeRole::Body, &look)
            .text_color(color)
            .when(!self.selected, move |row| {
                row.hover(move |s| s.text_color(ink))
            })
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(MARK)
                    .bg(look.accent.hsla().opacity(on)),
            )
            .children(
                self.icon
                    .filter(|_| look.icons && self.art.is_none())
                    // A glyph draws in its own colour: it brightens with the
                    // label under the pointer by the row's group.
                    .map(|icon| {
                        let glyph = KoanIcon::new(icon).color(color);
                        if selected {
                            glyph
                        } else {
                            glyph.hover_color(NAV_ROW, ink)
                        }
                    }),
            )
            .children(self.art)
            .map(|row| match self.detail {
                None => row.child(label),
                Some(detail) => row.child(
                    div().min_w_0().flex().flex_col().child(label).child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_role(TypeRole::Meta, &look)
                            .text_color(look.palette.muted.hsla())
                            .child(detail),
                    ),
                ),
            })
            .when_some(on_click, |row, on_click| {
                answers_click(row, on_click.clone())
                    .cursor_pointer()
                    .on_click(move |event, window, cx| on_click(event, window, cx))
            })
            .children(
                (self.cursor && window.last_input_was_keyboard())
                    .then(|| super::cursor_ring(&look)),
            )
    }
}

/// A heading over a group of navigation rows: `fine`, `ink`, lowercase,
/// with 16 points above, at the rows' inset.
#[derive(IntoElement)]
pub struct NavHeading {
    title: SharedString,
    trailing: Option<AnyElement>,
}

impl NavHeading {
    pub fn new(title: impl AsRef<str>) -> Self {
        Self {
            title: label(title),
            trailing: None,
        }
    }

    /// At the heading's far end: an action on the group ("new").
    pub fn trailing(mut self, trailing: impl IntoElement) -> Self {
        self.trailing = Some(trailing.into_any_element());
        self
    }
}

impl RenderOnce for NavHeading {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        accessible(
            div().id(ElementId::Name(
                format!("nav-heading-{}", self.title).into(),
            )),
            gpui_kit::Role::Heading,
            Some(self.title.clone()),
        )
        .aria_level(2)
        .flex()
        .flex_none()
        .items_end()
        .justify_between()
        .pt(look.space(Space::L))
        .pb(look.space(Space::Xs))
        .px(look.space(Space::L))
        .text_role(TypeRole::Fine, &look)
        .text_color(look.palette.ink.hsla())
        .child(self.title)
        .children(self.trailing)
    }
}

/// A line at the foot of a navigation list: what the app is doing, or what
/// went wrong. One line, ended in an ellipsis where it runs past the list,
/// so a long path in an error never widens or lengthens the foot.
#[derive(IntoElement)]
pub struct NavNote {
    text: SharedString,
    tone: crate::theme::Tone,
}

impl NavNote {
    pub fn new(text: impl Into<SharedString>, tone: crate::theme::Tone) -> Self {
        Self {
            text: text.into(),
            tone,
        }
    }
}

impl RenderOnce for NavNote {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        div()
            .w_full()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .px(look.space(Space::L))
            .py(look.space(Space::Xs))
            .text_role(TypeRole::Fine, &look)
            .text_color(look.text_color(self.tone))
            .child(self.text)
    }
}

/// A list row: no rule between rows; `hover` at 30 % behind it under the
/// pointer, `surface` when selected, and the playing row's title and number
/// in the accent. The caller lays out the row's parts with `ListRow::title`,
/// `ListRow::detail` and `ListRow::number` or its own children.
#[derive(IntoElement)]
pub struct ListRow {
    id: ElementId,
    selected: bool,
    playing: bool,
    number: Option<SharedString>,
    title: Option<SharedString>,
    detail: Option<SharedString>,
    trailing: Vec<AnyElement>,
    on_click: Option<Handler<ClickEvent>>,
}

impl ListRow {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            selected: false,
            playing: false,
            number: None,
            title: None,
            detail: None,
            trailing: Vec::new(),
            on_click: None,
        }
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn playing(mut self, playing: bool) -> Self {
        self.playing = playing;
        self
    }

    /// A track number or place, leading.
    pub fn number(mut self, number: impl Into<SharedString>) -> Self {
        self.number = Some(number.into());
        self
    }

    /// The title, from the library, in its own case.
    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Secondary text under the title, in `meta`, `muted`.
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// A figure at the trailing edge, right-aligned in `meta`, `muted`, such
    /// as a duration.
    pub fn figure(mut self, figure: impl Into<SharedString>) -> Self {
        self.trailing
            .push(FigureCell(figure.into()).into_any_element());
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Arc::new(handler));
        self
    }
}

#[derive(IntoElement)]
struct FigureCell(SharedString);

impl RenderOnce for FigureCell {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        div()
            .flex_none()
            .text_right()
            .text_role(TypeRole::Meta, &look)
            .text_color(look.palette.muted.hsla())
            .child(self.0)
    }
}

impl RenderOnce for ListRow {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let selected = transition(
            child(&self.id, "selected"),
            self.selected,
            Motion::FAST,
            window,
            cx,
        );
        let lead = if self.playing {
            look.text_color(crate::theme::Tone::Accent)
        } else {
            look.palette.ink.hsla()
        };
        let number_color = if self.playing {
            look.text_color(crate::theme::Tone::Accent)
        } else {
            look.palette.muted.hsla()
        };
        let on_click = self.on_click;
        let row = div()
            .id(self.id)
            .relative()
            .flex()
            .items_center()
            .gap(look.space(Space::M))
            .px(look.space(Space::M))
            .py(look.spaced(12.0))
            .min_h(HIT)
            .child(fill(look.palette.surface.hsla(), &selected))
            .children(self.number.map(|number| {
                div()
                    .relative()
                    .flex_none()
                    .min_w(look.spaced(28.0))
                    .text_right()
                    .text_role(TypeRole::Meta, &look)
                    .text_color(number_color)
                    .child(number)
            }))
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .children(self.title.map(|title| {
                        div()
                            .text_role(TypeRole::Body, &look)
                            .text_color(lead)
                            .truncate()
                            .child(title)
                    }))
                    .children(self.detail.map(|detail| {
                        div()
                            .text_role(TypeRole::Meta, &look)
                            .text_color(look.palette.muted.hsla())
                            .truncate()
                            .child(detail)
                    })),
            )
            .children(
                self.trailing
                    .into_iter()
                    .map(|cell| div().relative().child(cell)),
            );
        row.when_some(on_click, |row, on_click| {
            row.on_click(move |event, window, cx| on_click(event, window, cx))
        })
    }
}
