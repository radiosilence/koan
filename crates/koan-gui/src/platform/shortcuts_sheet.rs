//! What `?` and ⌘/ show: both shortcut tables, grouped, as the Mac app's
//! `ShortcutsSheet` lays them out.
//!
//! Single keys are written down only here, since a bare key cannot be a menu
//! item. The modifier ones are in the menu bar on macOS, and repeated here
//! because nobody opens six menus to find out what a key does; on Linux,
//! which has no menu bar, this is the only list of them.
//!
//! Built from koan-kit's sheet: its panel, head and body, keys as badges.

use koan_kit::app::platform::OperatingSystem;
use koan_kit::app::*;

use crate::components::{Badge, Button, ButtonKind, sheet};
use crate::theme::{KoanStyled, Look, Space, Tone, TypeRole, label};

use super::actions::{APPLICATION, Group, HOTKEYS, LISTS, MENU, Shortcut, caption};

actions!(shortcuts_sheet, [Dismiss]);

const CONTEXT: &str = "ShortcutsSheet";

/// The sheet asks to be closed: escape, return (its done button is the
/// default), or the button.
pub struct Dismissed;

pub struct ShortcutsSheet {
    focus: FocusHandle,
}

impl EventEmitter<Dismissed> for ShortcutsSheet {}

impl Focusable for ShortcutsSheet {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// Bind the sheet's own keys. Once, at launch.
pub fn bind(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", Dismiss, Some(CONTEXT)),
        KeyBinding::new("enter", Dismiss, Some(CONTEXT)),
    ]);
}

impl ShortcutsSheet {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
        }
    }
}

/// One line of the sheet: its keys as printed, and what they do.
struct Line {
    keys: Vec<String>,
    label: &'static str,
}

/// The lines of `table` in `group`, keys captioned for `os`.
fn lines(table: &[&Shortcut], group: Group, os: OperatingSystem) -> Vec<Line> {
    table
        .iter()
        .filter(|s| s.group == group)
        .map(|s| Line {
            keys: s.keys.iter().map(|k| caption(k, os)).collect(),
            label: s.title,
        })
        .collect()
}

/// The shortcuts with a modifier, as the sheet lists them. On macOS the
/// application's own (Settings, Hide, Quit) are in the menu bar, as in every
/// app; elsewhere there is no menu bar, so they are listed here too.
fn with_modifier(os: OperatingSystem) -> Vec<&'static Shortcut> {
    let mut all: Vec<&Shortcut> = MENU.iter().collect();
    if os != OperatingSystem::Mac {
        all.extend(
            APPLICATION
                .iter()
                .filter(|s| !s.keys[0].starts_with("cmd-")),
        );
    }
    all
}

/// One group's column: its title, then each line's keys and what they do.
fn group_column(group: Group, lines: Vec<Line>, look: &Look) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(look.space(Space::S))
        .min_w(look.spaced(210.0))
        .child(sheet::note(label(group.title()), Tone::Muted, look))
        .children(lines.into_iter().map(|line| {
            div()
                .flex()
                .items_center()
                .gap(look.space(Space::S))
                .child(
                    div()
                        .flex()
                        .gap(look.space(Space::Xs))
                        .children(line.keys.into_iter().map(Badge::new)),
                )
                .child(
                    div()
                        .text_role(TypeRole::Meta, look)
                        .text_color(look.palette.ink.hsla())
                        .child(label(line.label)),
                )
        }))
}

impl Render for ShortcutsSheet {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let os = cx.platform_info().os;
        let look = Look::of(window, cx);
        let modifier = if os == OperatingSystem::Mac {
            "with ⌘"
        } else {
            "with ctrl"
        };
        let columns = |table: &[&Shortcut]| {
            div()
                .flex()
                .flex_wrap()
                .gap(look.space(Space::Xl))
                .children(
                    Group::ALL
                        .into_iter()
                        .map(|group| (group, lines(table, group, os)))
                        .filter(|(_, lines)| !lines.is_empty())
                        .map(|(group, lines)| group_column(group, lines, &look)),
                )
        };
        let done = Button::new("shortcuts-sheet-done", ButtonKind::Text)
            .label("done")
            .on_click(cx.listener(|_, _, _, cx| cx.emit(Dismissed)));

        sheet::panel(&look)
            .id("shortcuts-sheet")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|_, _: &Dismiss, _, cx| cx.emit(Dismissed)))
            .child(sheet::head(
                "keyboard shortcuts",
                vec![done.into_any_element()],
                &look,
            ))
            .child(
                sheet::body("shortcuts-sheet-scroll", &look)
                    .pt(look.space(Space::L))
                    .gap(look.space(Space::L))
                    .child(columns(&HOTKEYS.iter().chain(LISTS).collect::<Vec<_>>()))
                    .child(div().h(look.hairline).bg(look.palette.rule.hsla()))
                    .child(sheet::note(modifier, Tone::Muted, &look))
                    .child(columns(&with_modifier(os)))
                    .child(sheet::note(
                        "single keys never fire while you're typing.",
                        Tone::Muted,
                        &look,
                    )),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn every_group_with_a_hotkey_lists_it() {
        let total: usize = Group::ALL
            .into_iter()
            .map(|g| lines(&HOTKEYS.iter().collect::<Vec<_>>(), g, OperatingSystem::Mac).len())
            .sum();
        assert_eq!(total, HOTKEYS.len());
    }

    #[test]
    fn the_list_keys_are_listed_under_their_own_heading() {
        let rows = lines(
            &LISTS.iter().collect::<Vec<_>>(),
            Group::Lists,
            OperatingSystem::Mac,
        );
        assert_eq!(rows.len(), LISTS.len());
    }

    #[test]
    fn linux_lists_settings_and_quit_and_the_mac_leaves_them_to_its_menu() {
        let titles = |os| -> Vec<&str> { with_modifier(os).iter().map(|s| s.title).collect() };
        let linux = titles(OperatingSystem::Linux);
        assert!(linux.contains(&"Quit kōan") && linux.contains(&"Settings…"));
        assert!(!linux.contains(&"Hide kōan"));
        assert!(!titles(OperatingSystem::Mac).contains(&"Quit kōan"));
    }

    #[test]
    fn a_line_shows_every_key_that_does_it() {
        let nav = lines(
            &HOTKEYS.iter().collect::<Vec<_>>(),
            Group::Navigation,
            OperatingSystem::Mac,
        );
        let albums = nav.iter().find(|l| l.label == "Albums").unwrap();
        assert_eq!(albums.keys, ["l", "a"]);
    }
}
