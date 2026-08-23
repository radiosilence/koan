use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

use lofty::config::ParseOptions;
use lofty::file::AudioFile;
use lofty::mp4::{Mp4Codec, Mp4File};
use lofty::prelude::*;
use symphonia::core::meta::{MetadataRevision, StandardTag};
use thiserror::Error;

use crate::db::queries::TrackMeta;

#[derive(Debug, Error)]
pub enum MetadataError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("tag error: {0}")]
    Tag(#[from] lofty::error::FileParseError),
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
///
/// If lofty fails to parse tags (e.g. corrupted UTF-16 ID3 frames), falls back
/// to Symphonia for duration/properties and infers what we can from the path.
pub fn read_metadata(path: &Path) -> Result<TrackMeta, MetadataError> {
    // Skip empty/tiny files — avoid confusing error messages from lofty/symphonia.
    match std::fs::metadata(path) {
        Ok(m) if m.len() == 0 => {
            return Err(MetadataError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "empty file (0 bytes) — may be a stale mount or incomplete transfer, will retry on next scan: {}",
                    path.display()
                ),
            )));
        }
        Err(e) => return Err(MetadataError::Io(e)),
        _ => {}
    }

    match lofty::read_from_path(path) {
        Ok(tagged_file) => read_metadata_lofty(path, &tagged_file),
        Err(e) => {
            log::warn!(
                "lofty failed for {}: {}; falling back to probe",
                path.display(),
                e
            );
            read_metadata_fallback(path)
        }
    }
}

/// Full metadata read via lofty (happy path).
fn read_metadata_lofty(
    path: &Path,
    tagged_file: &lofty::file::TaggedFile,
) -> Result<TrackMeta, MetadataError> {
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
                // Prefer the track date, falling back to the recording date.
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

    let codec = if tagged_file.file_type() == lofty::file::FileType::Mp4 {
        mp4_codec(path)
    } else {
        codec_string(tagged_file.file_type()).to_string()
    };

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
        album_added_at: None,
    })
}

/// Fallback metadata read when lofty fails. Uses Symphonia to probe
/// duration/codec/properties, and infers artist/album/title from the path.
fn read_metadata_fallback(path: &Path) -> Result<TrackMeta, MetadataError> {
    let file_meta = fs::metadata(path)?;
    let size_bytes = file_meta.len() as i64;
    let mtime = file_meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64);

    // Probe via Symphonia for duration, codec, and audio properties.
    let props = probe_symphonia(path);

    // Try to extract tags from Symphonia's metadata (it's more lenient than lofty
    // for corrupted frames — it skips bad frames instead of erroring).
    let (title, artist, album) = probe_symphonia_tags(path);

    let title = title.unwrap_or_else(|| {
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Unknown")
            .to_string()
    });
    let artist = artist.unwrap_or_else(|| "Unknown Artist".to_string());
    let album = album.unwrap_or_else(|| "Unknown Album".to_string());

    Ok(TrackMeta {
        title,
        artist,
        album_artist: None,
        album,
        date: None,
        disc: None,
        track_number: None,
        genre: None,
        label: None,
        duration_ms: props.duration_ms,
        codec: props.codec,
        sample_rate: props.sample_rate,
        bit_depth: props.bit_depth,
        channels: props.channels,
        bitrate: props.bitrate,
        size_bytes: Some(size_bytes),
        mtime,
        path: Some(path.to_string_lossy().to_string()),
        source: "local".to_string(),
        remote_id: None,
        remote_url: None,
        album_added_at: None,
    })
}

/// Probed audio properties from Symphonia.
struct SymphoniaProps {
    duration_ms: Option<i64>,
    sample_rate: Option<i32>,
    bit_depth: Option<i32>,
    channels: Option<i32>,
    bitrate: Option<i32>,
    codec: Option<String>,
}

/// Probe audio properties via Symphonia (duration, sample rate, codec, etc.).
fn probe_symphonia(path: &Path) -> SymphoniaProps {
    use symphonia::core::codecs::audio::CODEC_ID_NULL_AUDIO;
    use symphonia::core::formats::probe::Hint;
    use symphonia::core::formats::{FormatOptions, TrackType};
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;

    let empty = SymphoniaProps {
        duration_ms: None,
        sample_rate: None,
        bit_depth: None,
        channels: None,
        bitrate: None,
        codec: None,
    };

    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return empty,
    };
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let reader = match symphonia::default::get_probe().probe(
        &hint,
        mss,
        FormatOptions::default(),
        MetadataOptions::default(),
    ) {
        Ok(r) => r,
        Err(_) => return empty,
    };

    let track = match reader.default_track(TrackType::Audio) {
        Some(t) => t,
        None => return empty,
    };

    let params = match track.codec_params.as_ref().and_then(|p| p.audio()) {
        Some(p) => p,
        None => return empty,
    };
    let sample_rate = params.sample_rate.map(|r| r as i32);
    let bit_depth = params.bits_per_sample.map(|b| b as i32);
    let channels = params.channels.as_ref().map(|c| c.count() as i32);

    let duration_ms = params.sample_rate.and_then(|sr| {
        let ms = crate::audio::buffer::track_duration_ms(&*reader, track, sr);
        (ms > 0).then_some(ms as i64)
    });

    let bitrate = params.sample_rate.and_then(|sr| {
        params.bits_per_sample.and_then(|bps| {
            params
                .channels
                .as_ref()
                .map(|ch| (sr as i32 * bps as i32 * ch.count() as i32) / 1000)
        })
    });

    let codec = if params.codec != CODEC_ID_NULL_AUDIO {
        Some(symphonia_codec_name(params.codec))
    } else {
        None
    };

    SymphoniaProps {
        duration_ms,
        sample_rate,
        bit_depth,
        channels,
        bitrate,
        codec,
    }
}

/// Map Symphonia codec type to a human-readable string.
fn symphonia_codec_name(codec: symphonia::core::codecs::audio::AudioCodecId) -> String {
    use symphonia::core::codecs::audio::well_known as ids;
    match codec {
        ids::CODEC_ID_FLAC => "FLAC".to_string(),
        ids::CODEC_ID_MP3 => "MP3".to_string(),
        ids::CODEC_ID_AAC => "AAC".to_string(),
        ids::CODEC_ID_ALAC => "ALAC".to_string(),
        ids::CODEC_ID_VORBIS => "Vorbis".to_string(),
        ids::CODEC_ID_OPUS => "Opus".to_string(),
        ids::CODEC_ID_WAVPACK => "WavPack".to_string(),
        ids::CODEC_ID_PCM_S16LE
        | ids::CODEC_ID_PCM_S24LE
        | ids::CODEC_ID_PCM_S32LE
        | ids::CODEC_ID_PCM_F32LE
        | ids::CODEC_ID_PCM_F64LE
        | ids::CODEC_ID_PCM_S16BE
        | ids::CODEC_ID_PCM_S24BE
        | ids::CODEC_ID_PCM_S32BE
        | ids::CODEC_ID_PCM_F32BE
        | ids::CODEC_ID_PCM_F64BE
        | ids::CODEC_ID_PCM_U8 => "PCM".to_string(),
        _ => "Unknown".to_string(),
    }
}

/// Try to extract basic tags (title, artist, album) via Symphonia's metadata reader.
/// Symphonia is more lenient with corrupted ID3 frames than lofty.
fn probe_symphonia_tags(path: &Path) -> (Option<String>, Option<String>, Option<String>) {
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::formats::probe::Hint;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::{MetadataOptions, StandardTag};

    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return (None, None, None),
    };
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let mut reader = match symphonia::default::get_probe().probe(
        &hint,
        mss,
        FormatOptions::default(),
        MetadataOptions::default(),
    ) {
        Ok(r) => r,
        Err(_) => return (None, None, None),
    };

    let mut title = None;
    let mut artist = None;
    let mut album = None;

    // Metadata found while probing is queued on the reader ahead of the
    // container's own, so walking the revision log oldest-first keeps the
    // probe's tags authoritative and fills any gaps from the container.
    let mut log = reader.metadata();
    loop {
        if let Some(rev) = log.current() {
            for tag in &rev.media.tags {
                match &tag.std {
                    Some(StandardTag::TrackTitle(v)) if title.is_none() => {
                        title = Some(v.to_string())
                    }
                    Some(StandardTag::Artist(v)) if artist.is_none() => {
                        artist = Some(v.to_string())
                    }
                    Some(StandardTag::Album(v)) if album.is_none() => album = Some(v.to_string()),
                    _ => {}
                }
            }
        }
        if log.pop().is_none() {
            break;
        }
    }

    (title, artist, album)
}

/// Determine codec for an MP4 container file (AAC, ALAC, etc.).
/// Falls back to "AAC" if the file cannot be parsed as Mp4File.
fn mp4_codec(path: &Path) -> String {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return "AAC".to_string(),
    };
    let mut reader = std::io::BufReader::new(file);
    match Mp4File::read_from(&mut reader, ParseOptions::new()) {
        Ok(mp4) => match mp4.properties().codec() {
            Some(Mp4Codec::ALAC) => "ALAC".to_string(),
            Some(Mp4Codec::MP3) => "MP3".to_string(),
            Some(Mp4Codec::FLAC) => "FLAC".to_string(),
            _ => "AAC".to_string(),
        },
        Err(_) => "AAC".to_string(),
    }
}

/// Map lofty file type to a human-readable codec string.
pub fn codec_string(ft: lofty::file::FileType) -> &'static str {
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
}

/// Extract embedded front cover art bytes from an audio file.
/// Returns raw image bytes (JPEG/PNG) or None. TIFF images are
/// skipped — the `image` crate only has jpeg+png features and macOS
/// CGImageDestination rejects TIFF for Now Playing artwork.
pub fn extract_cover_art(path: &Path) -> Option<Vec<u8>> {
    let tagged_file = lofty::read_from_path(path).ok()?;
    let tag = tagged_file
        .primary_tag()
        .or_else(|| tagged_file.first_tag())?;

    // Prefer CoverFront, fall back to first picture.
    let pictures = tag.pictures();
    let pic = pictures
        .iter()
        .find(|p| p.pic_type() == lofty::picture::PictureType::CoverFront && !is_tiff(p.data()))
        .or_else(|| pictures.iter().find(|p| !is_tiff(p.data())))?;

    Some(pic.data().to_vec())
}

/// TIFF magic bytes: `II*\0` (little-endian) or `MM\0*` (big-endian).
fn is_tiff(data: &[u8]) -> bool {
    data.len() >= 4
        && ((data[0] == 0x49 && data[1] == 0x49 && data[2] == 0x2A && data[3] == 0x00)
            || (data[0] == 0x4D && data[1] == 0x4D && data[2] == 0x00 && data[3] == 0x2A))
}

/// Extract partial track metadata from a Symphonia probe `MetadataRevision`.
///
/// Used during streaming playback to populate track info before the full file
/// is downloaded (and before lofty can read complete tags).
///
/// Fields not present in the probe metadata are left as `None` or defaults.
/// Callers should merge this with a full `read_metadata()` result once the
/// download completes.
pub fn metadata_from_probe_result(meta: &MetadataRevision, fallback_title: &str) -> TrackMeta {
    let mut title: Option<String> = None;
    let mut artist: Option<String> = None;
    let mut album_artist: Option<String> = None;
    let mut album: Option<String> = None;
    let mut date: Option<String> = None;
    let mut disc: Option<i32> = None;
    let mut track_number: Option<i32> = None;
    let mut genre: Option<String> = None;
    let mut label: Option<String> = None;

    // A non-empty text tag replaces the field; empty values are ignored so a
    // blank tag never shadows a later populated one or the fallback.
    let set_text = |slot: &mut Option<String>, value: &str| {
        if !value.is_empty() {
            *slot = Some(value.to_string());
        }
    };

    for tag in &meta.media.tags {
        let Some(std) = &tag.std else { continue };
        match std {
            StandardTag::TrackTitle(v) => set_text(&mut title, v),
            StandardTag::Artist(v) => set_text(&mut artist, v),
            StandardTag::AlbumArtist(v) => set_text(&mut album_artist, v),
            StandardTag::Album(v) => set_text(&mut album, v),
            StandardTag::ReleaseDate(v) | StandardTag::RecordingDate(v) => set_text(&mut date, v),
            StandardTag::ReleaseYear(y) | StandardTag::RecordingYear(y) if date.is_none() => {
                date = Some(y.to_string())
            }
            StandardTag::OriginalReleaseDate(v) | StandardTag::OriginalRecordingDate(v)
                if date.is_none() =>
            {
                set_text(&mut date, v)
            }
            StandardTag::OriginalReleaseYear(y) | StandardTag::OriginalRecordingYear(y)
                if date.is_none() =>
            {
                date = Some(y.to_string())
            }
            StandardTag::TrackNumber(n) => track_number = Some(*n as i32),
            StandardTag::DiscNumber(n) => disc = Some(*n as i32),
            StandardTag::Genre(v) => set_text(&mut genre, v),
            StandardTag::Label(v) => set_text(&mut label, v),
            _ => {}
        }
    }

    TrackMeta {
        title: title.unwrap_or_else(|| fallback_title.to_string()),
        artist: artist.unwrap_or_else(|| "Unknown Artist".to_string()),
        album_artist,
        album: album.unwrap_or_else(|| "Unknown Album".to_string()),
        date,
        disc,
        track_number,
        genre,
        label,
        duration_ms: None,
        codec: None,
        sample_rate: None,
        bit_depth: None,
        channels: None,
        bitrate: None,
        size_bytes: None,
        mtime: None,
        path: None,
        source: "streaming".to_string(),
        remote_id: None,
        remote_url: None,
        album_added_at: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_audio_file() {
        assert!(is_audio_file(Path::new("track.flac")));
        assert!(is_audio_file(Path::new("track.FLAC")));
        assert!(is_audio_file(Path::new("track.mp3")));
        assert!(is_audio_file(Path::new("track.m4a")));
        assert!(is_audio_file(Path::new("track.ogg")));
        assert!(is_audio_file(Path::new("track.opus")));
        assert!(is_audio_file(Path::new("track.wv")));
        assert!(is_audio_file(Path::new("track.wav")));
        assert!(is_audio_file(Path::new("track.aiff")));
        assert!(is_audio_file(Path::new("track.ape")));

        assert!(!is_audio_file(Path::new("cover.jpg")));
        assert!(!is_audio_file(Path::new("notes.txt")));
        assert!(!is_audio_file(Path::new("playlist.m3u")));
        assert!(!is_audio_file(Path::new("track.pdf")));
        assert!(!is_audio_file(Path::new("noext")));
    }

    #[test]
    fn test_is_audio_file_paths() {
        assert!(is_audio_file(Path::new("/music/artist/album/01.flac")));
        assert!(!is_audio_file(Path::new("/music/artist/album/cover.png")));
    }

    #[test]
    fn test_read_metadata_nonexistent() {
        let result = read_metadata(Path::new("/nonexistent/track.flac"));
        assert!(result.is_err());
    }

    #[test]
    fn test_codec_string() {
        assert_eq!(codec_string(lofty::file::FileType::Flac), "FLAC");
        assert_eq!(codec_string(lofty::file::FileType::Mpeg), "MP3");
        assert_eq!(codec_string(lofty::file::FileType::Opus), "Opus");
        assert_eq!(codec_string(lofty::file::FileType::Wav), "WAV");
    }

    // --- metadata_from_probe_result tests ---

    use symphonia::core::meta::well_known::METADATA_ID_ID3V2;
    use symphonia::core::meta::{MetadataBuilder, MetadataInfo, StandardTag, Tag};

    const TEST_META_INFO: MetadataInfo = MetadataInfo {
        metadata: METADATA_ID_ID3V2,
        short_name: "id3v2",
        long_name: "ID3v2",
    };

    fn make_revision(tags: &[StandardTag]) -> symphonia::core::meta::MetadataRevision {
        let mut builder = MetadataBuilder::new(TEST_META_INFO);
        for std in tags {
            builder.add_tag(Tag::new_from_parts("", "", Some(std.clone())));
        }
        builder.build()
    }

    #[test]
    fn test_probe_track_and_disc_numbers() {
        let rev = make_revision(&[
            StandardTag::TrackTitle("My Song".to_string().into()),
            StandardTag::Artist("Artist".to_string().into()),
            StandardTag::Album("Album".to_string().into()),
            StandardTag::TrackNumber(3),
            StandardTag::TrackTotal(12),
            StandardTag::DiscNumber(2),
        ]);
        let meta = metadata_from_probe_result(&rev, "fallback");
        assert_eq!(
            meta.track_number,
            Some(3),
            "track number should come from the track number tag, not the total"
        );
        assert_eq!(meta.disc, Some(2));
    }

    #[test]
    fn test_probe_original_date_fallback() {
        // When no release/recording date is present, the original date is used.
        let rev = make_revision(&[
            StandardTag::TrackTitle("My Song".to_string().into()),
            StandardTag::OriginalReleaseDate("1991".to_string().into()),
        ]);
        let meta = metadata_from_probe_result(&rev, "fallback");
        assert_eq!(
            meta.date,
            Some("1991".to_string()),
            "original release date should be used when the release date is missing"
        );
    }

    #[test]
    fn test_probe_original_date_not_used_when_date_present() {
        // The release date takes precedence over the original release date.
        let rev = make_revision(&[
            StandardTag::ReleaseDate("2005".to_string().into()),
            StandardTag::OriginalReleaseDate("1991".to_string().into()),
        ]);
        let meta = metadata_from_probe_result(&rev, "fallback");
        assert_eq!(
            meta.date,
            Some("2005".to_string()),
            "release date should take precedence over original release date"
        );
    }

    #[test]
    fn test_probe_empty_values_skipped() {
        // Tags with empty string values should be silently skipped,
        // leaving the corresponding fields as None (or falling back to defaults).
        let rev = make_revision(&[
            StandardTag::Artist(String::new().into()),
            StandardTag::Album(String::new().into()),
            StandardTag::Genre(String::new().into()),
        ]);
        let meta = metadata_from_probe_result(&rev, "Title");
        // Empty artist/album fall back to defaults, not empty string.
        assert_eq!(
            meta.artist, "Unknown Artist",
            "empty artist tag should fall back to 'Unknown Artist'"
        );
        assert_eq!(
            meta.album, "Unknown Album",
            "empty album tag should fall back to 'Unknown Album'"
        );
        assert_eq!(meta.genre, None, "empty genre tag should produce None");
    }

    #[test]
    fn test_probe_defaults() {
        // When no tags are present, artist and album should use the hardcoded defaults.
        // Title should fall back to the fallback_title argument.
        let rev = make_revision(&[]);
        let meta = metadata_from_probe_result(&rev, "Fallback Title");
        assert_eq!(
            meta.title, "Fallback Title",
            "missing title should use fallback_title argument"
        );
        assert_eq!(
            meta.artist, "Unknown Artist",
            "missing artist should default to 'Unknown Artist'"
        );
        assert_eq!(
            meta.album, "Unknown Album",
            "missing album should default to 'Unknown Album'"
        );
        assert_eq!(meta.track_number, None);
        assert_eq!(meta.date, None);
        assert_eq!(meta.genre, None);
        assert_eq!(meta.source, "streaming");
    }

    #[test]
    fn test_mp4_codec_nonexistent_file_falls_back_to_aac() {
        assert_eq!(mp4_codec(Path::new("/nonexistent/track.m4a")), "AAC");
    }

    #[test]
    fn test_mp4_codec_non_mp4_file_falls_back_to_aac() {
        // A non-MP4 file should fail to parse and fall back to "AAC".
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        assert_eq!(mp4_codec(&manifest), "AAC");
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn test_mp4_codec_real_alac_file() {
        // Integration test: verify that a real ALAC .m4a file is correctly
        // identified as "ALAC" rather than "AAC". Skipped silently when the
        // Turtlehead volume is not mounted.
        let alac_path = Path::new(
            "/Volumes/Turtlehead/music/Valet Girls/(2017) PERENNIAL VICE [ALAC]/0101. Valet Girls - Tis the Season.m4a",
        );
        if !alac_path.exists() {
            eprintln!("SKIP: ALAC test file not found (volume not mounted)");
            return;
        }
        assert_eq!(
            mp4_codec(alac_path),
            "ALAC",
            "real ALAC .m4a should be identified as ALAC, not AAC"
        );
    }
}
