//! A shelf's previews, as the apps' shelf lays them out: artists as pills,
//! records as tiles, and tracks as a list that plays on from the row picked.
//! Each section's heading links to the browser with the shelf as its filter
//! and counts what the shelf has in all; the preview may show fewer.
//! Favourites, Recently played and Search each draw one.

use askama::Template;
use koan_core::db::queries::{self, AlbumRow};
use koan_core::shelves::{self, Shelf, Summary};
use koan_kit_web as kit;

use super::browse::{Kind, shelf_browser};
use super::covers::Versions;
use super::credit::Credits;
use super::heart::Hearts;
use super::tile::Tiles;
use super::track::{Lead, Row, Rows};
use crate::auth::AuthUser;
use crate::html::{Html, Render};
use crate::ui::{Logged, UiState, open, paths};

/// A shelf as read for one account: its previews and what draws them.
pub(in crate::ui) struct Read {
    summary: Summary,
    versions: Versions,
    credits: Credits,
    hearts: Option<Hearts>,
}

impl Read {
    /// `shelf` for `user`, as it stands. Blocking.
    pub(in crate::ui) fn of(s: &UiState, user: &AuthUser, shelf: Shelf) -> Option<Self> {
        let db = open(&s.pool)?;
        let summary =
            shelves::summary(&db.conn, shelf, user.user_id, shelves::now(), false).logged()?;
        let mut versions = Versions::of_albums(&db.conn, &summary.albums.preview);
        versions.extend(Versions::of_tracks(&db.conn, &summary.tracks.preview));
        let credits = Credits::load(
            &db.conn,
            &summary.tracks.preview,
            &summary.albums.preview.iter().map(|a| a.id).collect::<Vec<_>>(),
        );
        Some(Self {
            hearts: Hearts::load(&db.conn, user),
            summary,
            versions,
            credits,
        })
    }

    pub(in crate::ui) fn is_empty(&self) -> bool {
        self.summary.is_empty()
    }

    /// The previews, drawn.
    pub(in crate::ui) fn draw(&self, shelf: Shelf) -> Html {
        let s = &self.summary;
        let rows = Rows {
            versions: &self.versions,
            credits: &self.credits,
            hearts: self.hearts.as_ref(),
        };
        let browser = |kind| shelf_browser(shelf, kind);
        Sections {
            artists: (!s.artists.preview.is_empty()).then(|| {
                (
                    s.artists.total,
                    browser(Kind::Artists),
                    &s.artists.preview[..],
                )
            }),
            albums: (!s.albums.preview.is_empty()).then(|| {
                (
                    s.albums.total,
                    browser(Kind::Albums),
                    tiles(&s.albums.preview, &rows),
                )
            }),
            tracks: (!s.tracks.preview.is_empty()).then(|| {
                let html = s
                    .tracks
                    .preview
                    .iter()
                    .enumerate()
                    .map(|(i, t)| Row::new(t, Lead::Number(i + 1), &rows, true, true).html())
                    .collect();
                (s.tracks.total, browser(Kind::Tracks), html)
            }),
        }
        .html()
    }
}

fn tiles(albums: &[AlbumRow], rows: &Rows) -> Html {
    Tiles::of(albums, rows.versions, rows.credits, rows.hearts)
}

#[derive(Template)]
#[template(path = "parts/shelf.html")]
struct Sections<'a> {
    /// Each section's total, the browser it links to and its preview, where
    /// it has a preview to show.
    artists: Option<(u64, String, &'a [queries::ArtistRow])>,
    albums: Option<(u64, String, Html)>,
    tracks: Option<(u64, String, Html)>,
}

impl Sections<'_> {
    fn artist(&self, id: &i64) -> String {
        paths::artist(*id)
    }
}
