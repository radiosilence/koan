use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

use super::app::{ContextAction, ContextMenuState};
use super::theme::Theme;

/// What the queue's menu is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuFor {
    /// An album heading: its whole run.
    Album,
    /// One row. Not in the library, it can only be played or removed.
    Track {
        in_library: bool,
        favourite: bool,
        on_disk: bool,
    },
    /// Several rows; `favourite` when every one of them is.
    Tracks { favourite: bool },
}

const RULE: (ContextAction, &str, char) = (ContextAction::Separator, "", ' ');

/// The queue's menu, as the apps' queue menu offers it, with a key for each
/// item. `share` is whether a share link can be made.
pub fn queue_items(of: MenuFor, share: bool) -> Vec<(ContextAction, &'static str, char)> {
    use ContextAction::*;
    let mut items = Vec::new();
    match of {
        MenuFor::Album => {
            items.extend([
                (Play, "Play", 'p'),
                (Remove, "Remove album", 'd'),
                RULE,
                (AddToPlaylist, "Add to playlist\u{2026}", 'w'),
                RULE,
                (Organize, "Organize files\u{2026}", 'o'),
                (GoToAlbum, "Go to album", 'a'),
                (GoToArtist, "Go to artist", 'r'),
            ]);
            if share {
                items.push((CopyShareLink, "Copy album share link", 's'));
            }
        }
        MenuFor::Track {
            in_library,
            favourite,
            on_disk,
        } => {
            items.extend([(Play, "Play", 'p'), (Remove, "Remove", 'd')]);
            if in_library {
                items.extend([
                    RULE,
                    (AddToPlaylist, "Add to playlist\u{2026}", 'w'),
                    RULE,
                    (Organize, "Organize files\u{2026}", 'o'),
                    (
                        ToggleFavourite,
                        if favourite {
                            "Remove favourite"
                        } else {
                            "Favourite track"
                        },
                        'f',
                    ),
                    (GoToAlbum, "Go to album", 'a'),
                    (GoToArtist, "Go to artist", 'r'),
                    if on_disk {
                        (RemoveDownload, "Remove downloaded file", 'u')
                    } else {
                        (Download, "Download to cache", 'l')
                    },
                ]);
                if share {
                    items.push((CopyShareLink, "Copy share link", 's'));
                }
                items.push((TrackInfo, "Track info", 'i'));
            }
        }
        MenuFor::Tracks { favourite } => {
            items.extend([
                (Remove, "Remove", 'd'),
                RULE,
                (AddToPlaylist, "Add to playlist\u{2026}", 'w'),
                RULE,
                (Organize, "Organize files\u{2026}", 'o'),
                (
                    ToggleFavourite,
                    if favourite {
                        "Remove favourites"
                    } else {
                        "Favourite tracks"
                    },
                    'f',
                ),
                (Download, "Download to cache", 'l'),
                (RemoveDownload, "Remove downloaded files", 'u'),
            ]);
            if share {
                items.push((CopyShareLink, "Copy share link", 's'));
            }
        }
    }
    items
}

/// The item `delta` steps from `cursor`, past the rules, wrapping.
pub fn step(actions: &[(ContextAction, &str, char)], cursor: usize, delta: isize) -> usize {
    let n = actions.len() as isize;
    if n == 0 {
        return 0;
    }
    let mut at = cursor as isize;
    for _ in 0..n {
        at = (at + delta).rem_euclid(n);
        if actions[at as usize].0 != ContextAction::Separator {
            break;
        }
    }
    at as usize
}

pub struct ContextMenuOverlay<'a> {
    state: &'a ContextMenuState,
    theme: &'a Theme,
    click_col: Option<u16>,
    click_row: Option<u16>,
}

impl<'a> ContextMenuOverlay<'a> {
    pub fn new(state: &'a ContextMenuState, theme: &'a Theme) -> Self {
        Self {
            state,
            theme,
            click_col: None,
            click_row: None,
        }
    }

    /// Position the context menu at the mouse click location.
    pub fn at_position(mut self, col: u16, row: u16) -> Self {
        self.click_col = Some(col);
        self.click_row = Some(row);
        self
    }
}

/// Compute the popup rect at a specific position, clamped to terminal bounds.
/// Pass None for both click_col/click_row to center the menu.
pub fn context_menu_rect_at(
    area: Rect,
    action_count: usize,
    click_col: Option<u16>,
    click_row: Option<u16>,
) -> Rect {
    let w = 32u16.min(area.width);
    // 2 for border top/bottom + 1 row per action
    let h = (2 + action_count as u16).min(area.height);

    let (x, y) = match (click_col, click_row) {
        (Some(cx), Some(cy)) => {
            // Position at click, clamped to terminal bounds.
            let x = if cx + w > area.x + area.width {
                (area.x + area.width).saturating_sub(w)
            } else {
                cx
            };
            let y = if cy + h > area.y + area.height {
                cy.saturating_sub(h)
            } else {
                cy
            };
            (x, y)
        }
        _ => {
            // Centered fallback.
            let x = area.x + (area.width.saturating_sub(w)) / 2;
            let y = area.y + (area.height.saturating_sub(h)) / 2;
            (x, y)
        }
    };

    Rect::new(x, y, w, h)
}

impl Widget for ContextMenuOverlay<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let popup = context_menu_rect_at(
            area,
            self.state.actions.len(),
            self.click_col,
            self.click_row,
        );
        Clear.render(popup, buf);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(self.theme.hint_key)
            .title(" Actions ");
        let inner = block.inner(popup);
        block.render(popup, buf);

        for (i, (action, label, hotkey)) in self.state.actions.iter().enumerate() {
            if i >= inner.height as usize {
                break;
            }
            if *action == ContextAction::Separator {
                for x in inner.x..inner.x + inner.width {
                    buf[(x, inner.y + i as u16)]
                        .set_char('\u{2500}')
                        .set_style(self.theme.rule);
                }
                continue;
            }
            let is_selected = i == self.state.cursor;
            let base_style = if is_selected {
                self.theme.picker_cursor
            } else {
                self.theme.hint_desc
            };
            let key_style = if is_selected {
                base_style
            } else {
                self.theme.hint_key
            };
            let line = Line::from(vec![
                Span::styled(" [", base_style),
                Span::styled(hotkey.to_string(), key_style),
                Span::styled("] ", base_style),
                Span::styled(*label, base_style),
                Span::styled(" ", base_style),
            ]);
            let row = Rect::new(inner.x, inner.y + i as u16, inner.width, 1);
            Paragraph::new(line).render(row, buf);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_menu_offers_what_applies_to_it() {
        let labels = |of| -> Vec<&str> {
            queue_items(of, false)
                .into_iter()
                .filter(|(a, _, _)| *a != ContextAction::Separator)
                .map(|(_, l, _)| l)
                .collect()
        };
        assert_eq!(
            labels(MenuFor::Track {
                in_library: false,
                favourite: false,
                on_disk: false
            }),
            ["Play", "Remove"],
            "a track the library lost can only be played or removed"
        );
        let track = labels(MenuFor::Track {
            in_library: true,
            favourite: true,
            on_disk: true,
        });
        assert!(track.contains(&"Remove favourite") && track.contains(&"Remove downloaded file"));
        assert!(!track.contains(&"Download to cache"));
        assert!(labels(MenuFor::Album).contains(&"Remove album"));
        assert!(!labels(MenuFor::Tracks { favourite: false }).contains(&"Play"));
    }

    #[test]
    fn the_cursor_steps_over_the_rules() {
        let items = queue_items(MenuFor::Album, false);
        assert_eq!(items[2].0, ContextAction::Separator);
        assert_eq!(step(&items, 1, 1), 3);
        assert_eq!(step(&items, 3, -1), 1);
        assert_eq!(step(&items, 0, -1), items.len() - 1);
    }

    #[test]
    fn hotkeys_are_unique_within_a_menu() {
        for of in [
            MenuFor::Album,
            MenuFor::Tracks { favourite: false },
            MenuFor::Track {
                in_library: true,
                favourite: false,
                on_disk: false,
            },
        ] {
            let keys: Vec<char> = queue_items(of, true)
                .into_iter()
                .filter(|(a, _, _)| *a != ContextAction::Separator)
                .map(|(_, _, k)| k)
                .collect();
            let unique: std::collections::HashSet<_> = keys.iter().collect();
            assert_eq!(unique.len(), keys.len(), "{of:?}");
        }
    }
}
