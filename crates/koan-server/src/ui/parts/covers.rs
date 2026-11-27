//! Covers as a page asks for them: each record's cover version is when its
//! files last changed, and a cover's address carries it, so the address
//! changes whenever the art might have and the browser can keep each one for
//! good. One query for a page of records.

use std::collections::HashMap;

use koan_core::db::queries::{AlbumRow, TrackRow};

use crate::ui::paths;

/// Each record's cover version.
#[derive(Default)]
pub(in crate::ui) struct Versions(HashMap<i64, i64>);

impl Versions {
    /// The versions of `albums`' covers.
    pub(in crate::ui) fn of(conn: &rusqlite::Connection, album_ids: &[i64]) -> Self {
        if album_ids.is_empty() {
            return Self::default();
        }
        let sql = format!(
            "SELECT album_id, MAX(COALESCE(mtime, 0)) FROM tracks WHERE album_id IN ({}) GROUP BY album_id",
            vec!["?"; album_ids.len()].join(",")
        );
        let Ok(mut stmt) = conn.prepare(&sql) else {
            return Self::default();
        };
        Self(
            stmt.query_map(rusqlite::params_from_iter(album_ids), |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default(),
        )
    }

    pub(in crate::ui) fn of_albums(conn: &rusqlite::Connection, albums: &[AlbumRow]) -> Self {
        Self::of(conn, &albums.iter().map(|a| a.id).collect::<Vec<_>>())
    }

    pub(in crate::ui) fn of_tracks(conn: &rusqlite::Connection, tracks: &[TrackRow]) -> Self {
        let mut ids: Vec<i64> = tracks.iter().filter_map(|t| t.album_id).collect();
        ids.sort_unstable();
        ids.dedup();
        Self::of(conn, &ids)
    }

    pub(in crate::ui) fn extend(&mut self, other: Self) {
        self.0.extend(other.0);
    }

    /// `album`'s cover at `size`.
    pub(in crate::ui) fn url(&self, album: i64, size: u32) -> String {
        paths::cover(album, size, self.0.get(&album).copied().unwrap_or(0))
    }
}
