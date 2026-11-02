use std::sync::Arc;

use gpui_kit::{
    App, ClickEvent, ElementId, InteractiveElement, IntoElement, ParentElement, Pixels, RenderOnce,
    SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};

use super::{
    Handler, accessible, answers_click, child, fade, fill, focus, mix, pointer, ring, track,
    transition,
};
use crate::theme::{
    DISABLED_OPACITY, HAIRLINE, HIT, Icon, KoanIcon, KoanStyled, Look, Motion, Space,
    TOOLBAR_BUTTON, Tone, TypeRole, label,
};

/// A button's weight and shape. The rules for which a screen uses are the
/// spec's: one prominent action at most, compact for repeated and secondary
/// controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonKind {
    /// The accent outline: what the screen is for.
    Prominent,
    /// The label in `ink`, no outline.
    Standard,
    /// As standard, smaller and tighter.
    Compact,
    /// Compact, in a `muted` outline: an action standing in a form row.
    Bordered,
    /// Underlined `muted` text: anything that goes somewhere.
    Link,
    /// `muted` text in a bar, `ink` under the pointer.
    Text,
    /// A transport glyph, no outline, a 44-point target.
    Icon,
    /// Play/pause: a transport glyph in a square `ink` outline.
    IconOutlined,
    /// A glyph in a 36-point cell, `surface` under the pointer.
    Toolbar,
}

impl ButtonKind {
    fn role(self) -> TypeRole {
        match self {
            ButtonKind::Prominent | ButtonKind::Icon | ButtonKind::IconOutlined => TypeRole::Body,
            ButtonKind::Standard | ButtonKind::Link | ButtonKind::Text | ButtonKind::Toolbar => {
                TypeRole::Control
            }
            ButtonKind::Compact | ButtonKind::Bordered => TypeRole::Meta,
        }
    }

    /// Padding, vertical then horizontal, in points before spacing's scale.
    fn padding(self) -> (f32, f32) {
        match self {
            ButtonKind::Prominent => (12.0, 20.0),
            ButtonKind::Standard => (8.0, 4.0),
            ButtonKind::Compact => (4.0, 2.0),
            ButtonKind::Bordered => (6.0, 12.0),
            ButtonKind::Link | ButtonKind::Text => (4.0, 0.0),
            ButtonKind::IconOutlined => (7.0, 7.0),
            ButtonKind::Icon | ButtonKind::Toolbar => (0.0, 0.0),
        }
    }

    /// The side a glyph-only button of this kind draws at: its glyph, its
    /// padding either side and its outline, where it has one. What a layout
    /// around the button measures from, rather than copying the parts.
    pub fn glyph_side(self, look: &Look) -> Pixels {
        let glyph = px(crate::theme::icons::side(look.points(self.role())));
        let (vertical, _) = self.padding();
        let outline = match self {
            ButtonKind::Prominent | ButtonKind::Bordered | ButtonKind::IconOutlined => look.hairline,
            _ => px(0.0),
        };
        glyph + look.spaced(vertical) * 2.0 + outline * 2.0
    }

    fn tone(self) -> Tone {
        match self {
            ButtonKind::Prominent => Tone::Accent,
            ButtonKind::Link | ButtonKind::Text => Tone::Muted,
            _ => Tone::Ink,
        }
    }
}

#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    kind: ButtonKind,
    label: Option<SharedString>,
    icon: Option<Icon>,
    tone: Option<Tone>,
    destructive: bool,
    disabled: bool,
    selected: bool,
    /// What the tooltip says in place of the label: what it does, and the
    /// keys that do the same.
    help: Option<SharedString>,
    on_click: Option<Handler<ClickEvent>>,
}

impl Button {
    pub fn new(id: impl Into<ElementId>, kind: ButtonKind) -> Self {
        Self {
            id: id.into(),
            kind,
            label: None,
            icon: None,
            tone: None,
            destructive: false,
            disabled: false,
            selected: false,
            help: None,
            on_click: None,
        }
    }

    /// A glyph alone, which says what it does only in its tooltip: `help`
    /// names the action, with its keys where it has them ("back (⌘[)"). The
    /// way to make one, so none goes without.
    pub fn icon_only(id: impl Into<ElementId>, icon: Icon, help: impl Into<SharedString>) -> Self {
        Self::new(id, ButtonKind::Icon).icon(icon).help(help)
    }

    /// `icon_only`, outlined: the transport's play button.
    pub fn icon_outlined(
        id: impl Into<ElementId>,
        icon: Icon,
        help: impl Into<SharedString>,
    ) -> Self {
        Self::new(id, ButtonKind::IconOutlined)
            .icon(icon)
            .help(help)
    }

    /// The tooltip, in place of a glyph-only button's label, and on a button
    /// with words too: "back (⌘[)", as the Mac's buttons say.
    pub fn help(mut self, help: impl Into<SharedString>) -> Self {
        self.help = Some(help.into());
        self
    }

    /// The label, an app word, drawn lowercase. A glyph-only button keeps it
    /// for its tooltip.
    pub fn label(mut self, text: impl AsRef<str>) -> Self {
        self.label = Some(label(text));
        self
    }

    /// A label that holds data, a device's or a profile's name, which keeps
    /// its case: the caller lowercases the app's own words in it.
    pub fn text(mut self, text: impl Into<SharedString>) -> Self {
        self.label = Some(text.into());
        self
    }

    /// The glyph: beside the label with "show icons" on, or alone in a glyph
    /// button whatever the setting.
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Its label and glyph in `tone` rather than the kind's: a favourite in
    /// the accent while it is on.
    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = Some(tone);
        self
    }

    /// In `bad`, for a compact or bordered button that destroys something.
    pub fn destructive(mut self, destructive: bool) -> Self {
        self.destructive = destructive;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// A toolbar toggle's on state (the lyrics, the sidebar): the pressed fill.
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

impl RenderOnce for Button {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let kind = self.kind;
        let state = pointer(&self.id, window, cx);
        let (hovered, pressed) = {
            let p = state.read(cx);
            (p.hovered && !self.disabled, p.pressed && !self.disabled)
        };
        let handle = focus(&self.id, window, cx);
        // Text colour fades on the CPU, for the kinds whose text changes;
        // fills are the GPU's (`transition`).
        let hover = if kind == ButtonKind::Text {
            fade(child(&self.id, "hover"), hovered, Motion::FAST, window, cx)
        } else {
            0.0
        };
        let press = if kind == ButtonKind::Link {
            fade(
                child(&self.id, "press"),
                pressed || self.selected,
                Motion::FAST,
                window,
                cx,
            )
        } else {
            0.0
        };

        let tone = if self.destructive {
            Tone::Bad
        } else {
            self.tone.unwrap_or_else(|| kind.tone())
        };
        let rest = look.text_color(tone);
        let color = match kind {
            ButtonKind::Text => mix(rest, look.palette.ink.hsla(), hover),
            ButtonKind::Link => mix(rest, look.palette.ink.hsla(), press),
            _ => rest,
        };
        let outline = match kind {
            ButtonKind::Prominent => Some(look.color(Tone::Accent)),
            ButtonKind::Bordered if self.destructive => Some(look.palette.bad.hsla()),
            ButtonKind::Bordered => Some(look.palette.muted.hsla()),
            ButtonKind::IconOutlined => Some(look.palette.ink.hsla()),
            _ => None,
        };
        let (vertical, horizontal) = kind.padding();
        // A toolbar button with icons off is its word in the same cell.
        let glyph_only = matches!(kind, ButtonKind::Icon | ButtonKind::IconOutlined)
            || (kind == ButtonKind::Toolbar && look.icons && self.icon.is_some());
        let show_glyph = glyph_only || look.icons;
        debug_assert!(
            !glyph_only || self.help.is_some() || self.label.is_some(),
            "a glyph-only button says what it does in a tooltip: make it with Button::icon_only"
        );
        let role = kind.role();

        // Named by its label, or a glyph's by what its tooltip says it does.
        let name = self.label.clone().or_else(|| self.help.clone());
        let mut button = accessible(div().id(self.id.clone()), gpui_kit::Role::Button, name)
            .aria_disabled(self.disabled)
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .gap(look.space(Space::Xs))
            .py(look.spaced(vertical))
            .px(look.spaced(horizontal))
            .text_role(role, &look)
            .text_color(color);
        button = match kind {
            ButtonKind::Icon => button.min_w(HIT).min_h(HIT),
            ButtonKind::Toolbar => button.min_w(TOOLBAR_BUTTON).h(TOOLBAR_BUTTON),
            _ => button,
        };
        if let Some(outline) = outline {
            button = button.border(look.hairline).border_color(outline);
        }
        if self.disabled {
            button = button.opacity(DISABLED_OPACITY);
        } else {
            // GPUI turns Enter and Space on the focused button into a click.
            button = track(button, &state).track_focus(&handle).cursor_pointer();
            if let Some(on_click) = self.on_click.clone() {
                button = answers_click(button, on_click.clone());
                button = button.on_click(move |event, window, cx| on_click(event, window, cx));
            }
        }

        // Fills: the prominent button's pressed `hover`, the toolbar's
        // `surface` under the pointer and `hover` pressed.
        match kind {
            ButtonKind::Prominent | ButtonKind::IconOutlined => {
                let press = transition(
                    child(&self.id, "press-fill"),
                    pressed || self.selected,
                    Motion::FAST,
                    window,
                    cx,
                );
                button = button.child(fill(look.palette.hover.hsla(), &press));
            }
            ButtonKind::Toolbar => {
                let hover = transition(
                    child(&self.id, "hover-fill"),
                    hovered,
                    Motion::FAST,
                    window,
                    cx,
                );
                let press = transition(
                    child(&self.id, "press-fill"),
                    pressed || self.selected,
                    Motion::FAST,
                    window,
                    cx,
                );
                button = button
                    .child(fill(look.palette.surface.hsla(), &hover))
                    .child(fill(look.palette.hover.hsla(), &press));
            }
            _ => {}
        }

        let glyph = self
            .icon
            .filter(|_| show_glyph)
            .map(|icon| KoanIcon::new(icon).role(role).color(color));
        let text = self.label.clone().filter(|_| !glyph_only);
        let underline = kind == ButtonKind::Link;
        button = button.child(
            div()
                .relative()
                .flex()
                .items_center()
                .gap(look.space(Space::Xs))
                .children(glyph)
                .children(text.map(|text| {
                    let line = div().whitespace_nowrap().child(text);
                    if underline {
                        line.underline().text_decoration_color(color)
                    } else {
                        line
                    }
                })),
        );
        let tooltip = self.help.or(self.label.filter(|_| glyph_only));
        if let Some(tooltip) = tooltip {
            button =
                button.tooltip(move |window, cx| super::Tooltip::view(tooltip.clone(), window, cx));
        }
        button.children(ring(&handle, &look, window))
    }
}
