//! What a listener keeps rather than what the library holds: playlists, the
//! play history, the shelves (favourites, recently played, what is on this
//! device) and the downloads arriving.
//!
//! Each page reads the engine through the library's `Lib` and draws its rows
//! with the library's row and tile, so a track looks the same wherever it is
//! listed. What a page writes is asked first, through the one host here
//! (`Asking`), which says what will change before anything does.

pub mod downloads;
pub mod history;
pub mod playlist;
pub mod shelf;

use std::collections::HashSet;
use std::rc::Rc;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::components::{Button, ButtonKind, TextField};
use crate::dsp::sheet::{body, head, note, panel};
use crate::theme::{Look, Space, Tone};

/// Rows picked in a list whose rows are not tracks: plays in the history,
/// entries in a playlist. Picked by id, since one track can be several rows,
/// and ranged by position in the list as drawn.
#[derive(Default)]
pub struct Picks {
    ids: HashSet<i64>,
    /// Where a ⇧-click range starts: the last row clicked without ⇧.
    anchor: Option<i64>,
}

impl Picks {
    pub fn contains(&self, id: i64) -> bool {
        self.ids.contains(&id)
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn clear(&mut self) {
        self.ids.clear();
        self.anchor = None;
    }

    /// The picked ids in `order`'s order.
    pub fn in_order(&self, order: &[i64]) -> Vec<i64> {
        order
            .iter()
            .copied()
            .filter(|id| self.ids.contains(id))
            .collect()
    }

    pub fn all(&mut self, order: &[i64]) {
        self.ids = order.iter().copied().collect();
    }

    /// Keeps only ids still in `order`, after the list was read again.
    pub fn retain(&mut self, order: &[i64]) {
        let live: HashSet<i64> = order.iter().copied().collect();
        self.ids.retain(|id| live.contains(id));
        if self.anchor.is_some_and(|a| !live.contains(&a)) {
            self.anchor = None;
        }
    }

    /// What a click on row `id` does, as a list's click does anywhere: picks
    /// it alone, ⌘ adds or removes it, ⇧ extends from the anchor through
    /// `order`. Returns whether it was a double click, which plays.
    pub fn click(&mut self, id: i64, event: &ClickEvent, order: &[i64], cx: &App) -> bool {
        let m = event.modifiers();
        if event.click_count() >= 2 && !m.shift && !m.secondary(cx) {
            return true;
        }
        if m.shift
            && let Some(anchor) = self.anchor
            && let (Some(a), Some(b)) = (
                order.iter().position(|x| *x == anchor),
                order.iter().position(|x| *x == id),
            )
        {
            self.ids = order[a.min(b)..=a.max(b)].iter().copied().collect();
        } else if m.secondary(cx) {
            if !self.ids.remove(&id) {
                self.ids.insert(id);
            }
            self.anchor = Some(id);
        } else {
            self.ids = HashSet::from([id]);
            self.anchor = Some(id);
        }
        false
    }

    /// The ids a row's menu or drag acts on: the pick when the row is in it,
    /// the row alone otherwise.
    pub fn acting_on(&self, id: i64, order: &[i64]) -> Vec<i64> {
        if self.ids.contains(&id) {
            self.in_order(order)
        } else {
            vec![id]
        }
    }
}

/// A question asked before a write: what will change, and the verb that
/// changes it. With a name field, the preview follows what is typed.
struct Ask {
    title: SharedString,
    verb: SharedString,
    destructive: bool,
    /// What the write will change, a line each.
    preview: Rc<dyn Fn(&str) -> Vec<SharedString>>,
    /// Said under the preview: what the write leaves alone, or how to undo
    /// it.
    note: Option<SharedString>,
    name: Option<Entity<InputState>>,
    run: Rc<dyn Fn(String, &mut App)>,
    /// The sheet's own keys, for a question without a field; with one, the
    /// field has them and answers Return itself.
    focus: Option<FocusHandle>,
    _enter: Option<Subscription>,
}

/// The one place a question is asked, over the whole window: a menu that
/// asks is gone the moment it is used, so the question cannot live there.
#[derive(Default)]
pub struct Asking {
    ask: Option<Ask>,
}

struct AskingHandle(Entity<Asking>);

impl Global for AskingHandle {}

/// The window's question host, made the first time it is wanted.
pub fn asking(cx: &mut App) -> Entity<Asking> {
    if let Some(handle) = cx.try_global::<AskingHandle>() {
        return handle.0.clone();
    }
    let entity = cx.new(|_| Asking::default());
    cx.set_global(AskingHandle(entity.clone()));
    entity
}

/// What a confirmation shows and does.
pub struct Confirm {
    pub title: SharedString,
    pub verb: SharedString,
    pub destructive: bool,
    pub lines: Vec<SharedString>,
    pub note: Option<SharedString>,
}

/// Asks before `run`: the lines are what it will change.
pub fn confirm(c: Confirm, window: &mut Window, cx: &mut App, run: impl Fn(&mut App) + 'static) {
    let lines = c.lines;
    let focus = cx.focus_handle();
    window.focus(&focus, cx);
    show(
        Ask {
            title: c.title,
            verb: c.verb,
            destructive: c.destructive,
            preview: Rc::new(move |_| lines.clone()),
            note: c.note,
            name: None,
            run: Rc::new(move |_, cx| run(cx)),
            focus: Some(focus),
            _enter: None,
        },
        cx,
    );
}

/// Asks for a name, saying what it will do with what is typed, before
/// `run` gets it trimmed and not empty.
pub fn name(
    title: impl Into<SharedString>,
    verb: impl Into<SharedString>,
    initial: &str,
    preview: impl Fn(&str) -> Vec<SharedString> + 'static,
    window: &mut Window,
    cx: &mut App,
    run: impl Fn(String, &mut App) + 'static,
) {
    let initial = initial.to_owned();
    let input = cx.new(|cx| InputState::new(window, cx).default_value(initial));
    let focus = input.read(cx).focus_handle(cx);
    window.focus(&focus, cx);
    let focus = None;
    let run: Rc<dyn Fn(String, &mut App)> = Rc::new(run);
    let enter = {
        let run = run.clone();
        window.subscribe(&input, cx, move |input, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                let typed = input.read(cx).value().trim().to_owned();
                if !typed.is_empty() {
                    dismiss(cx);
                    run(typed, cx);
                }
            }
        })
    };
    show(
        Ask {
            title: title.into(),
            verb: verb.into(),
            destructive: false,
            preview: Rc::new(preview),
            note: None,
            name: Some(input.clone()),
            run,
            focus,
            _enter: Some(enter),
        },
        cx,
    );
    // The preview follows the field.
    let host = asking(cx);
    cx.observe(&input, move |_, cx| host.update(cx, |_, cx| cx.notify()))
        .detach();
}

fn show(ask: Ask, cx: &mut App) {
    asking(cx).update(cx, |a, cx| {
        a.ask = Some(ask);
        cx.notify();
    });
}

fn dismiss(cx: &mut App) {
    asking(cx).update(cx, |a, cx| {
        a.ask = None;
        cx.notify();
    });
}

/// The question being asked, over the window, for the shell to draw.
pub fn overlay(window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    let host = asking(cx);
    let a = host.read(cx);
    let ask = a.ask.as_ref()?;
    let look = Look::of(window, cx);
    let typed = ask
        .name
        .as_ref()
        .map(|i| i.read(cx).value().trim().to_owned());
    let lines = (ask.preview)(typed.as_deref().unwrap_or(""));
    let blocked = typed.as_ref().is_some_and(String::is_empty);
    let go = {
        let (run, name) = (ask.run.clone(), ask.name.clone());
        move |cx: &mut App| {
            let typed = name
                .as_ref()
                .map(|i| i.read(cx).value().trim().to_owned())
                .unwrap_or_default();
            if name.is_some() && typed.is_empty() {
                return;
            }
            dismiss(cx);
            run(typed, cx);
        }
    };
    let go = Rc::new(go);
    let named = ask.name.is_some();
    let cancel = Button::new("ask-cancel", ButtonKind::Standard)
        .label("cancel")
        .on_click(|_, _, cx| dismiss(cx));
    let verb = {
        let go = go.clone();
        Button::new("ask-go", ButtonKind::Standard)
            .label(ask.verb.as_ref())
            .destructive(ask.destructive)
            .disabled(blocked)
            .on_click(move |_, _, cx| go(cx))
    };
    let rows: Vec<AnyElement> = lines
        .into_iter()
        .map(|line| note(line, Tone::Ink, &look).into_any_element())
        .collect();
    Some(
        div()
            .id("ask")
            .key_context("Ask")
            .when_some(ask.focus.as_ref(), |d, focus| d.track_focus(focus))
            .on_key_down(move |e: &KeyDownEvent, _, cx| match e.keystroke.key.as_str() {
                "escape" => dismiss(cx),
                "enter" if !named => go(cx),
                _ => {}
            })
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .items_start()
            .pt(look.space(Space::Xxl))
            .bg(look.palette.bg.hsla().opacity(0.6))
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                panel(&look)
                    .child(head(
                        ask.title.as_ref(),
                        vec![cancel.into_any_element(), verb.into_any_element()],
                        &look,
                    ))
                    .child(
                        body("ask-body", &look)
                            .pt(look.space(Space::M))
                            .when_some(ask.name.as_ref(), |d, input| {
                                d.child(TextField::new(input))
                            })
                            .children(rows)
                            .when_some(ask.note.clone(), |d, n| {
                                d.child(note(n, Tone::Muted, &look))
                            }),
                    ),
            )
            .into_any_element(),
    )
}

/// "3 tracks", "1 play".
pub fn count(n: usize, one: &str, many: &str) -> String {
    crate::library::chrome::count(n as u64, one, many)
}

/// Up to `shown` names, then how many more: a preview's list of what a
/// write touches.
pub fn listed(names: impl IntoIterator<Item = String>, shown: usize) -> Vec<SharedString> {
    let names: Vec<String> = names.into_iter().collect();
    let mut lines: Vec<SharedString> = names
        .iter()
        .take(shown)
        .map(|n| SharedString::from(format!("· {n}")))
        .collect();
    if names.len() > shown {
        lines.push(format!("and {} more", names.len() - shown).into());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pick_keeps_the_lists_order() {
        let mut picks = Picks::default();
        picks.ids = HashSet::from([3, 1]);
        assert_eq!(picks.in_order(&[1, 2, 3]), vec![1, 3]);
        assert_eq!(picks.acting_on(2, &[1, 2, 3]), vec![2]);
        assert_eq!(picks.acting_on(3, &[1, 2, 3]), vec![1, 3]);
        picks.retain(&[3]);
        assert_eq!(picks.in_order(&[1, 2, 3]), vec![3]);
    }

    #[test]
    fn a_long_list_is_cut_short() {
        let lines = listed((0..5).map(|i| i.to_string()), 3);
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[3].as_ref(), "and 2 more");
    }
}
