use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;

use gpui::{
    App, ClickEvent, ElementId, InteractiveElement, IntoElement, ParentElement, Pixels, RenderOnce,
    SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};

use super::{
    Handler, accessible, answers_click, child, fade, fill, focus, pointer, ring, track, transition,
};
use crate::theme::{
    DISABLED_OPACITY, HIT, Icon, KoanIcon, KoanStyled, Look, Space, TOOLBAR_BUTTON, Tone, TypeRole,
    label,
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
    /// A glyph in a 36-point cell, no ground, answering in colour alone:
    /// `ink`; `strong` under the pointer; the accent pressed. Selected it
    /// rests at `strong` and the pointer takes it to the accent.
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

    /// How a press is answered: a ring off a glyph, the ground lifting in
    /// an outlined button, nothing for the kinds that read as words.
    fn answer(self) -> Option<super::Answer> {
        match self {
            ButtonKind::Icon | ButtonKind::IconOutlined | ButtonKind::Toolbar => {
                Some(super::Answer::Ring)
            }
            ButtonKind::Prominent | ButtonKind::Bordered => Some(super::Answer::Lift),
            ButtonKind::Standard | ButtonKind::Compact | ButtonKind::Link | ButtonKind::Text => {
                None
            }
        }
    }

    /// The side a glyph-only button of this kind draws at: its glyph, its
    /// padding either side and its outline, where it has one. What a layout
    /// around the button measures from, rather than copying the parts.
    pub fn glyph_side(self, look: &Look) -> Pixels {
        let glyph = px(crate::theme::icons::side(look.points(self.role())));
        let (vertical, _) = self.padding();
        let outline = match self {
            ButtonKind::Prominent | ButtonKind::Bordered | ButtonKind::IconOutlined => {
                look.hairline
            }
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
        let answers = super::Answerer::of(&self.id, window, cx);
        let tone = if self.destructive {
            Tone::Bad
        } else {
            self.tone.unwrap_or_else(|| kind.tone())
        };
        let rest = look.text_color(tone);
        let ink = look.palette.ink.hsla();
        // Text colour fades through a colour slot, for the kinds whose text
        // changes; fills are shader quads (`transition`). Both move on the
        // GPU.
        let color = match kind {
            ButtonKind::Text => fade(child(&self.id, "hover"), hovered, rest, ink, window, cx),
            ButtonKind::Link => fade(
                child(&self.id, "press"),
                pressed || self.selected,
                rest,
                ink,
                window,
                cx,
            ),
            ButtonKind::Toolbar => {
                let (base, over) = if self.selected {
                    (look.palette.strong.hsla(), look.color(Tone::Accent))
                } else {
                    (rest, look.palette.strong.hsla())
                };
                let hover = fade(child(&self.id, "hover"), hovered, base, over, window, cx);
                fade(
                    child(&self.id, "press"),
                    pressed,
                    hover,
                    look.color(Tone::Accent),
                    window,
                    cx,
                )
            }
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
        // A press is answered by a ring off a glyph alone; a button showing
        // its word is lifted instead, under the word, never ringed through it.
        let worded = !glyph_only && self.label.is_some();
        let answer = match kind.answer() {
            Some(super::Answer::Ring) if worded => Some(super::Answer::Lift),
            answer => answer,
        };
        debug_assert!(
            !glyph_only || self.help.is_some() || self.label.is_some(),
            "a glyph-only button says what it does in a tooltip: make it with Button::icon_only"
        );
        let role = kind.role();
        // Play and pause fold into each other as the state they show moves,
        // whoever moved it; the glyph is drawn by the program meanwhile.
        // Only a glyph alone, a toggle's face, or copy turning to its tick
        // (a copy button is labelled): a labelled button with a transport
        // glyph (a "Play" header button) shows no state to move.
        let morphing = self.icon.is_some_and(|icon| {
            super::answer::morphs(icon) && (glyph_only || matches!(icon, Icon::Copy | Icon::Check))
        }) && show_glyph;
        if let Some(icon) = self.icon.filter(|_| morphing) {
            let mark = super::answer::glyph_mark(icon, None);
            if let Some(answer) = answers
                .seen(cx)
                .and_then(|from| super::answer::glyph_motion(from, mark))
            {
                answers.follow(mark, answer, look.accent.hsla(), window, cx);
            } else {
                answers.note(mark, cx);
            }
        }

        // Named by its label, or a glyph's by what its tooltip says it does.
        let name = self.label.clone().or_else(|| self.help.clone());
        let mut button = accessible(div().id(self.id.clone()), gpui::Role::Button, name)
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
                let (answers, accent) = (answers.clone(), look.accent.hsla());
                #[allow(clippy::arc_with_non_send_sync)] // handlers run on the main thread only
                let press: Handler<ClickEvent> = Arc::new(move |event, window, cx| {
                    // A glyph that morphs answers as its state lands.
                    if let Some(answer) = answer.filter(|_| !morphing) {
                        answers.start(answer, accent, cx);
                    }
                    on_click(event, window, cx)
                });
                button = answers_click(button, press.clone());
                button = button.on_click(move |event, window, cx| press(event, window, cx));
            }
        }

        // Fills: the outlined kinds' pressed `hover`. The toolbar button has
        // none; its glyph changes colour instead.
        match kind {
            ButtonKind::Prominent | ButtonKind::IconOutlined => {
                let press = transition(
                    child(&self.id, "press-fill"),
                    pressed || self.selected,
                    window,
                    cx,
                );
                button = button.child(fill(look.palette.hover.hsla(), &press));
            }
            _ => {}
        }

        let hidden = answers.glyph_live(cx);
        let drawn = answers.glyph_layer(
            px(crate::theme::icons::side(look.points(role))),
            color,
            None,
            window,
            cx,
        );
        let glyph = self.icon.filter(|_| show_glyph).map(|icon| {
            div()
                .relative()
                .child(
                    div()
                        .when(hidden, |d| d.invisible())
                        .child(KoanIcon::new(icon).role(role).color(color)),
                )
                .children(drawn)
        });
        let text = self.label.clone().filter(|_| !glyph_only);
        let underline = kind == ButtonKind::Link;
        // A ring leaves a glyph alone, not the hit area around it; a lift
        // fills the button under its content, over its fills.
        let ring_answer = answer == Some(super::Answer::Ring);
        let answered = answers.layer(window, cx);
        let (inner_answer, outer_answer) = if ring_answer {
            (answered, None)
        } else {
            (None, answered)
        };
        button = button.children(outer_answer).child(
            div()
                .relative()
                .flex()
                .items_center()
                .gap(look.space(Space::Xs))
                .children(inner_answer)
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

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui::accesskit::Action;
    use gpui::{IntoElement, TestAppContext, VisualTestContext};

    use super::*;
    use crate::components::testing::{act, click, node, press, stage};

    /// A labelled button, a glyph named by its tooltip, and a disabled one,
    /// each counting the clicks it is given.
    fn buttons(cx: &mut TestAppContext, clicks: Rc<Cell<[u32; 3]>>) -> &mut VisualTestContext {
        let (_, cx) = stage(cx, move |_, _| {
            let count = |at: usize| {
                let clicks = clicks.clone();
                move |_: &ClickEvent, _: &mut Window, _: &mut App| {
                    let mut n = clicks.get();
                    n[at] += 1;
                    clicks.set(n);
                }
            };
            vec![
                (
                    "queue",
                    Button::new("queue", ButtonKind::Compact)
                        .label("queue")
                        .on_click(count(0))
                        .into_any_element(),
                ),
                (
                    "forget",
                    Button::icon_only("forget", Icon::Clear, "forget this device")
                        .on_click(count(1))
                        .into_any_element(),
                ),
                (
                    "remove",
                    Button::new("remove", ButtonKind::Compact)
                        .label("remove")
                        .disabled(true)
                        .on_click(count(2))
                        .into_any_element(),
                ),
            ]
        });
        cx
    }

    #[gpui::test]
    fn a_click_reaches_its_own_button(cx: &mut TestAppContext) {
        let clicks = Rc::new(Cell::new([0; 3]));
        let cx = buttons(cx, clicks.clone());
        click(cx, "queue");
        assert_eq!(clicks.get(), [1, 0, 0]);
        click(cx, "forget");
        assert_eq!(clicks.get(), [1, 1, 0]);
    }

    #[gpui::test]
    fn enter_and_space_click_the_focused_button(cx: &mut TestAppContext) {
        let clicks = Rc::new(Cell::new([0; 3]));
        let cx = buttons(cx, clicks.clone());
        cx.update(|window, cx| window.focus_next(cx));
        press(cx, "enter");
        press(cx, "space");
        assert_eq!(clicks.get(), [2, 0, 0]);
    }

    #[gpui::test]
    fn a_disabled_button_refuses_the_pointer_the_keys_and_a_screen_reader(cx: &mut TestAppContext) {
        let clicks = Rc::new(Cell::new([0; 3]));
        let cx = buttons(cx, clicks.clone());
        click(cx, "remove");
        act(cx, "remove", Action::Click);
        // Tab order skips it: from the last enabled button, focus wraps.
        cx.update(|window, cx| {
            window.focus_next(cx);
            window.focus_next(cx);
            window.focus_next(cx);
        });
        press(cx, "enter");
        assert_eq!(clicks.get()[2], 0, "a disabled button was clicked");
        assert_eq!(node(cx, "remove").1["disabled"], true);
    }

    #[gpui::test]
    fn buttons_are_named_by_their_label_or_their_tooltip(cx: &mut TestAppContext) {
        let clicks = Rc::new(Cell::new([0; 3]));
        let cx = buttons(cx, clicks.clone());
        for name in ["queue", "forget this device", "remove"] {
            assert_eq!(node(cx, name).1["role"], "Button", "{name}");
        }
        assert!(node(cx, "queue").1.get("disabled").is_none());
        act(cx, "forget this device", Action::Click);
        assert_eq!(clicks.get(), [0, 1, 0]);
    }
}
