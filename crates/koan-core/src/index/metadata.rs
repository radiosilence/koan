use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

use lofty::file::AudioFile;
use lofty::prelude::*;
use thiserror::Error;

use crate::db::queries::TrackMeta;

#[derive(Debug, Error)]
pub enum MetadataError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("tag error: {0}")]
    Tag(#[from] lofty::error::LoftyError),
}

/// Audio file extensions we care about.
const AUDIO_EXTENSIONS: &[&str] = &[
    "flac", "mp3", "m4a", "aac", "ogg", "opus", "wv", "wav", "aiff", "aif", "alac", "ape",
];

/// Check if a path has a supported audio extension.
pub fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| AUDIO_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
}

/// Read metadata from an audio file, returning a TrackMeta ready for DB insertion.
pub fn read_metadata(path: &Path) -> Result<TrackMeta, MetadataError> {
    let tagged_file = lofty::read_from_path(path)?;

    let properties = tagged_file.properties();
    let duration_ms = properties.duration().as_millis() as i64;
    let sample_rate = properties.sample_rate().map(|r| r as i32);
    let bit_depth = properties.bit_depth().map(|b| b as i32);
    let channels = properties.channels().map(|c| c as i32);
    let bitrate = properties.audio_bitrate().map(|b| b as i32);

    let tag = tagged_file
        .primary_tag()
        .or_else(|| tagged_file.first_tag());

    let (title, artist, album_artist, album, date, disc, track_number, genre, label) =
        if let Some(tag) = tag {
            (
                tag.title().map(|s| s.to_string()),
                tag.artist().map(|s| s.to_string()),
                tag.get_string(ItemKey::AlbumArtist).map(|s| s.to_string()),
                tag.album().map(|s| s.to_string()),
                // lofty 0.23 removed year() — use TrackDate or RecordingDate.
                tag.get_string(ItemKey::Year)
                    .or_else(|| tag.get_string(ItemKey::RecordingDate))
                    .map(|s| s.to_string()),
                tag.disk().map(|d| d as i32),
                tag.track().map(|t| t as i32),
                tag.genre().map(|s| s.to_string()),
                tag.get_string(ItemKey::Label).map(|s| s.to_string()),
            )
        } else {
            (None, None, None, None, None, None, None, None, None)
        };

    let title = title.unwrap_or_else(|| {
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Unknown")
            .to_string()
    });
    let artist = artist.unwrap_or_else(|| "Unknown Artist".to_string());
    let album = album.unwrap_or_else(|| "Unknown Album".to_string());

    let file_meta = fs::metadata(path)?;
    let size_bytes = file_meta.len() as i64;
    let mtime = file_meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64);

    let codec = codec_from_file_type(tagged_file.file_type());

    Ok(TrackMeta {
        title,
        artist,
        album_artist,
        album,
        date,
        disc,
        track_number,
        genre,
        label,
        duration_ms: Some(duration_ms),
        codec: Some(codec),
        sample_rate,
        bit_depth,
        channels,
        bitrate,
        size_bytes: Some(size_bytes),
        mtime,
        path: Some(path.to_string_lossy().to_string()),
        source: "local".to_string(),
        remote_id: None,
        remote_url: None,
    })
}

fn codec_from_file_type(ft: lofty::file::FileType) -> String {
    match ft {
        lofty::file::FileType::Flac => "FLAC",
        lofty::file::FileType::Mpeg => "MP3",
        lofty::file::FileType::Mp4 => "AAC",
        lofty::file::FileType::Opus => "Opus",
        lofty::file::FileType::Vorbis => "Vorbis",
        lofty::file::FileType::WavPack => "WavPack",
        lofty::file::FileType::Wav => "WAV",
        lofty::file::FileType::Aiff => "AIFF",
        lofty::file::FileType::Ape => "APE",
        _ => "Unknown",
    }
    .to_string()
}
