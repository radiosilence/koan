use std::sync::Arc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, ClickEvent, ElementId, InteractiveElement, IntoElement, ParentElement,
    RenderOnce, SharedString, StatefulInteractiveElement, Styled, Window, div,
};

use super::{Handler, child, fade, fill, pointer, track};
use crate::theme::{
    HIT, Icon, KoanIcon, KoanStyled, Look, MARK, Motion, ROW_HOVER_OPACITY, Space, TypeRole, label,
};

/// A navigation row, as the sidebar's: the label in `body`, `muted`, after
/// its glyph when icons are on; where you are, in the accent with a 2-point
/// accent rule on the leading edge, and no fill.
#[derive(IntoElement)]
pub struct NavRow {
    id: ElementId,
    label: SharedString,
    icon: Option<Icon>,
    selected: bool,
    on_click: Option<Handler<ClickEvent>>,
}

impl NavRow {
    pub fn new(id: impl Into<ElementId>, text: impl AsRef<str>) -> Self {
        Self {
            id: id.into(),
            label: label(text),
            icon: None,
            selected: false,
            on_click: None,
        }
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
        div()
            .id(self.id)
            .relative()
            .flex()
            .items_center()
            .gap(look.space(Space::S))
            .px(look.space(Space::L))
            .min_h(look.line_height(TypeRole::Body) + look.space(Space::S) * 2.0)
            .text_role(TypeRole::Body, &look)
            .text_color(color)
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
                    .filter(|_| look.icons)
                    .map(|icon| KoanIcon::new(icon).color(color)),
            )
            .child(self.label)
            .when_some(on_click, |row, on_click| {
                row.cursor_pointer()
                    .on_click(move |event, window, cx| on_click(event, window, cx))
            })
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
        let state = pointer(&self.id, window, cx);
        let hovered = state.read(cx).hovered;
        let hover = fade(child(&self.id, "hover"), hovered, Motion::FAST, window, cx);
        let selected = fade(
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
            .child(fill(
                look.palette.hover.hsla().opacity(ROW_HOVER_OPACITY),
                hover,
            ))
            .child(fill(look.palette.surface.hsla(), selected))
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
        track(row, &state).when_some(on_click, |row, on_click| {
            row.on_click(move |event, window, cx| on_click(event, window, cx))
        })
    }
}
