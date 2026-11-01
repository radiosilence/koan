//! What `?` and ⌘/ show: both shortcut tables, grouped, as the Mac app's
//! `ShortcutsSheet` lays them out.
//!
//! Single keys are written down only here, since a bare key cannot be a menu
//! item. The modifier ones are in the menu bar on macOS, and repeated here
//! because nobody opens six menus to find out what a key does; on Linux,
//! which has no menu bar, this is the only list of them.
//!
//! Drawn from the theme's tokens directly (docs/design/koan-theme.md) until
//! the theme module is in place.

use gpui_kit::{
    App, Context, EventEmitter, FocusHandle, Focusable, Hsla, IntoElement, KeyBinding,
    OperatingSystem, ParentElement, Render, Rgba, SharedString, Styled, Window, WindowAppearance,
    actions, div, prelude::*, px,
};

use super::actions::{Group, HOTKEYS, MENU, Shortcut, caption};

actions!(shortcuts_sheet, [Dismiss]);

const CONTEXT: &str = "ShortcutsSheet";

/// The sheet asks to be closed: escape, or its done button.
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
    cx.bind_keys([KeyBinding::new("escape", Dismiss, Some(CONTEXT))]);
}

impl ShortcutsSheet {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
        }
    }
}

/// The theme's colours for one appearance.
struct Palette {
    surface: Hsla,
    rule: Hsla,
    ink: Hsla,
    strong: Hsla,
    muted: Hsla,
}

fn rgb(hex: u32) -> Hsla {
    Rgba {
        r: ((hex >> 16) & 0xff) as f32 / 255.0,
        g: ((hex >> 8) & 0xff) as f32 / 255.0,
        b: (hex & 0xff) as f32 / 255.0,
        a: 1.0,
    }
    .into()
}

impl Palette {
    fn of(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self {
                surface: rgb(0x2a2a2a),
                rule: rgb(0x383838),
                ink: rgb(0xcccccc),
                strong: rgb(0xffffff),
                muted: rgb(0x919191),
            },
            WindowAppearance::Light | WindowAppearance::VibrantLight => Self {
                surface: rgb(0xf2f2f2),
                rule: rgb(0xe0e0e0),
                ink: rgb(0x333333),
                strong: rgb(0x111111),
                muted: rgb(0x666666),
            },
        }
    }
}

/// One line of the sheet: its keys as printed, and what they do.
struct Line {
    keys: Vec<String>,
    label: &'static str,
}

/// The lines of `table` in `group`, keys captioned for `os`.
fn lines(table: &[Shortcut], group: Group, os: OperatingSystem) -> Vec<Line> {
    table
        .iter()
        .filter(|s| s.group == group)
        .map(|s| Line {
            keys: s.keys.iter().map(|k| caption(k, os)).collect(),
            label: s.title,
        })
        .collect()
}

impl Render for ShortcutsSheet {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let os = cx.platform_info().os;
        let p = Palette::of(window.appearance());
        // The theme's scale is drawn at 0.86 on the Mac; see Type.
        let scale = if os == OperatingSystem::Mac {
            0.86
        } else {
            1.0
        };
        let size = |pt: f32| px(pt * scale);
        let modifier = if os == OperatingSystem::Mac {
            "with ⌘"
        } else {
            "with ctrl"
        };

        let section = |title: &'static str| {
            div()
                .text_size(size(12.0))
                .text_color(p.muted)
                .child(SharedString::from(title))
        };
        let badge = |key: String| {
            div()
                .px(px(7.0))
                .py(px(2.0))
                .border_1()
                .border_color(p.rule)
                .text_size(size(12.0))
                .text_color(p.ink)
                .child(key)
        };
        let columns = |table: &[Shortcut]| {
            div().flex().flex_wrap().gap(px(30.0)).children(
                Group::ALL
                    .into_iter()
                    .map(|group| (group, lines(table, group, os)))
                    .filter(|(_, lines)| !lines.is_empty())
                    .map(|(group, lines)| {
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(7.0))
                            .min_w(px(210.0))
                            .child(section(group.title()))
                            .children(lines.into_iter().map(|line| {
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(9.0))
                                    .child(
                                        div()
                                            .flex()
                                            .gap(px(3.0))
                                            .children(line.keys.into_iter().map(badge)),
                                    )
                                    .child(
                                        div()
                                            .text_size(size(13.0))
                                            .text_color(p.ink)
                                            .child(line.label.to_lowercase()),
                                    )
                            }))
                    }),
            )
        };

        div()
            .id("shortcuts-sheet")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|_, _: &Dismiss, _, cx| cx.emit(Dismissed)))
            .font_family("Geist Mono")
            .flex()
            .flex_col()
            .gap(px(18.0))
            .p(px(24.0))
            .min_w(px(620.0))
            .bg(p.surface)
            .border_1()
            .border_color(p.rule)
            .child(
                div()
                    .text_size(size(22.0))
                    .font_weight(gpui_kit::FontWeight(300.0))
                    .text_color(p.strong)
                    .child("keyboard shortcuts"),
            )
            .child(
                div()
                    .id("shortcuts-sheet-scroll")
                    .flex()
                    .flex_col()
                    .gap(px(20.0))
                    .max_h(px(460.0))
                    .overflow_y_scroll()
                    .child(columns(HOTKEYS))
                    .child(div().h(px(1.0)).bg(p.rule))
                    .child(section(modifier))
                    .child(columns(MENU)),
            )
            .child(
                div()
                    .text_size(size(12.0))
                    .text_color(p.muted)
                    .child("none of these fire while you're typing."),
            )
            .child(
                div().flex().justify_end().child(
                    div()
                        .id("shortcuts-sheet-done")
                        .px(px(8.0))
                        .py(px(4.0))
                        .text_size(size(14.0))
                        .text_color(p.ink)
                        .cursor_pointer()
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(Dismissed)))
                        .child("done"),
                ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_group_with_a_hotkey_lists_it() {
        let total: usize = Group::ALL
            .into_iter()
            .map(|g| lines(HOTKEYS, g, OperatingSystem::Mac).len())
            .sum();
        assert_eq!(total, HOTKEYS.len());
    }

    #[test]
    fn a_line_shows_every_key_that_does_it() {
        let nav = lines(HOTKEYS, Group::Navigation, OperatingSystem::Mac);
        let albums = nav.iter().find(|l| l.label == "Albums").unwrap();
        assert_eq!(albums.keys, ["l", "a"]);
    }
}
