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
            .min_w(look.spaced(180.))
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
    disabled: bool,
    /// A row that opens a submenu: the chevron at its end.
    opens: bool,
    /// The rich row's second line, and whether it warns.
    detail: Option<(SharedString, bool)>,
    /// The rich row: its glyph large in a column of its own, the chosen row
    /// on `surface` with its glyph in the accent.
    rich: bool,
    /// A small glyph after the label: how a place is reached.
    reach: Option<Icon>,
    /// Buttons at the row's end, before the tick.
    trailing: Vec<AnyElement>,
    /// The keys that do the same, at the row's end in `muted`.
    keys: Option<SharedString>,
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
            disabled: false,
            opens: false,
            detail: None,
            rich: false,
            reach: None,
            trailing: Vec::new(),
            keys: None,
            on_click: None,
        }
    }

    /// The keys that do the same, as a menu bar shows them: "⌘K", "Ctrl+K".
    pub fn keys(mut self, keys: impl Into<SharedString>) -> Self {
        self.keys = Some(keys.into());
        self
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

    /// Not a choice now: `muted`, and nothing under the pointer.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Opens a submenu: a chevron at the row's end.
    pub fn opens(mut self, opens: bool) -> Self {
        self.opens = opens;
        self
    }

    /// A second line under the label, in `fine`, `muted`; in `bad` when it
    /// warns.
    pub fn detail(mut self, detail: impl Into<SharedString>, warns: bool) -> Self {
        self.detail = Some((detail.into(), warns));
        self
    }

    /// The rich row: a place to choose, with its glyph large beside a label
    /// and its detail, the chosen one on `surface` and its glyph in the
    /// accent.
    pub fn rich(mut self, rich: bool) -> Self {
        self.rich = rich;
        self
    }

    pub fn reach(mut self, reach: Icon) -> Self {
        self.reach = Some(reach);
        self
    }

    pub fn trailing(mut self, trailing: impl IntoElement) -> Self {
        self.trailing.push(trailing.into_any_element());
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
        let color = look.color(if self.disabled {
            Tone::Muted
        } else if self.destructive {
            Tone::Bad
        } else {
            Tone::Ink
        });
        let surface = look.palette.surface.hsla();
        let tick = look.points(TypeRole::Control);
        let on_click = self.on_click;
        let chosen = self.rich && self.checked == Some(true);
        let muted = look.palette.muted.hsla();
        let accent = look.color(Tone::Accent);
        let (label, detail, reach) = (self.label, self.detail, self.reach);
        let name = div()
            .flex()
            .items_center()
            .gap(look.space(Space::Xs))
            .child(label)
            .children(reach.map(|icon| KoanIcon::new(icon).role(TypeRole::Fine).color(muted)));
        let text = match detail {
            Some((detail, warns)) => div()
                .relative()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(name)
                .child(
                    div()
                        .text_role(TypeRole::Fine, &look)
                        .text_color(if warns {
                            look.palette.bad.hsla()
                        } else {
                            muted
                        })
                        .child(detail),
                ),
            None => div().relative().flex_1().child(name),
        };
        let rich = self.rich;
        let row = div()
            .id(self.id)
            .relative()
            .flex()
            .items_center()
            .gap(look.space(if rich { Space::M } else { Space::S }))
            .px(look.space(Space::M))
            .py(look.space(if rich { Space::S } else { Space::Xs }))
            .text_role(
                if rich {
                    TypeRole::Body
                } else {
                    TypeRole::Control
                },
                &look,
            )
            .text_color(color)
            .whitespace_nowrap()
            .when((self.highlighted || chosen) && !self.disabled, |row| {
                row.bg(surface)
            })
            .when(!self.disabled, |row| row.hover(move |row| row.bg(surface)))
            .children(self.icon.filter(|_| look.icons || rich).map(|icon| {
                if rich {
                    // The place's glyph, at the size of a small title, in a
                    // column so the names line up.
                    div()
                        .relative()
                        .flex_none()
                        .w(look.spaced(28.0))
                        .flex()
                        .justify_center()
                        .child(
                            KoanIcon::new(icon)
                                .role(TypeRole::TitleSmall)
                                .color(if chosen { accent } else { muted }),
                        )
                } else {
                    div()
                        .relative()
                        .child(KoanIcon::new(icon).role(TypeRole::Control).color(color))
                }
            }))
            .child(text)
            .children(
                self.trailing
                    .into_iter()
                    .map(|t| div().relative().flex_none().child(t)),
            )
            .children(self.keys.map(|keys| {
                div()
                    .relative()
                    .flex_none()
                    .ml(look.space(Space::L))
                    .text_color(muted)
                    .child(keys)
            }))
            // The tick at the row's end, after `m`, as the Mac's menus put
            // it; its room is kept on every row of a set of choices.
            .children(self.checked.map(|checked| {
                div()
                    .relative()
                    .flex_none()
                    .ml(look.space(Space::M))
                    .size(gpui_kit::px(crate::theme::icons::side(tick)))
                    .when(checked, |cell| {
                        cell.child(KoanIcon::new(Icon::Check).points(tick).color(color))
                    })
            }))
            .when(self.opens, |row| {
                row.child(
                    div()
                        .relative()
                        .flex_none()
                        .ml(look.space(Space::M))
                        .child(KoanIcon::new(Icon::Disclosure).points(tick).color(color)),
                )
            });
        let on_click = on_click.filter(|_| !self.disabled);
        row.when_some(on_click, |row, on_click| {
            row.cursor_pointer()
                .on_click(move |event, window, cx| on_click(event, window, cx))
        })
    }
}

/// The rule between a menu's groups: a hairline of `rule` across the panel,
/// `xs` above and below.
#[derive(IntoElement)]
pub struct MenuSeparator;

impl RenderOnce for MenuSeparator {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        div()
            .h(HAIRLINE)
            .my(look.space(Space::Xs))
            .bg(look.palette.rule.hsla())
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
