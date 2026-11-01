//! The toolbar: its items placed leading, in the middle or trailing, as
//! SwiftUI's `ToolbarItem` placements are, at the toolbar's height and gaps;
//! and the toolbar button, a glyph in a 36-point cell.

use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{Button, ButtonKind};
use crate::theme::{HAIRLINE, Icon, KoanStyled, KoanTheme, Look};

/// The toolbar's figures, the Mac's unified toolbar measured at 2x: 52
/// tall, its buttons 36-point cells. Gaps are fixed points, as AppKit's.
pub struct ToolbarMetrics;

impl ToolbarMetrics {
    pub const HEIGHT: Pixels = px(TOOLBAR_HEIGHT);
    /// A toolbar button's cell.
    pub const BUTTON: Pixels = px(36.);
    /// From the toolbar's edges to its first and last cells.
    pub const INSET: Pixels = px(8.);
    /// Between separate items: sidebar, back and forward; select and lyrics.
    pub const ITEM_GAP: Pixels = px(8.);
    /// Between the buttons of one group: a page's filters and sort.
    pub const GROUP_GAP: Pixels = px(2.);
    /// The fixed spacer between groups: the filter field and the buttons
    /// after it, the buttons and the selection's controls.
    pub const SPACER: Pixels = px(16.);
    /// The filter field, from its glyph to the end of its text.
    pub const FIELD: Pixels = px(167.);
    /// Between the filter field's glyph and its text.
    pub const FIELD_GLYPH_GAP: Pixels = px(10.);
    /// The window's own buttons (the Mac's traffic lights), 14 tall,
    /// centred on the toolbar.
    pub const WINDOW_BUTTONS_ORIGIN: Point<Pixels> = Point {
        x: px(20.),
        y: px((TOOLBAR_HEIGHT - 14.) / 2.),
    };
}

/// The toolbar's height, in points.
pub const TOOLBAR_HEIGHT: f32 = 52.0;

/// The room the window's own buttons take where they sit over the app (the
/// Mac's traffic lights, at the toolbar's height); none where the window
/// has a title bar or the toolbar draws them.
#[cfg(target_os = "macos")]
pub const WINDOW_BUTTONS: Size<Pixels> = Size {
    width: px(86.),
    height: px(TOOLBAR_HEIGHT),
};
#[cfg(not(target_os = "macos"))]
pub const WINDOW_BUTTONS: Size<Pixels> = Size {
    width: px(0.),
    height: px(0.),
};

#[derive(IntoElement, Default)]
pub struct Toolbar {
    inset: Pixels,
    leading: Vec<AnyElement>,
    principal: Option<AnyElement>,
    trailing: Vec<AnyElement>,
}

impl Toolbar {
    pub fn new() -> Self {
        Self::default()
    }

    /// The window's own buttons sit over the toolbar's leading edge (the
    /// sidebar they otherwise sit over is hidden): it leaves them their room.
    pub fn under_window_buttons(mut self, under: bool) -> Self {
        self.inset = if under { WINDOW_BUTTONS.width } else { px(0.) };
        self
    }

    /// At the leading edge: back and forward, the sidebar's toggle.
    pub fn leading(mut self, item: impl IntoElement) -> Self {
        self.leading.push(item.into_any_element());
        self
    }

    /// Filling the middle: the page's title, or bare space that moves the
    /// window.
    pub fn principal(mut self, item: impl IntoElement) -> Self {
        self.principal = Some(item.into_any_element());
        self
    }

    /// At the trailing edge: the lyrics toggle, the window's own buttons.
    pub fn trailing(mut self, item: impl IntoElement) -> Self {
        self.trailing.push(item.into_any_element());
        self
    }
}

impl RenderOnce for Toolbar {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        div()
            .h(ToolbarMetrics::HEIGHT)
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .pl(self.inset + ToolbarMetrics::INSET)
            .pr(ToolbarMetrics::INSET)
            .gap(ToolbarMetrics::ITEM_GAP)
            // With the wash under the whole window the toolbar has no
            // ground and meets the page at a hairline; otherwise it is on
            // `bg`, flush with the page.
            .map(|d| {
                if cx.global::<KoanTheme>().wash_window {
                    d.border_b(HAIRLINE).border_color(look.palette.rule.hsla())
                } else {
                    d.bg(look.palette.bg.hsla())
                }
            })
            .children(self.leading)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .items_center()
                    .children(self.principal),
            )
            .children(self.trailing)
    }
}

/// What a page puts in the toolbar, in groups as the Mac's toolbar items:
/// the filter field, then its buttons a fixed spacer after it, 2 points
/// apart, then the selection's controls a spacer further on. Pages declare
/// the groups; the spacing is the toolbar's.
#[derive(IntoElement, Default)]
pub struct ToolbarGroup {
    groups: Vec<Vec<AnyElement>>,
}

impl ToolbarGroup {
    pub fn new() -> Self {
        Self {
            groups: vec![Vec::new()],
        }
    }

    /// Starts a new group, a fixed spacer after the last.
    pub fn spacer(mut self) -> Self {
        if self.groups.last().is_some_and(|g| !g.is_empty()) {
            self.groups.push(Vec::new());
        }
        self
    }
}

impl ParentElement for ToolbarGroup {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        if self.groups.is_empty() {
            self.groups.push(Vec::new());
        }
        self.groups.last_mut().unwrap().extend(elements);
    }
}

impl RenderOnce for ToolbarGroup {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(ToolbarMetrics::SPACER)
            .children(
                self.groups
                    .into_iter()
                    .filter(|g| !g.is_empty())
                    .map(|group| {
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(ToolbarMetrics::GROUP_GAP)
                            .children(group)
                    }),
            )
    }
}

/// A toolbar's button: the glyph in `ink` in a 36-point cell, `surface`
/// under the pointer and `hover` pressed; the word in its place with icons
/// off.
#[derive(IntoElement)]
pub struct ToolbarButton {
    button: Button,
    selected: bool,
}

impl ToolbarButton {
    /// `label` names it to VoiceOver, and is drawn with icons off.
    pub fn new(id: impl Into<ElementId>, icon: Icon, label: impl AsRef<str>) -> Self {
        Self {
            button: Button::new(id, ButtonKind::Toolbar).icon(icon).label(label),
            selected: false,
        }
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.button = self.button.disabled(disabled);
        self
    }

    /// Drawn pressed: a toggle that is on, or a menu or panel it opened
    /// that is showing.
    pub fn selected(mut self, selected: bool) -> Self {
        self.button = self.button.selected(selected);
        self.selected = selected;
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.button = self.button.on_click(handler);
        self
    }
}

/// A popover's trigger: drawn pressed while what it opened shows.
impl gpui_kit::base::Selectable for ToolbarButton {
    fn selected(self, selected: bool) -> Self {
        ToolbarButton::selected(self, selected)
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl RenderOnce for ToolbarButton {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        self.button
    }
}

/// Closes the menu a `ToolbarMenu` opened: what a row runs before its action.
pub type Dismiss = std::rc::Rc<dyn Fn(&mut Window, &mut App)>;

type Content = std::rc::Rc<dyn Fn(Dismiss, &mut Window, &mut App) -> super::MenuPanel>;

/// A toolbar button that opens a menu panel under it: a page's ⋯, a sort.
/// Drawn pressed while the menu shows.
#[derive(IntoElement)]
pub struct ToolbarMenu {
    id: ElementId,
    button: ToolbarButton,
    content: Content,
}

impl ToolbarMenu {
    /// `content` builds the panel each time it opens; its rows call the
    /// `Dismiss` they are given before they act.
    pub fn new(
        id: impl Into<ElementId>,
        button: ToolbarButton,
        content: impl Fn(Dismiss, &mut Window, &mut App) -> super::MenuPanel + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            button,
            content: std::rc::Rc::new(content),
        }
    }
}

impl RenderOnce for ToolbarMenu {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let content = self.content;
        gpui_kit::component::popover::Popover::new(self.id)
            .appearance(false)
            .arrow(false)
            .trigger(self.button)
            .content(move |_, window, cx| {
                let state = cx.weak_entity();
                let dismiss: Dismiss = std::rc::Rc::new(move |window, cx| {
                    state.update(cx, |s, cx| s.dismiss(window, cx)).ok();
                });
                content(dismiss, window, cx)
            })
    }
}

/// What a toolbar says while things are picked, the Mac's selection bar: how
/// many, in `muted`, then what can be done to them as text buttons in the
/// accent, the toolbar's group spacing between each.
#[derive(IntoElement)]
pub struct ToolbarSelection {
    count: SharedString,
    actions: Vec<AnyElement>,
}

impl ToolbarSelection {
    /// `count` says how many: "3 selected".
    pub fn new(count: impl Into<SharedString>) -> Self {
        Self {
            count: count.into(),
            actions: Vec::new(),
        }
    }

    /// `label` is lowercased with the theme's other labels.
    pub fn action(
        mut self,
        id: impl Into<ElementId>,
        label: impl AsRef<str>,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.actions.push(
            Button::new(id, ButtonKind::Text)
                .label(label)
                .tone(crate::theme::Tone::Accent)
                .on_click(on_click)
                .into_any_element(),
        );
        self
    }
}

impl RenderOnce for ToolbarSelection {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(ToolbarMetrics::SPACER)
            .child(
                div()
                    .text_role(crate::theme::TypeRole::Control, &look)
                    .text_color(look.palette.muted.hsla())
                    .whitespace_nowrap()
                    .child(self.count),
            )
            .children(self.actions)
    }
}
