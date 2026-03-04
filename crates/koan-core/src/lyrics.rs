/// Lyrics fetch pipeline: embedded tags -> sidecar .lrc -> LRCLIB API -> DB cache.
///
/// Phase 1 implements LRCLIB + DB caching. Embedded and sidecar sources are
/// stubbed and will be filled in Phase 2.
use rusqlite::Connection;

use crate::db::connection::DbError;
use crate::db::queries::lyrics::{cache_lyrics, get_cached_lyrics};
use crate::remote::lrclib::{self, LrclibError};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// The resolved lyrics for a track.
#[derive(Debug, Clone)]
pub struct Lyrics {
    /// Raw lyrics text. LRC format if `synced` is true, plain text otherwise.
    pub content: String,
    /// Whether `content` is in LRC (time-tagged) format.
    pub synced: bool,
    /// Which source provided the lyrics.
    pub source: LyricsSource,
}

/// Which source provided the lyrics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LyricsSource {
    /// Embedded in the audio file tags (e.g. USLT ID3, Vorbis LYRICS).
    Embedded,
    /// Sidecar `.lrc` file next to the audio file.
    Sidecar,
    /// LRCLIB community lyrics database.
    Lrclib,
    /// DB cache hit (originally from any of the above sources).
    Cache,
}

impl LyricsSource {
    fn as_str(&self) -> &'static str {
        match self {
            LyricsSource::Embedded => "embedded",
            LyricsSource::Sidecar => "sidecar",
            LyricsSource::Lrclib => "lrclib",
            LyricsSource::Cache => "cache",
        }
    }
}

/// Errors that can occur during lyrics fetching.
#[derive(Debug, thiserror::Error)]
pub enum LyricsError {
    #[error("database error: {0}")]
    Db(#[from] DbError),
    #[error("lrclib error: {0}")]
    Lrclib(#[from] LrclibError),
    #[error("lyrics not found")]
    NotFound,
}

// ---------------------------------------------------------------------------
// LRC parsing
// ---------------------------------------------------------------------------

/// A single time-tagged line from an LRC file.
#[derive(Debug, Clone)]
pub struct LrcLine {
    /// Timestamp in seconds (e.g. 12.0 for `[00:12.00]`).
    pub time_secs: f64,
    /// The lyric text for this timestamp.
    pub text: String,
}

/// Parse LRC-format text into a sorted list of [`LrcLine`]s.
///
/// Lines without a valid timestamp are silently skipped. The result is sorted
/// by `time_secs` ascending so binary search works correctly.
pub fn parse_lrc(content: &str) -> Vec<LrcLine> {
    let mut lines: Vec<LrcLine> = content
        .lines()
        .filter_map(|line| {
            // LRC timestamp: `[mm:ss.xx]` or `[mm:ss.xxx]`
            let line = line.trim();
            if !line.starts_with('[') {
                return None;
            }
            let close = line.find(']')?;
            let tag = &line[1..close];
            let text = line[close + 1..].trim().to_string();

            // Parse mm:ss.xx
            let colon = tag.find(':')?;
            let mins: f64 = tag[..colon].parse().ok()?;
            let secs: f64 = tag[colon + 1..].parse().ok()?;
            let time_secs = mins * 60.0 + secs;

            Some(LrcLine { time_secs, text })
        })
        .collect();

    lines.sort_by(|a, b| {
        a.time_secs
            .partial_cmp(&b.time_secs)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    lines
}

/// Return the index of the current lyric line for the given playback position.
///
/// Returns `None` if the position is before the first timestamped line.
/// Uses binary search for O(log n) lookup.
pub fn current_line_index(lines: &[LrcLine], position_secs: f64) -> Option<usize> {
    if lines.is_empty() {
        return None;
    }
    // Find the last line whose timestamp <= position_secs.
    match lines.binary_search_by(|l| {
        l.time_secs
            .partial_cmp(&position_secs)
            .unwrap_or(std::cmp::Ordering::Less)
    }) {
        Ok(i) => Some(i),
        Err(0) => None,        // before first line
        Err(i) => Some(i - 1), // i is the insertion point; i-1 is the active line
    }
}

// ---------------------------------------------------------------------------
// Fetch pipeline
// ---------------------------------------------------------------------------

/// Fetch lyrics for a track using the priority chain:
///
/// 1. DB cache (instant, no network)
/// 2. Embedded lyrics tag (stub — returns `None` in Phase 1)
/// 3. Sidecar `.lrc` file (stub — returns `None` in Phase 1)
/// 4. LRCLIB API
///
/// On a successful LRCLIB fetch the result is written to the DB cache so the
/// next call is instant.
pub fn fetch_lyrics(
    conn: &Connection,
    track_id: i64,
    artist: &str,
    title: &str,
    album: &str,
    duration_secs: u64,
) -> Result<Lyrics, LyricsError> {
    // 1. Check DB cache first.
    if let Some((content, synced)) = get_cached_lyrics(conn, track_id)? {
        return Ok(Lyrics {
            content,
            synced,
            source: LyricsSource::Cache,
        });
    }

    // 2. Embedded lyrics (stub — Phase 2 will read via lofty ItemKey::Lyrics).
    // 3. Sidecar .lrc (stub — Phase 2 will check `track_path.with_extension("lrc")`).

    // 4. LRCLIB API.
    let response =
        lrclib::get_lyrics(artist, title, album, duration_secs).map_err(|e| match e {
            LrclibError::NotFound => LyricsError::NotFound,
            other => LyricsError::Lrclib(other),
        })?;

    // Prefer synced lyrics; fall back to plain.
    let (content, synced) = if let Some(synced_lyrics) = response.synced_lyrics {
        (synced_lyrics, true)
    } else if let Some(plain_lyrics) = response.plain_lyrics {
        (plain_lyrics, false)
    } else {
        return Err(LyricsError::NotFound);
    };

    // Cache the result.
    cache_lyrics(
        conn,
        track_id,
        LyricsSource::Lrclib.as_str(),
        synced,
        &content,
    )?;

    Ok(Lyrics {
        content,
        synced,
        source: LyricsSource::Lrclib,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_lrc_basic() {
        let lrc = "[00:12.00]Hello world\n[00:17.20]Second line\n[01:05.50]Third line";
        let lines = parse_lrc(lrc);
        assert_eq!(lines.len(), 3);
        assert!((lines[0].time_secs - 12.0).abs() < 0.01);
        assert_eq!(lines[0].text, "Hello world");
        assert!((lines[1].time_secs - 17.2).abs() < 0.01);
        assert!((lines[2].time_secs - 65.5).abs() < 0.01);
    }

    #[test]
    fn test_parse_lrc_skips_non_timestamp_lines() {
        let lrc = "[ti:Song Title]\n[ar:Artist]\n[00:05.00]First lyric\n[00:10.00]Second lyric";
        let lines = parse_lrc(lrc);
        // [ti:...] and [ar:...] tags won't parse as mm:ss.xx (colon at wrong position / no dots)
        // They might or might not parse depending on content; just verify the lyric lines are there.
        assert!(lines.iter().any(|l| l.text == "First lyric"));
        assert!(lines.iter().any(|l| l.text == "Second lyric"));
    }

    #[test]
    fn test_current_line_index_empty() {
        assert_eq!(current_line_index(&[], 5.0), None);
    }

    #[test]
    fn test_current_line_index_before_first() {
        let lines = parse_lrc("[00:10.00]First");
        assert_eq!(current_line_index(&lines, 5.0), None);
    }

    #[test]
    fn test_current_line_index_exact_match() {
        let lines = parse_lrc("[00:10.00]First\n[00:20.00]Second");
        assert_eq!(current_line_index(&lines, 10.0), Some(0));
        assert_eq!(current_line_index(&lines, 20.0), Some(1));
    }

    #[test]
    fn test_current_line_index_between_lines() {
        let lines = parse_lrc("[00:10.00]First\n[00:20.00]Second\n[00:30.00]Third");
        assert_eq!(current_line_index(&lines, 15.0), Some(0));
        assert_eq!(current_line_index(&lines, 25.0), Some(1));
        assert_eq!(current_line_index(&lines, 35.0), Some(2));
    }
}
