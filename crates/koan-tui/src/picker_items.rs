use std::sync::Arc;

use koan_core::db::queries;

use crate::picker::{PickerItem, PickerKind, PickerPartKind};

/// How long a kind's items are kept while the library holds the same rows. Its
/// fingerprint sees rows added and removed, not rows edited in place.
const MAX_AGE_SECS: u64 = 300;

/// A kind's items, and the library they were read from.
type Cached = (PickerKind, u64, Arc<Vec<PickerItem>>);

static CACHE: parking_lot::Mutex<Vec<Cached>> = parking_lot::Mutex::new(Vec::new());

fn format_time(ms: u64) -> String {
    let secs = ms / 1000;
    let mins = secs / 60;
    let secs = secs % 60;
    format!("{}:{:02}", mins, secs)
}

/// The items a picker of `kind` lists, read once per library.
///
/// The whole library, in library order: the picker lists it before anything
/// is typed. Too slow for the render thread, so the caller loads it off it.
pub fn load_picker_items(kind: PickerKind) -> Vec<PickerItem> {
    let db = match koan_core::db::pool::shared().get() {
        Ok(db) => db,
        Err(e) => {
            log::error!("db error: {}", e);
            return Vec::new();
        }
    };
    let window = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        / MAX_AGE_SECS;
    let version = queries::library_fingerprint(&db.conn).unwrap_or_default() ^ window;
    if let Some((_, _, items)) = CACHE
        .lock()
        .iter()
        .find(|(k, v, _)| *k == kind && *v == version)
    {
        return items.as_ref().clone();
    }
    let items = Arc::new(read_picker_items(&db, kind));
    let mut cache = CACHE.lock();
    cache.retain(|(k, _, _)| *k != kind);
    cache.push((kind, version, Arc::clone(&items)));
    items.as_ref().clone()
}

fn read_picker_items(
    db: &koan_core::db::connection::Database,
    kind: PickerKind,
) -> Vec<PickerItem> {
    let conn = &db.conn;
    match kind {
        PickerKind::Track => {
            let tracks = queries::all_tracks(conn).unwrap_or_default();
            make_track_picker_items(&tracks)
        }
        PickerKind::Album => {
            let albums = queries::all_albums(conn).unwrap_or_default();
            make_album_picker_items(&albums)
        }
        PickerKind::Artist => {
            let artists = queries::all_artists(conn).unwrap_or_default();
            make_artist_picker_items(&artists)
        }
        // QueueJump items are created eagerly in app.rs, never lazy-loaded.
        PickerKind::QueueJump => vec![],
    }
}

pub fn make_track_picker_items(tracks: &[queries::TrackRow]) -> Vec<PickerItem> {
    tracks
        .iter()
        .map(|t| {
            let dur = t
                .duration_ms
                .map(|d| format_time(d as u64))
                .unwrap_or_default();
            let track_num = match (t.disc, t.track_number) {
                (Some(d), Some(n)) if d > 1 => format!("{}.{:02}", d, n),
                (_, Some(n)) => format!("{:02}", n),
                _ => "  ".into(),
            };
            let mut parts = vec![
                (format!("{} ", track_num), PickerPartKind::TrackNum),
                (t.artist_name.clone(), PickerPartKind::Artist),
                (" - ".into(), PickerPartKind::Separator),
                (t.title.clone(), PickerPartKind::Title),
            ];
            if !dur.is_empty() {
                parts.push((format!(" {}", dur), PickerPartKind::Duration));
            }
            PickerItem {
                id: t.id,
                display: format!("{} {} - {} {}", track_num, t.artist_name, t.title, dur),
                match_text: format!("{} {} {}", t.artist_name, t.album_title, t.title),
                parts,
            }
        })
        .collect()
}

pub fn make_album_picker_items(albums: &[queries::AlbumRow]) -> Vec<PickerItem> {
    albums
        .iter()
        .map(|a| {
            let year = crate::library::album_year(a.date.as_deref());
            let codec = a.codec.as_deref();
            let mut parts = vec![
                (a.artist_name.clone(), PickerPartKind::Artist),
                (" - ".into(), PickerPartKind::Separator),
            ];
            if let Some(ref y) = year {
                parts.push((format!("({}) ", y), PickerPartKind::Date));
            }
            parts.push((a.title.clone(), PickerPartKind::Album));
            if let Some(c) = codec {
                parts.push((format!(" [{}]", c), PickerPartKind::Codec));
            }
            let year_str = year.map(|y| format!("({}) ", y)).unwrap_or_default();
            let codec_str = codec.map(|c| format!(" [{}]", c)).unwrap_or_default();
            PickerItem {
                id: a.id,
                display: format!("{} - {}{}{}", a.artist_name, year_str, a.title, codec_str),
                match_text: format!("{} {}", a.artist_name, a.title),
                parts,
            }
        })
        .collect()
}

pub fn make_artist_picker_items(artists: &[queries::ArtistRow]) -> Vec<PickerItem> {
    artists
        .iter()
        .map(|a| PickerItem {
            id: a.id,
            display: a.name.clone(),
            match_text: a.name.clone(),
            parts: vec![(a.name.clone(), PickerPartKind::Artist)],
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn album(date: &str) -> queries::AlbumRow {
        queries::AlbumRow {
            id: 1,
            title: "album".into(),
            artist_id: 1,
            artist_name: "artist".into(),
            date: Some(date.into()),
            total_discs: None,
            total_tracks: None,
            codec: None,
            label: None,
            remote_id: None,
            added_at: None,
        }
    }

    #[test]
    fn multibyte_album_dates_do_not_split_a_char() {
        for date in [
            "\u{65e5}\u{672c}\u{8a9e}",
            "\u{ff12}\u{ff10}\u{ff12}\u{ff14}",
            "",
            "20",
            "2024-05-01",
        ] {
            make_album_picker_items(&[album(date)]);
        }
    }
}
