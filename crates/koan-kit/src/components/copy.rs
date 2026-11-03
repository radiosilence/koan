//! Copying a value: a button that says what it copies ("copy link"), and a
//! value that copies itself when clicked (a server's address). Each says
//! "copied" for a moment after, so the click is seen to have done something,
//! and answers with a line drawn under it (the button's press answers as
//! every button's does), keeping that moment itself: a page holds no state
//! for it.

use std::time::Duration;

use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{Button, ButtonKind, Tooltip, accessible, child, focus, ring};
use crate::theme::{Icon, KoanStyled, Look, TypeRole, label};

/// How long "copied" stands in for the label.
const COPIED: Duration = Duration::from_millis(1600);

/// Whether a copy was made a moment ago, and the timer that ends the moment.
#[derive(Default)]
struct Moment {
    copied: bool,
    _reset: Option<Task<()>>,
}

fn moment(id: &ElementId, window: &mut Window, cx: &mut App) -> Entity<Moment> {
    window.use_keyed_state(child(id, "copied"), cx, |_, _| Moment::default())
}

/// Puts `value` on the clipboard and starts the moment.
fn copy(state: &Entity<Moment>, value: &SharedString, cx: &mut App) {
    cx.write_to_clipboard(ClipboardItem::new_string(value.to_string()));
    state.update(cx, |m, cx| {
        m.copied = true;
        m._reset = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COPIED).await;
            this.update(cx, |m, cx| {
                m.copied = false;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    });
}

#[derive(IntoElement)]
pub struct CopyButton {
    id: ElementId,
    label: SharedString,
    value: SharedString,
    kind: ButtonKind,
    icon: Icon,
    disabled: bool,
}

impl CopyButton {
    /// `label` says what is copied, in the app's words: "copy link".
    pub fn new(
        id: impl Into<ElementId>,
        label: impl AsRef<str>,
        value: impl Into<SharedString>,
    ) -> Self {
        Self {
            id: id.into(),
            label: crate::theme::label(label),
            value: value.into(),
            kind: ButtonKind::Bordered,
            icon: Icon::Copy,
            disabled: false,
        }
    }

    /// Nothing to copy yet: the link is still being made.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn kind(mut self, kind: ButtonKind) -> Self {
        self.kind = kind;
        self
    }

    /// The glyph before the label, the copy glyph unless what is copied has
    /// one of its own (a link to share).
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = icon;
        self
    }
}

impl RenderOnce for CopyButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = moment(&self.id, window, cx);
        let copied = state.read(cx).copied;
        let value = self.value;
        // The face is monospaced: each word padded either side to the longer
        // one's length keeps the button's width, and the word centred, as it
        // says "copied" and back, so what is beside it does not move.
        let said = label("copied");
        let width = said.chars().count().max(self.label.chars().count());
        let held = |text: &SharedString| -> SharedString {
            let pad = width - text.chars().count();
            let (before, after) = (pad / 2, pad - pad / 2);
            format!(
                "{}{text}{}",
                "\u{a0}".repeat(before),
                "\u{a0}".repeat(after)
            )
            .into()
        };
        Button::new(self.id, self.kind)
            .text(if copied {
                held(&said)
            } else {
                held(&self.label)
            })
            .icon(if copied { Icon::Check } else { self.icon })
            .disabled(self.disabled)
            .on_click(move |_, _, cx| copy(&state, &value, cx))
    }
}

/// A value shown in `control`, `muted`, `ink` under the pointer, which a
/// click copies: the value reads "copied" for a moment after.
#[derive(IntoElement)]
pub struct CopyText {
    id: ElementId,
    value: SharedString,
    help: Option<SharedString>,
}

impl CopyText {
    pub fn new(id: impl Into<ElementId>, value: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            value: value.into(),
            help: None,
        }
    }

    /// The tooltip, where "copy" does not say what is copied: "copy the
    /// server's address".
    pub fn help(mut self, help: impl Into<SharedString>) -> Self {
        self.help = Some(label(help.into()));
        self
    }
}

impl RenderOnce for CopyText {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let state = moment(&self.id, window, cx);
        let copied = state.read(cx).copied;
        let answers = super::Answerer::of(&self.id, window, cx);
        let handle = focus(&self.id, window, cx);
        let accent = look.accent.hsla();
        let ink = look.palette.ink.hsla();
        let value = self.value;
        let help = self.help.unwrap_or_else(|| label("copy"));
        // A button to a screen reader, named by what it copies and the value.
        let name: SharedString = format!("{help}: {value}").into();
        // GPUI turns Enter and Space on the focused value into a click; the
        // ring stands outside the text, which clips.
        accessible(div().id(self.id), gpui_kit::Role::Button, Some(name))
            .relative()
            .flex()
            .min_w_0()
            .cursor_pointer()
            .track_focus(&handle)
            .text_role(TypeRole::Control, &look)
            .text_color(look.palette.muted.hsla())
            .hover(move |s| s.text_color(ink))
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(if copied {
                        label("copied")
                    } else {
                        value.clone()
                    }),
            )
            .children(answers.layer(window, cx))
            .children(ring(&handle, &look, window))
            .tooltip(move |window, cx| Tooltip::view(help.clone(), window, cx))
            .on_click(move |_, _, cx| {
                answers.start(super::Answer::Sweep, accent, cx);
                copy(&state, &value, cx)
            })
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::{Modifiers, TestAppContext, VisualTestContext};

    use super::*;
    use core::prelude::v1::test;

    struct Page;

    impl Render for Page {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(300.)).child(
                div()
                    .debug_selector(|| "copy".into())
                    .child(CopyButton::new("copy", "copy link", "https://koan.rocks/x")),
            )
        }
    }

    /// A click puts the value on the clipboard and says so for a moment.
    #[gpui_kit::test]
    fn a_copy_lands_on_the_clipboard_and_says_so(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
        });
        let (_, cx) = cx.add_window_view(|_, _| Page);
        let cx: &mut VisualTestContext = cx;
        cx.run_until_parked();
        let at = cx.debug_bounds("copy").expect("drawn").center();
        cx.simulate_click(at, Modifiers::default());
        cx.run_until_parked();
        let copied = cx.update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()));
        assert_eq!(copied.as_deref(), Some("https://koan.rocks/x"));
        cx.executor()
            .advance_clock(COPIED + Duration::from_millis(10));
        cx.run_until_parked();
    }
}
