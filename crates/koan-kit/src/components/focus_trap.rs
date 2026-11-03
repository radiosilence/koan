//! Tab and Shift-Tab kept inside a sheet, a question or a menu while it
//! holds the keyboard: the window's root (`Root`) and `sheet::step_focus`
//! move the focus through the window's order and, while a trap holds it,
//! past every stop outside the trap, wrapping inside it.

use gpui::{App, FocusHandle, Global, InteractiveElement, WeakFocusHandle, Window};

/// The containers that keep the keyboard while they hold it.
#[derive(Default)]
struct Traps(Vec<WeakFocusHandle>);

impl Global for Traps {}

pub trait FocusTrap: InteractiveElement + Sized {
    /// This element takes `focus` and keeps Tab and Shift-Tab among its
    /// own controls while the focus is inside it.
    fn focus_trap(self, focus: &FocusHandle, cx: &mut App) -> Self {
        let traps = &mut cx.default_global::<Traps>().0;
        traps.retain(|trap| trap.upgrade().is_some());
        if !traps
            .iter()
            .any(|trap| trap.upgrade().as_ref() == Some(focus))
        {
            traps.push(focus.downgrade());
        }
        self.track_focus(focus)
    }
}

impl<E: InteractiveElement> FocusTrap for E {}

/// The innermost trap holding the focus, if any does.
pub fn active(window: &Window, cx: &App) -> Option<FocusHandle> {
    let traps = cx.try_global::<Traps>()?;
    let mut active: Option<FocusHandle> = None;
    for trap in traps.0.iter().filter_map(WeakFocusHandle::upgrade) {
        if trap.contains_focused(window, cx)
            && active
                .as_ref()
                .is_none_or(|outer| outer.contains(&trap, window))
        {
            active = Some(trap);
        }
    }
    active
}

/// The next (or previous) control in the window's order, kept inside the
/// innermost trap holding the keyboard. GPUI walks the whole window's tab
/// order, so stops outside the trap are passed over, at most one cycle of
/// them; a trap with no stop of its own keeps the focus itself.
pub fn step(forward: bool, window: &mut Window, cx: &mut App) {
    let advance = |window: &mut Window, cx: &mut App| {
        if forward {
            window.focus_next(cx)
        } else {
            window.focus_prev(cx)
        }
    };
    let Some(container) = active(window, cx) else {
        advance(window, cx);
        return;
    };
    advance(window, cx);
    let mut visited = Vec::new();
    while !container.contains_focused(window, cx) {
        let focused = window.focused(cx);
        if visited.contains(&focused) {
            container.focus(window, cx);
            break;
        }
        visited.push(focused);
        advance(window, cx);
    }
}
