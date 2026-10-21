//! Credits as koan-core builds them, as the TUI draws them: as text, beside
//! the decision whether a row under its record names its artists. Read when a
//! list loads, never per frame.

use std::collections::HashMap;

use koan_core::db::connection::Database;
use koan_core::db::queries::{AlbumRow, CreditsOf, TrackRow};
use koan_core::helpers::{ListCredits, RowCredits, credit_text, credits_by_id, record_credit};

/// A row's credit and its record's, as text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credits {
    pub artist: String,
    pub album_artist: String,
    /// The track's credit reads differently from its record's
    /// (`RowCredits::differ`): under the record's heading or on its page, the
    /// row names its artists only then.
    pub differ: bool,
}

impl Credits {
    pub fn of(row: RowCredits) -> Self {
        Self {
            differ: row.differ(),
            artist: credit_text(&row.artist),
            album_artist: credit_text(&row.album_artist),
        }
    }
}

/// A track's credits from its tags alone, as before its credits are read.
pub fn of_tags(t: &TrackRow) -> Credits {
    Credits::of(RowCredits::new::<&str>(
        &t.artist_name,
        &t.album_artist_name,
        Default::default(),
        None,
        &[],
    ))
}

/// The credits of library tracks, by track id.
pub fn of_tracks<'a>(
    db: &Database,
    rows: impl IntoIterator<Item = &'a TrackRow>,
) -> HashMap<i64, Credits> {
    let rows: Vec<&TrackRow> = rows.into_iter().collect();
    let credits = ListCredits::read_rows(&db.conn, rows.iter().copied());
    rows.into_iter()
        .map(|r| (r.id, Credits::of(credits.track(r))))
        .collect()
}

/// The credits of albums, as text, by album id.
pub fn of_albums<'a>(
    db: &Database,
    rows: impl IntoIterator<Item = &'a AlbumRow>,
) -> HashMap<i64, String> {
    let rows: Vec<&AlbumRow> = rows.into_iter().collect();
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    let credited = credits_by_id(&db.conn, CreditsOf::Albums, &ids).unwrap_or_default();
    rows.into_iter()
        .map(|r| {
            let artists = credited.get(&r.id).map_or(&[][..], Vec::as_slice);
            let credit = record_credit(&r.artist_name, Some(r.artist_id), artists);
            (r.id, credit_text(&credit))
        })
        .collect()
}

/// The credits of a listing's tracks and albums, read with the listing.
#[derive(Debug, Default, Clone)]
pub struct Credited {
    pub tracks: HashMap<i64, Credits>,
    pub albums: HashMap<i64, String>,
}

impl Credited {
    pub fn read<'a>(
        db: &Database,
        tracks: impl IntoIterator<Item = &'a TrackRow>,
        albums: impl IntoIterator<Item = &'a AlbumRow>,
    ) -> Self {
        Self {
            tracks: of_tracks(db, tracks),
            albums: of_albums(db, albums),
        }
    }

    /// A track's artist as its credit reads: its tag until the credits are
    /// read.
    pub fn track<'a>(&'a self, t: &'a TrackRow) -> &'a str {
        self.tracks.get(&t.id).map_or(&t.artist_name, |c| &c.artist)
    }

    /// An album's artist as its credit reads: its tag until the credits are
    /// read.
    pub fn album<'a>(&'a self, a: &'a AlbumRow) -> &'a str {
        self.albums
            .get(&a.id)
            .map_or(&a.artist_name, String::as_str)
    }
}
