//! kōan's text fields: `TextField` for a line, `TextArea` for several,
//! both drawing a `FieldState` the page owns.
//!
//! The editing is the platform's: an input method composes in place
//! (GPUI's `EntityInputHandler`), the selection takes clicks, drags, Shift
//! with the arrows and double and triple clicks, words and lines move as
//! the Mac's fields move them, the clipboard and undo work, a line too long
//! for the field scrolls to keep the caret in view, and a screen reader
//! hears a text field with its value and selection. The look is kōan's:
//! the type and ground of a `control`, no bezel and no focus ring (the
//! caret says where typing goes), the selection the accent at 30 %, the
//! placeholder in `muted`, and the caret a shader quad whose blink draws
//! nothing.

mod actions;
mod element;
mod state;
#[cfg(test)]
mod tests;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    App, CursorStyle, Entity, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    SharedString, StatefulInteractiveElement, Styled, Window, div,
};

pub use actions::{
    Backspace, CONTEXT, Copy, Cut, Delete, DeleteToLineEnd, DeleteToLineStart, DeleteWordLeft,
    DeleteWordRight, Enter, Escape, MoveDown, MoveEnd, MoveHome, MoveLeft, MoveRight, MoveToEnd,
    MoveToStart, MoveUp, MoveWordLeft, MoveWordRight, Paste, Redo, SecondaryEnter, SelectAll,
    SelectDown, SelectEnd, SelectHome, SelectLeft, SelectRight, SelectToEnd, SelectToStart,
    SelectUp, SelectWordLeft, SelectWordRight, ShiftEnter, ShowCharacterPalette, Undo, bind,
};
pub use state::{FieldState, InputEvent};

use element::{Colours, FieldElement};

use crate::theme::{KoanStyled, Look, Space, TypeRole};

/// What a `TextArea` edits: a `FieldState` made with `.rows(n)`.
pub type AreaState = FieldState;

/// A text field: `control` type in `ink` on a square `surface` field with no
/// bezel, or on nothing at all without `ground`, as the sidebar's search is.
/// As on the Mac, a field draws no focus ring: the caret shows where typing
/// goes, and the field answers taking the keyboard with a line drawn under
/// it. The owner makes the state
/// (`cx.new(|cx| FieldState::new(window, cx))`), which carries the
/// placeholder, drawn in `muted`.
#[derive(IntoElement)]
pub struct TextField {
    state: Entity<FieldState>,
    ground: bool,
    disabled: bool,
    role: TypeRole,
    label: Option<SharedString>,
}

impl TextField {
    pub fn new(state: &Entity<FieldState>) -> Self {
        Self {
            state: state.clone(),
            ground: true,
            disabled: false,
            role: TypeRole::Control,
            label: None,
        }
    }

    /// What a screen reader names it, where no visible label beside it
    /// does: "search", "name". The placeholder is read as a hint after it.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Not editable, and faded: while what it feeds is under way.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Its text in `role` rather than `control`: a search box's large
    /// query.
    pub fn role(mut self, role: TypeRole) -> Self {
        self.role = role;
        self
    }

    /// No field of its own, on the page's ground.
    pub fn bare(mut self) -> Self {
        self.ground = false;
        self
    }
}

impl RenderOnce for TextField {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let id = gpui_kit::ElementId::from(("koan-field", self.state.entity_id().as_u64()));
        let answers = super::Answerer::of(&id, window, cx);
        let focused = self.state.read(cx).focus.is_focused(window);
        // Taking the keyboard is answered by a line drawn under the field,
        // once each time.
        let was = window.use_keyed_state(super::child(&id, "focused"), cx, |_, _| focused);
        if *was.read(cx) != focused {
            was.update(cx, |was, _| *was = focused);
            if focused {
                answers.start(super::Answer::Sweep, look.accent.hsla(), cx);
            }
        }
        div()
            .relative()
            .flex()
            .items_center()
            .min_w_0()
            .when(self.ground, |field| {
                field
                    .px(look.space(Space::M))
                    .py(look.space(Space::S))
                    .bg(look.palette.surface.hsla())
            })
            .when(self.disabled, |field| {
                field.opacity(crate::theme::DISABLED_OPACITY)
            })
            .child(
                editor(&self.state, &id, self.label, !self.disabled, window, cx)
                    .h(look.line_height(self.role))
                    .text_role(self.role, &look)
                    .text_color(look.palette.ink.hsla()),
            )
            .children(answers.layer(window, cx))
    }
}

/// Several lines of text pasted or typed (a measurement's rows): `control`
/// type in `ink` on a square `surface` ground, as a `TextField`, the state
/// made with `.rows(n)` for its height in lines.
#[derive(IntoElement)]
pub struct TextArea {
    state: Entity<AreaState>,
    label: Option<SharedString>,
}

impl TextArea {
    pub fn new(state: &Entity<AreaState>) -> Self {
        Self {
            state: state.clone(),
            label: None,
        }
    }

    /// What a screen reader names it, as `TextField::label`.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }
}

impl RenderOnce for TextArea {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let id = gpui_kit::ElementId::from(("koan-area", self.state.entity_id().as_u64()));
        div()
            .p(look.space(Space::S))
            .bg(look.palette.surface.hsla())
            .child(
                editor(&self.state, &id, self.label, true, window, cx)
                    .text_role(TypeRole::Control, &look)
                    .text_color(look.palette.ink.hsla()),
            )
    }
}

/// The text itself, with the keys, the pointer, the input method and the
/// screen reader wired to `state`: what `TextField` and `TextArea` frame.
/// A disabled field takes nothing; a read-only one takes the pointer, the
/// movement keys and Copy, and no edit.
fn editor(
    state: &Entity<FieldState>,
    id: &gpui_kit::ElementId,
    label: Option<SharedString>,
    enabled: bool,
    window: &mut Window,
    cx: &mut App,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let look = Look::of(window, cx);
    let s = state.read(cx);
    let focus = s.focus.clone();
    let masked = s.masked;
    let multi = s.multi_line();
    let read_only = s.read_only;
    let announce = s.announce;
    let placeholder = s.placeholder.clone();
    let value = s.text.clone();
    let (anchor, active) = s.selection_in_chars();
    let spans = s
        .spans
        .iter()
        .filter(|(range, _)| range.end <= value.len() && range.start < range.end)
        .map(|(range, tone)| (range.clone(), look.text_color(*tone)))
        .collect();
    let a11y = window.is_a11y_active() || cfg!(test);
    let role = if masked {
        gpui_kit::Role::PasswordInput
    } else if multi {
        gpui_kit::Role::MultilineTextInput
    } else {
        gpui_kit::Role::TextInput
    };
    let colours = Colours {
        ink: look.palette.ink.hsla(),
        placeholder: look.palette.muted.hsla(),
        caret: look.accent.hsla(),
        caret_width: look.hairline,
        selection: look.text_selection(),
        newline: look.space(Space::Xs),
        spans,
    };
    let element = FieldElement {
        state: state.clone(),
        editable: enabled && !read_only,
        colours,
    };
    let mut el = div()
        .id(super::child(id, "text"))
        .flex()
        .flex_1()
        .min_w_0()
        .overflow_hidden()
        .key_context(CONTEXT)
        .track_focus(&focus)
        .role(role)
        .when_some(label, |el, label| el.aria_label(label))
        .aria_placeholder(placeholder)
        .aria_disabled(!enabled)
        // A value the owner set is said as it lands; one being typed is
        // heard as it is typed.
        .when(announce, |el| {
            el.aria_live(gpui_kit::accesskit::Live::Polite)
        })
        .when(a11y && !masked, |el| el.aria_value(value.clone()))
        // The text as one run with the selection on it, so a screen reader
        // reads where the caret is and what is selected; and whether it can
        // be changed.
        .a11y_synthetic_children(move |builder| {
            if read_only {
                builder.parent_node().set_read_only();
            }
            if masked || !a11y {
                return;
            }
            let mut run = gpui_kit::accesskit::Node::new(gpui_kit::Role::TextRun);
            run.set_value(value.clone());
            run.set_character_lengths(
                value.chars().map(|c| c.len_utf8() as u8).collect::<Vec<_>>(),
            );
            let id = builder.synthetic_node_id(0);
            builder.push_child(id, run);
            let at = |character_index| gpui_kit::accesskit::TextPosition {
                node: id,
                character_index,
            };
            builder
                .parent_node()
                .set_text_selection(gpui_kit::accesskit::TextSelection {
                    anchor: at(anchor),
                    focus: at(active),
                });
        })
        .on_a11y_action(gpui_kit::AccessibleAction::Focus, {
            let focus = focus.clone();
            move |_, window, cx| window.focus(&focus, cx)
        })
        .child(element);
    if !enabled {
        return el;
    }
    // Reading: the pointer, the movement keys and Copy.
    el = el
        .cursor(CursorStyle::IBeam)
        .on_mouse_down(
            gpui_kit::MouseButton::Left,
            window.listener_for(state, FieldState::mouse_down),
        )
        .on_mouse_move(window.listener_for(state, FieldState::mouse_move))
        .on_mouse_up(
            gpui_kit::MouseButton::Left,
            window.listener_for(state, FieldState::mouse_up),
        )
        .on_mouse_up_out(
            gpui_kit::MouseButton::Left,
            window.listener_for(state, FieldState::mouse_up),
        )
        .on_action(window.listener_for(state, FieldState::left))
        .on_action(window.listener_for(state, FieldState::right))
        .on_action(window.listener_for(state, FieldState::up))
        .on_action(window.listener_for(state, FieldState::down))
        .on_action(window.listener_for(state, FieldState::word_left))
        .on_action(window.listener_for(state, FieldState::word_right))
        .on_action(window.listener_for(state, FieldState::home))
        .on_action(window.listener_for(state, FieldState::end))
        .on_action(window.listener_for(state, FieldState::start))
        .on_action(window.listener_for(state, FieldState::finish))
        .on_action(window.listener_for(state, FieldState::select_left))
        .on_action(window.listener_for(state, FieldState::select_right))
        .on_action(window.listener_for(state, FieldState::select_up))
        .on_action(window.listener_for(state, FieldState::select_down))
        .on_action(window.listener_for(state, FieldState::select_word_left))
        .on_action(window.listener_for(state, FieldState::select_word_right))
        .on_action(window.listener_for(state, FieldState::select_home))
        .on_action(window.listener_for(state, FieldState::select_end))
        .on_action(window.listener_for(state, FieldState::select_to_start))
        .on_action(window.listener_for(state, FieldState::select_to_end))
        .on_action(window.listener_for(state, FieldState::select_all))
        .on_action(window.listener_for(state, FieldState::copy))
        .on_action(window.listener_for(state, FieldState::enter))
        .on_action(window.listener_for(state, FieldState::shift_enter))
        .on_action(window.listener_for(state, FieldState::secondary_enter));
    if read_only {
        return el;
    }
    // Editing.
    el.on_a11y_action(gpui_kit::AccessibleAction::SetValue, {
        let state = state.clone();
        move |data, window, cx| {
            if let Some(gpui_kit::accesskit::ActionData::Value(value)) = data {
                let value = value.to_string();
                state.update(cx, |s, cx| s.set_by_assistant(value, window, cx));
            }
        }
    })
    .on_action(window.listener_for(state, FieldState::backspace))
    .on_action(window.listener_for(state, FieldState::delete))
    .on_action(window.listener_for(state, FieldState::delete_word_left))
    .on_action(window.listener_for(state, FieldState::delete_word_right))
    .on_action(window.listener_for(state, FieldState::delete_to_line_start))
    .on_action(window.listener_for(state, FieldState::delete_to_line_end))
    .on_action(window.listener_for(state, FieldState::cut))
    .on_action(window.listener_for(state, FieldState::paste))
    .on_action(window.listener_for(state, FieldState::undo))
    .on_action(window.listener_for(state, FieldState::redo))
    .on_action(window.listener_for(state, FieldState::character_palette))
}
