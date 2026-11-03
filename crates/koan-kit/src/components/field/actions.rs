//! What a field's keys do, bound in the `Input` context (which koan's
//! window-wide shortcuts step aside for while a field has the keyboard).

use gpui_kit::{App, KeyBinding};

gpui_kit::actions!(
    input,
    [
        Backspace,
        Delete,
        DeleteWordLeft,
        DeleteWordRight,
        DeleteToLineStart,
        DeleteToLineEnd,
        MoveLeft,
        MoveRight,
        MoveUp,
        MoveDown,
        MoveWordLeft,
        MoveWordRight,
        MoveHome,
        MoveEnd,
        MoveToStart,
        MoveToEnd,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        SelectWordLeft,
        SelectWordRight,
        SelectHome,
        SelectEnd,
        SelectToStart,
        SelectToEnd,
        SelectAll,
        Copy,
        Cut,
        Paste,
        Undo,
        Redo,
        ShowCharacterPalette,
        Escape,
        /// Return.
        Enter,
        /// Shift-Return.
        ShiftEnter,
        /// Command-Return (Control-Return off the Mac).
        SecondaryEnter,
    ]
);

/// The context every field's keys are bound in.
pub const CONTEXT: &str = "Input";

/// Binds a field's keys, as the platform's text fields take them.
pub fn bind(cx: &mut App) {
    let c = Some(CONTEXT);
    let mac = cfg!(target_os = "macos");
    let mut keys = vec![
        KeyBinding::new("backspace", Backspace, c),
        KeyBinding::new("shift-backspace", Backspace, c),
        KeyBinding::new("delete", Delete, c),
        KeyBinding::new("left", MoveLeft, c),
        KeyBinding::new("right", MoveRight, c),
        KeyBinding::new("up", MoveUp, c),
        KeyBinding::new("down", MoveDown, c),
        KeyBinding::new("shift-left", SelectLeft, c),
        KeyBinding::new("shift-right", SelectRight, c),
        KeyBinding::new("shift-up", SelectUp, c),
        KeyBinding::new("shift-down", SelectDown, c),
        KeyBinding::new("home", MoveHome, c),
        KeyBinding::new("end", MoveEnd, c),
        KeyBinding::new("shift-home", SelectHome, c),
        KeyBinding::new("shift-end", SelectEnd, c),
        KeyBinding::new("escape", Escape, c),
        KeyBinding::new("enter", Enter, c),
        KeyBinding::new("shift-enter", ShiftEnter, c),
    ];
    if mac {
        keys.extend([
            KeyBinding::new("alt-backspace", DeleteWordLeft, c),
            KeyBinding::new("alt-delete", DeleteWordRight, c),
            KeyBinding::new("cmd-backspace", DeleteToLineStart, c),
            KeyBinding::new("cmd-delete", DeleteToLineEnd, c),
            KeyBinding::new("ctrl-k", DeleteToLineEnd, c),
            KeyBinding::new("alt-left", MoveWordLeft, c),
            KeyBinding::new("alt-right", MoveWordRight, c),
            KeyBinding::new("alt-shift-left", SelectWordLeft, c),
            KeyBinding::new("alt-shift-right", SelectWordRight, c),
            KeyBinding::new("cmd-left", MoveHome, c),
            KeyBinding::new("cmd-right", MoveEnd, c),
            KeyBinding::new("ctrl-a", MoveHome, c),
            KeyBinding::new("ctrl-e", MoveEnd, c),
            KeyBinding::new("cmd-shift-left", SelectHome, c),
            KeyBinding::new("cmd-shift-right", SelectEnd, c),
            KeyBinding::new("cmd-up", MoveToStart, c),
            KeyBinding::new("cmd-down", MoveToEnd, c),
            KeyBinding::new("cmd-shift-up", SelectToStart, c),
            KeyBinding::new("cmd-shift-down", SelectToEnd, c),
            KeyBinding::new("cmd-a", SelectAll, c),
            KeyBinding::new("cmd-c", Copy, c),
            KeyBinding::new("cmd-x", Cut, c),
            KeyBinding::new("cmd-v", Paste, c),
            KeyBinding::new("cmd-z", Undo, c),
            KeyBinding::new("cmd-shift-z", Redo, c),
            KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, c),
            KeyBinding::new("cmd-enter", SecondaryEnter, c),
        ]);
    } else {
        keys.extend([
            KeyBinding::new("ctrl-backspace", DeleteWordLeft, c),
            KeyBinding::new("ctrl-delete", DeleteWordRight, c),
            KeyBinding::new("ctrl-left", MoveWordLeft, c),
            KeyBinding::new("ctrl-right", MoveWordRight, c),
            KeyBinding::new("ctrl-shift-left", SelectWordLeft, c),
            KeyBinding::new("ctrl-shift-right", SelectWordRight, c),
            KeyBinding::new("ctrl-home", MoveToStart, c),
            KeyBinding::new("ctrl-end", MoveToEnd, c),
            KeyBinding::new("ctrl-shift-home", SelectToStart, c),
            KeyBinding::new("ctrl-shift-end", SelectToEnd, c),
            KeyBinding::new("ctrl-a", SelectAll, c),
            KeyBinding::new("ctrl-c", Copy, c),
            KeyBinding::new("ctrl-x", Cut, c),
            KeyBinding::new("ctrl-v", Paste, c),
            KeyBinding::new("ctrl-z", Undo, c),
            KeyBinding::new("ctrl-shift-z", Redo, c),
            KeyBinding::new("ctrl-y", Redo, c),
            KeyBinding::new("ctrl-enter", SecondaryEnter, c),
        ]);
    }
    cx.bind_keys(keys);
}
