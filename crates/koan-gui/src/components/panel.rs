use std::sync::Arc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, AnyView, App, AppContext, ClickEvent, Context, ElementId, InteractiveElement,
    IntoElement, ParentElement, Render, RenderOnce, SharedString, StatefulInteractiveElement,
    Styled, Window, div,
};

use super::Handler;
use crate::theme::{HAIRLINE, Icon, KoanIcon, KoanStyled, Look, Space, Tone, TypeRole, label};

/// The theme's popover and menu: a square `bg` panel in a hairline `rule`,
/// with no arrow, no shadow and no material. What it holds is its own; a
/// menu's rows are `MenuRow`s.
#[derive(IntoElement)]
pub struct MenuPanel {
    children: Vec<AnyElement>,
}

impl MenuPanel {
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
        }
    }
}

impl Default for MenuPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl ParentElement for MenuPanel {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for MenuPanel {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        div()
            .flex()
            .flex_col()
            .py(look.space(Space::Xs))
            .bg(look.palette.bg.hsla())
            .border(HAIRLINE)
            .border_color(look.palette.rule.hsla())
            .children(self.children)
    }
}

/// A row of a menu: its label in `control`, `ink`, after its glyph when
/// icons are on, on `surface` under the pointer or the arrow keys. A choice
/// that is in force is ticked; a destructive one is in `bad`.
#[derive(IntoElement)]
pub struct MenuRow {
    id: ElementId,
    label: SharedString,
    icon: Option<Icon>,
    checked: Option<bool>,
    highlighted: bool,
    destructive: bool,
    on_click: Option<Handler<ClickEvent>>,
}

impl MenuRow {
    pub fn new(id: impl Into<ElementId>, text: impl AsRef<str>) -> Self {
        Self {
            id: id.into(),
            label: label(text),
            icon: None,
            checked: None,
            highlighted: false,
            destructive: false,
            on_click: None,
        }
    }

    /// A row whose label is data rather than the app's words (a preset's
    /// name, a device's), which keeps its case.
    pub fn data(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Self {
        Self {
            label: text.into(),
            ..Self::new(id, "")
        }
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// A label that is data rather than the app's words, kept as it is.
    pub fn label_data(mut self, text: impl Into<SharedString>) -> Self {
        self.label = text.into();
        self
    }

    /// One of a set of choices: ticked when in force, with room for the tick
    /// kept when not.
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = Some(checked);
        self
    }

    /// Lit by the arrow keys, as the pointer lights it.
    pub fn highlighted(mut self, highlighted: bool) -> Self {
        self.highlighted = highlighted;
        self
    }

    pub fn destructive(mut self, destructive: bool) -> Self {
        self.destructive = destructive;
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

impl RenderOnce for MenuRow {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let color = look.color(if self.destructive {
            Tone::Bad
        } else {
            Tone::Ink
        });
        let tick = look.points(TypeRole::Control);
        let on_click = self.on_click;
        let row = div()
            .id(self.id)
            .relative()
            .flex()
            .items_center()
            .gap(look.space(Space::S))
            .px(look.space(Space::M))
            .py(look.space(Space::Xs))
            .text_role(TypeRole::Control, &look)
            .text_color(color)
            .whitespace_nowrap()
            .when(self.highlighted, |row| row.bg(look.palette.surface.hsla()))
            .hover(|row| row.bg(look.palette.surface.hsla()))
            .children(self.checked.map(|checked| {
                div()
                    .relative()
                    .flex_none()
                    .size(gpui_kit::px(crate::theme::icons::side(tick)))
                    .when(checked, |cell| {
                        cell.child(KoanIcon::new(Icon::Check).points(tick).color(color))
                    })
            }))
            .children(self.icon.filter(|_| look.icons).map(|icon| {
                div()
                    .relative()
                    .child(KoanIcon::new(icon).role(TypeRole::Control).color(color))
            }))
            .child(div().relative().child(self.label));
        row.when_some(on_click, |row, on_click| {
            row.cursor_pointer()
                .on_click(move |event, window, cx| on_click(event, window, cx))
        })
    }
}

/// A tooltip, in the theme's panel: `fine`, `ink`, on `bg` in a hairline
/// `rule`.
pub struct Tooltip {
    text: SharedString,
}

impl Tooltip {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self { text: text.into() }
    }

    /// The view an element's `.tooltip(..)` builder returns.
    pub fn view(text: SharedString, _window: &mut Window, cx: &mut App) -> AnyView {
        cx.new(|_| Tooltip::new(text)).into()
    }
}

impl Render for Tooltip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        div()
            .px(look.space(Space::S))
            .py(look.space(Space::Xs))
            .bg(look.palette.bg.hsla())
            .border(HAIRLINE)
            .border_color(look.palette.rule.hsla())
            .text_role(TypeRole::Fine, &look)
            .text_color(look.palette.ink.hsla())
            .child(self.text.clone())
    }
}
