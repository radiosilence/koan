//! Records as tiles: the album grid, an artist's records, a shelf's.

use askama::Template;
use koan_core::db::queries::AlbumRow;

use super::covers::Versions;
use super::credit::{Credit, Credits};
use super::heart::Hearts;
use crate::html::{Html, Render};
use crate::ui::paths;

struct Tile<'a> {
    album: &'a AlbumRow,
    href: String,
    src: String,
    artist: Html,
    heart: Html,
}

/// Record tiles, each with its heart over the corner where the account may
/// favourite.
#[derive(Template)]
#[template(path = "parts/tiles.html")]
pub(in crate::ui) struct Tiles<'a> {
    tiles: Vec<Tile<'a>>,
}

impl<'a> Tiles<'a> {
    pub(in crate::ui) fn of(
        albums: &'a [AlbumRow],
        versions: &Versions,
        credits: &Credits,
        hearts: Option<&Hearts>,
    ) -> Html {
        let tiles = albums
            .iter()
            .map(|a| Tile {
                album: a,
                href: paths::album(a.id),
                src: versions.url(a.id, crate::covers::GRID),
                artist: Credit::quiet(&credits.album(Some(a.id), &a.artist_name, Some(a.artist_id))),
                heart: hearts.map(|h| h.tile(a.id)).unwrap_or_default(),
            })
            .collect();
        Tiles { tiles }.html()
    }

    fn size(&self) -> u32 {
        crate::covers::GRID
    }
}
