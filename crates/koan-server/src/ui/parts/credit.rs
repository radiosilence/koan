//! Credits: the artists credited on a page's tracks and records, read once
//! for the page as koan-core builds every credit, and drawn with each
//! credited artist's name linked to their page.

use std::collections::HashMap;

use askama::Template;
use koan_core::db::queries::TrackRow;
use koan_core::db::queries::batch::CreditsOf;
use koan_core::helpers::{CreditPart, ListCredits, RowCredits, credits_by_id, record_credit};

use crate::html::{Html, Render};
use crate::ui::paths;

/// The artists credited on a page's tracks and albums. Where the read fails
/// each credit is one link, to the artist it is filed under.
#[derive(Default)]
pub(in crate::ui) struct Credits {
    list: ListCredits,
    /// The albums the page lists as albums, beside its tracks' records.
    albums: HashMap<i64, Vec<(i64, String)>>,
}

impl Credits {
    pub(in crate::ui) fn load(conn: &rusqlite::Connection, tracks: &[TrackRow], albums: &[i64]) -> Self {
        let list = ListCredits::read_rows(conn, tracks);
        // The tracks' records are read with them.
        let records: std::collections::HashSet<i64> =
            tracks.iter().filter_map(|t| t.album_id).collect();
        let albums: Vec<i64> = albums
            .iter()
            .copied()
            .filter(|a| !records.contains(a))
            .collect();
        Self {
            list,
            albums: credits_by_id(conn, CreditsOf::Albums, &albums).unwrap_or_default(),
        }
    }

    /// A track's credit and its record's.
    pub(in crate::ui) fn row(&self, t: &TrackRow) -> RowCredits {
        self.list.track(t)
    }

    /// A track's artist credit.
    pub(in crate::ui) fn track(&self, t: &TrackRow) -> Vec<CreditPart> {
        self.row(t).artist
    }

    /// An album's artist credit, `credit` as the row names it.
    pub(in crate::ui) fn album(&self, id: Option<i64>, credit: &str, filed: Option<i64>) -> Vec<CreditPart> {
        let artists = id
            .and_then(|id| self.albums.get(&id).or_else(|| self.list.albums.get(&id)))
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        record_credit(credit, filed, artists)
    }
}

/// A credit as it reads, each credited artist's name a link to their page and
/// the text joining them plain. `quiet` links are left out of the tab order,
/// as beside a tile's own link.
#[derive(Template)]
#[template(path = "parts/credit.html")]
pub(in crate::ui) struct Credit<'a> {
    parts: &'a [CreditPart],
    quiet: bool,
}

impl<'a> Credit<'a> {
    pub(in crate::ui) fn links(parts: &'a [CreditPart]) -> Html {
        Self { parts, quiet: false }.html()
    }

    pub(in crate::ui) fn quiet(parts: &'a [CreditPart]) -> Html {
        Self { parts, quiet: true }.html()
    }

    fn href(&self, artist: &i64) -> String {
        paths::artist(*artist)
    }
}

/// A name linked to its page, or the name alone where the row has no id for
/// it.
#[derive(Template)]
#[template(path = "parts/link.html")]
pub(in crate::ui) struct Link<'a> {
    href: Option<String>,
    name: &'a str,
}

impl<'a> Link<'a> {
    pub(in crate::ui) fn album(id: Option<i64>, name: &'a str) -> Html {
        Self {
            href: id.map(paths::album),
            name,
        }
        .html()
    }
}

/// A row's credit for the player, which draws the queue's Now Playing from
/// the row's data: `[text, address or ""]` for each part, as JSON.
pub(in crate::ui) fn for_player(parts: &[CreditPart]) -> String {
    let parts: Vec<_> = parts
        .iter()
        .map(|p| {
            serde_json::json!([p.text, p.artist_id.map(paths::artist).unwrap_or_default()])
        })
        .collect();
    serde_json::Value::Array(parts).to_string()
}
