use koan_core::db::queries;

use super::{format_time, open_db};
use crate::tui::picker::{PickerItem, PickerKind, PickerPartKind};

pub fn load_picker_items(kind: PickerKind) -> Vec<PickerItem> {
    let db = open_db();
    match kind {
        PickerKind::Track => {
            let tracks = queries::all_tracks(&db.conn).unwrap_or_default();
            make_track_picker_items(&tracks)
        }
        PickerKind::Album => {
            let albums = queries::all_albums(&db.conn).unwrap_or_default();
            make_album_picker_items(&albums)
        }
        PickerKind::Artist => {
            let artists = queries::all_artists(&db.conn).unwrap_or_default();
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
            let year = a
                .date
                .as_deref()
                .and_then(|d| if d.len() >= 4 { Some(&d[..4]) } else { None });
            let codec = a.codec.as_deref();
            let mut parts = vec![
                (a.artist_name.clone(), PickerPartKind::Artist),
                (" - ".into(), PickerPartKind::Separator),
            ];
            if let Some(y) = year {
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
