//! Credits as koan-core builds them, as the TUI draws them: as text, beside
//! the decision whether a row under its record names its artists. Read when a
//! list loads, never per frame.

use std::collections::HashMap;

use koan_core::db::queries::{AlbumRow, CreditsOf, TrackRow};
use koan_core::helpers::{ListCredits, RowCredits, credit_text, credits_by_id, record_credit};
use rusqlite::Connection;

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

/// The credits of library tracks, by track id.
pub fn of_tracks<'a>(
    conn: &Connection,
    rows: impl IntoIterator<Item = &'a TrackRow>,
) -> HashMap<i64, Credits> {
    let rows: Vec<&TrackRow> = rows.into_iter().collect();
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    let (places, credits) = ListCredits::read_tracks(conn, &ids);
    rows.into_iter()
        .map(|r| {
            let place = places.get(&r.id).copied().unwrap_or_default();
            let row = credits.row(Some(r.id), place, &r.artist_name, &r.album_artist_name);
            (r.id, Credits::of(row))
        })
        .collect()
}

/// The credits of albums, as text, by album id.
pub fn of_albums<'a>(
    conn: &Connection,
    rows: impl IntoIterator<Item = &'a AlbumRow>,
) -> HashMap<i64, String> {
    let rows: Vec<&AlbumRow> = rows.into_iter().collect();
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    let credited = credits_by_id(conn, CreditsOf::Albums, &ids).unwrap_or_default();
    rows.into_iter()
        .map(|r| {
            let artists = credited.get(&r.id).map_or(&[][..], Vec::as_slice);
            let credit = record_credit(&r.artist_name, Some(r.artist_id), artists);
            (r.id, credit_text(&credit))
        })
        .collect()
}
