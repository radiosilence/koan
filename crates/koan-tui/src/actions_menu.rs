//! The list of things that can be done with what the cursor is on: the keys
//! that have no room in a hint bar, as a menu for a pointer. A page builds the
//! items; choosing one runs the same action its key would.

use std::cell::Cell;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;

use koan_core::db::queries::CreditsOf;
use koan_core::helpers::{credit_parts, credits_by_id};

use super::app::PickerAction;
use super::browse::{Action, scroll_to};
use super::playlists_page::PlAction;
use super::theme::Theme;

/// Something the library holds that can be played: what the menu is for.
#[derive(Debug, Clone)]
pub enum Playable {
    Track {
        id: i64,
        favourite: bool,
        /// Whether its file is on this device.
        on_disk: bool,
        album: Option<i64>,
        /// The artists its credit links, from `credited`.
        artists: Vec<(i64, String)>,
    },
    Album {
        id: i64,
        tracks: Vec<i64>,
        favourite: bool,
        /// The artists its credit links, from `credited`.
        artists: Vec<(i64, String)>,
    },
    Artist {
        id: i64,
        tracks: Vec<i64>,
        favourite: bool,
    },
    /// Several tracks picked together.
    Tracks(Vec<i64>),
}

/// A rule between groups of items: no label, nothing to do.
fn rule() -> (String, Action) {
    (String::new(), Action::None)
}

/// The artists `credit` links, in credit order: id and the name as the
/// credit spells it. A credit naming one artist, or none, links `filed`.
/// Read when a menu opens, for the one thing it is about.
pub fn credited(of: CreditsOf, id: i64, credit: &str, filed: Option<i64>) -> Vec<(i64, String)> {
    let artists = koan_core::db::pool::shared()
        .get()
        .ok()
        .and_then(|db| credits_by_id(&db.conn, of, &[id]).ok())
        .and_then(|mut by| by.remove(&id))
        .unwrap_or_default();
    credit_parts(credit, filed, &artists)
        .into_iter()
        .filter_map(|p| Some((p.artist_id?, p.text)))
        .collect()
}

/// A way to each artist, as the apps' menu offers them: "Go to artist" for
/// one, and for several an item each, named.
pub fn go_to_artists(artists: &[(i64, String)]) -> Vec<(String, Action)> {
    artists
        .iter()
        .map(|(id, name)| {
            let label = if artists.len() == 1 {
                "Go to artist".to_string()
            } else {
                format!("Go to {name}")
            };
            (label, Action::OpenArtist(*id))
        })
        .collect()
}

fn is_rule<T>(item: &(String, T)) -> bool {
    item.0.is_empty()
}

/// The menu for something playable, as the apps' menu has it: playing it,
/// a playlist, favouring and downloading, then where it leads.
pub fn playable(thing: &Playable) -> Vec<(String, Action)> {
    use PickerAction::{Append, PlayNext, ReplaceQueue};
    let ids = match thing {
        Playable::Track { id, .. } => vec![*id],
        Playable::Album { tracks, .. }
        | Playable::Artist { tracks, .. }
        | Playable::Tracks(tracks) => tracks.clone(),
    };
    let play = |label: &str, how| (label.to_string(), Action::Play(ids.clone(), how));
    if let Playable::Tracks(_) = thing {
        return vec![
            play("Play", ReplaceQueue),
            play("Play next", PlayNext),
            play("Add to queue", Append),
            rule(),
            ("Download to cache".into(), Action::Download(ids.clone())),
            (
                "Remove downloaded files".into(),
                Action::RemoveDownload(ids.clone()),
            ),
        ];
    }
    let mut items = vec![
        play("Play", ReplaceQueue),
        play("Play next", PlayNext),
        play("Add to queue", Append),
    ];
    if !matches!(thing, Playable::Track { .. }) {
        items.push(("Shuffle".into(), Action::Shuffle(ids.clone())));
    }
    items.extend([
        rule(),
        (
            "Add to playlist\u{2026}".into(),
            Action::Choose(ids.clone()),
        ),
        rule(),
    ]);
    let unfavourite = |on: bool, noun: &str| {
        if on {
            "Remove favourite".to_string()
        } else {
            format!("Favourite {noun}")
        }
    };
    match thing {
        &Playable::Track {
            id,
            favourite,
            on_disk,
            album,
            ref artists,
        } => {
            items.push((unfavourite(favourite, "track"), Action::Favourite(id)));
            items.push(if on_disk {
                (
                    "Remove downloaded file".into(),
                    Action::RemoveDownload(ids.clone()),
                )
            } else {
                ("Download to cache".into(), Action::Download(ids.clone()))
            });
            items.push(("Track info".into(), Action::Info(id)));
            if album.is_some() || !artists.is_empty() {
                items.push(rule());
            }
            if let Some(album) = album {
                items.push(("Go to album".into(), Action::OpenAlbum(album)));
            }
            items.extend(go_to_artists(artists));
        }
        &Playable::Album {
            id,
            favourite,
            ref artists,
            ..
        } => {
            items.extend([
                (unfavourite(favourite, "album"), Action::ToggleAlbum(id)),
                (
                    "Download all to cache".into(),
                    Action::Download(ids.clone()),
                ),
                (
                    "Remove downloaded files".into(),
                    Action::RemoveDownload(ids.clone()),
                ),
                rule(),
                ("Go to album".into(), Action::OpenAlbum(id)),
            ]);
            items.extend(go_to_artists(artists));
        }
        Playable::Tracks(_) => unreachable!("answered above"),
        &Playable::Artist { id, favourite, .. } => {
            items.extend([
                (unfavourite(favourite, "artist"), Action::ToggleArtist(id)),
                (
                    "Download all to cache".into(),
                    Action::Download(ids.clone()),
                ),
                (
                    "Remove downloaded files".into(),
                    Action::RemoveDownload(ids.clone()),
                ),
                rule(),
                ("Go to artist".into(), Action::OpenArtist(id)),
            ]);
        }
    }
    items
}

/// What a menu item does: an action of the browsing pages or of a
/// playlist's page.
pub enum Item {
    Browse(Action),
    Playlist(PlAction),
}

pub struct ActionsMenu {
    pub title: String,
    pub items: Vec<(String, Item)>,
    pub cursor: usize,
    /// The first item shown.
    scroll: Cell<usize>,
}

impl ActionsMenu {
    pub fn new(title: String, items: Vec<(String, Item)>) -> Self {
        Self {
            title,
            items,
            cursor: 0,
            scroll: Cell::default(),
        }
    }

    /// Moves the cursor `delta` items, past the rules, wrapping.
    pub fn move_by(&mut self, delta: isize) {
        let n = self.items.len() as isize;
        for _ in 0..n {
            self.cursor = (self.cursor as isize + delta).rem_euclid(n) as usize;
            if !is_rule(&self.items[self.cursor]) {
                break;
            }
        }
    }

    /// The first item shown when `rows` fit.
    pub fn top(&self, rows: usize) -> usize {
        scroll_to(&self.scroll, self.cursor, rows)
    }
}

pub fn menu_rect(area: Rect, items: &[(String, Item)], title: &str) -> Rect {
    let widest = items
        .iter()
        .map(|(l, _)| l.chars().count())
        .chain([title.chars().count() + 2])
        .max()
        .unwrap_or(10) as u16;
    let w = (widest + 4)
        .min(area.width.saturating_sub(2))
        .max(10.min(area.width));
    let h = (items.len() as u16 + 2)
        .min(area.height.saturating_sub(2))
        .max(3.min(area.height));
    Rect::new(
        area.x + area.width.saturating_sub(w) / 2,
        area.y + area.height.saturating_sub(h) / 2,
        w,
        h,
    )
}

pub struct ActionsMenuOverlay<'a> {
    menu: &'a ActionsMenu,
    theme: &'a Theme,
}

impl<'a> ActionsMenuOverlay<'a> {
    pub fn new(menu: &'a ActionsMenu, theme: &'a Theme) -> Self {
        Self { menu, theme }
    }
}

impl Widget for ActionsMenuOverlay<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let popup = menu_rect(area, &self.menu.items, &self.menu.title);
        super::shell::clear(buf, popup);
        let block = super::shell::frame(&self.menu.title, self.theme);
        let inner = block.inner(popup);
        block.render(popup, buf);
        let top = self.menu.top(inner.height as usize);
        for (row, (i, (label, _))) in self
            .menu
            .items
            .iter()
            .enumerate()
            .skip(top)
            .take(inner.height as usize)
            .enumerate()
        {
            let y = inner.y + row as u16;
            if label.is_empty() {
                for x in inner.x..inner.x + inner.width {
                    buf[(x, y)].set_char('\u{2500}').set_style(self.theme.rule);
                }
                continue;
            }
            let style = if i == self.menu.cursor {
                for x in inner.x..inner.x + inner.width {
                    buf[(x, y)].set_style(self.theme.picker_cursor);
                }
                self.theme.picker_cursor
            } else {
                self.theme.track_normal
            };
            buf.set_stringn(
                inner.x + 1,
                y,
                label,
                inner.width.saturating_sub(2) as usize,
                style,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn menu() -> ActionsMenu {
        ActionsMenu::new(
            "record".into(),
            [
                (
                    "Play".to_string(),
                    Action::Play(vec![1], super::super::app::PickerAction::AppendAndPlay),
                ),
                ("Favourite".into(), Action::ToggleAlbum(1)),
                ("Info".into(), Action::Info(1)),
            ]
            .into_iter()
            .map(|(l, a)| (l, Item::Browse(a)))
            .collect(),
        )
    }

    #[test]
    fn the_cursor_wraps() {
        let mut m = menu();
        m.move_by(-1);
        assert_eq!(m.cursor, 2);
        m.move_by(1);
        assert_eq!(m.cursor, 0);
    }

    #[test]
    fn it_draws_at_any_size() {
        let m = menu();
        let theme = Theme::new(super::super::theme::Palette::Ansi, (125, 211, 167));
        for w in 0..60 {
            for h in 0..12 {
                let area = Rect::new(0, 0, w, h);
                let mut buf = Buffer::empty(area);
                ActionsMenuOverlay::new(&m, &theme).render(area, &mut buf);
            }
        }
    }

    #[test]
    fn the_cursor_steps_over_rules() {
        let items = playable(&Playable::Album {
            id: 1,
            tracks: vec![1, 2],
            favourite: false,
            artists: vec![(3, "a".into())],
        });
        let mut m = ActionsMenu::new(
            "x".into(),
            items
                .into_iter()
                .map(|(l, a)| (l, Item::Browse(a)))
                .collect(),
        );
        let labels: Vec<&str> = m.items.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(
            &labels[..5],
            ["Play", "Play next", "Add to queue", "Shuffle", ""]
        );
        m.cursor = 3;
        m.move_by(1);
        assert_eq!(m.items[m.cursor].0, "Add to playlist\u{2026}");
        m.move_by(-1);
        assert_eq!(m.cursor, 3);
    }

    #[test]
    fn a_track_offers_the_download_it_can_use() {
        let menu = |on_disk| -> Vec<String> {
            playable(&Playable::Track {
                id: 1,
                favourite: true,
                on_disk,
                album: None,
                artists: Vec::new(),
            })
            .into_iter()
            .map(|(l, _)| l)
            .collect()
        };
        assert!(menu(true).contains(&"Remove downloaded file".to_string()));
        assert!(menu(false).contains(&"Download to cache".to_string()));
        assert!(menu(false).contains(&"Remove favourite".to_string()));
        assert!(!menu(false).contains(&"Shuffle".to_string()));
        assert!(
            !menu(false).last().is_some_and(|l| l.is_empty()),
            "no rule with nothing after it"
        );
    }

    #[test]
    fn a_credit_naming_several_artists_offers_each() {
        let menu = |artists: Vec<(i64, String)>| -> Vec<(String, Action)> {
            playable(&Playable::Album {
                id: 1,
                tracks: vec![1],
                favourite: false,
                artists,
            })
            .into_iter()
            .skip_while(|(l, _)| l != "Go to album")
            .skip(1)
            .collect()
        };
        assert_eq!(
            menu(vec![(3, "Solo".into())]),
            [("Go to artist".to_string(), Action::OpenArtist(3))]
        );
        assert_eq!(
            menu(vec![(3, "A".into()), (4, "B".into())]),
            [
                ("Go to A".to_string(), Action::OpenArtist(3)),
                ("Go to B".to_string(), Action::OpenArtist(4)),
            ]
        );
    }

    #[test]
    fn several_tracks_get_the_apps_selection_menu() {
        let labels: Vec<String> = playable(&Playable::Tracks(vec![1, 2]))
            .into_iter()
            .map(|(l, _)| l)
            .collect();
        assert_eq!(
            labels,
            [
                "Play",
                "Play next",
                "Add to queue",
                "",
                "Download to cache",
                "Remove downloaded files"
            ]
        );
    }
}
