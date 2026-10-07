use serde::Deserialize;
use thiserror::Error;
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

const USER_AGENT: &str = "koan-music/0.3.0";

#[derive(Debug, Error)]
pub enum LrclibError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("not found")]
    NotFound,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LrclibResponse {
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub track_name: String,
    /// Seconds.
    #[serde(default)]
    pub duration: f64,
    pub synced_lyrics: Option<String>,
    pub plain_lyrics: Option<String>,
}

/// How far a record's length may be from the track's and still be its words.
const DURATION_TOLERANCE_SECS: f64 = 2.0;

/// Fetch lyrics from LRCLIB for a given track.
///
/// `/api/get` first, which matches artist, title, album and duration, and is
/// held only to the length. A miss falls back to `/api/search` by artist and
/// title, whose hits are fuzzy: one is taken only if it is this title and this
/// length. No match is no lyrics,
/// never another song's. A track of unknown length is not looked up at all.
pub fn get_lyrics(
    artist: &str,
    title: &str,
    album: &str,
    duration_secs: u64,
) -> Result<LrclibResponse, LrclibError> {
    if duration_secs == 0 {
        return Err(LrclibError::NotFound);
    }
    let client = reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .build()?;

    let resp = client
        .get("https://lrclib.net/api/get")
        .query(&[
            ("artist_name", artist),
            ("track_name", title),
            ("album_name", album),
        ])
        .query(&[("duration", &duration_secs.to_string())])
        .send()?;

    if resp.status() != reqwest::StatusCode::NOT_FOUND {
        let body: LrclibResponse = resp.error_for_status()?.json()?;
        if close_in_length(&body, duration_secs) {
            return Ok(body);
        }
    }

    let results: Vec<LrclibResponse> = client
        .get("https://lrclib.net/api/search")
        .query(&[("artist_name", artist), ("track_name", title)])
        .send()?
        .error_for_status()?
        .json()?;
    pick(results, title, duration_secs).ok_or(LrclibError::NotFound)
}

/// The first hit that is this track, if any is.
fn pick(results: Vec<LrclibResponse>, title: &str, duration_secs: u64) -> Option<LrclibResponse> {
    results
        .into_iter()
        .find(|r| matches(r, title, duration_secs))
}

fn close_in_length(record: &LrclibResponse, duration_secs: u64) -> bool {
    (record.duration - duration_secs as f64).abs() <= DURATION_TOLERANCE_SECS
}

/// Whether a record is the track's: the same title once credits, editions and
/// punctuation are set aside, and a length within [`DURATION_TOLERANCE_SECS`],
/// which is what tells two songs of one name apart.
fn matches(record: &LrclibResponse, title: &str, duration_secs: u64) -> bool {
    close_in_length(record, duration_secs)
        && normalise_title(&record.track_name) == normalise_title(title)
}

/// Words that mark a bracketed group as a credit or an edition of the song
/// rather than part of its name.
const ASIDES: [&str; 14] = [
    "feat",
    "ft",
    "featuring",
    "remaster",
    "remastered",
    "live",
    "version",
    "edit",
    "mono",
    "stereo",
    "deluxe",
    "bonus",
    "mix",
    "remix",
];

/// A title as matching sees it: lowercased, accents folded, a trailing
/// " - …" and any bracketed aside dropped, an unbracketed featured credit cut
/// to the end, and only letters and digits kept, single-spaced.
fn normalise_title(title: &str) -> String {
    let folded: String = title
        .nfkd()
        .filter(|c| !is_combining_mark(*c))
        .collect::<String>()
        .to_lowercase();
    let head = folded.split(" - ").next().unwrap_or_default();

    let mut kept = String::new();
    let mut rest = head;
    while let Some(ch) = rest.chars().next() {
        let close = match ch {
            '(' => Some(')'),
            '[' => Some(']'),
            _ => None,
        };
        if let Some(close) = close {
            let end = rest.find(close).map_or(rest.len(), |at| at + 1);
            if words(&rest[..end]).any(|w| ASIDES.contains(&w)) {
                rest = &rest[end..];
                continue;
            }
        }
        kept.push(if ch.is_alphanumeric() { ch } else { ' ' });
        rest = &rest[ch.len_utf8()..];
    }
    let mut out = Vec::new();
    for w in words(&kept) {
        if matches!(w, "feat" | "ft" | "featuring") {
            break;
        }
        out.push(w);
    }
    out.join(" ")
}

fn words(s: &str) -> impl Iterator<Item = &str> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_lrclib_response() {
        let json = r#"{
            "id": 123,
            "trackName": "Test",
            "artistName": "Artist",
            "albumName": "Album",
            "duration": 240,
            "instrumental": false,
            "plainLyrics": "Hello world\nSecond line",
            "syncedLyrics": "[00:12.00]Hello world\n[00:17.20]Second line"
        }"#;
        let resp: LrclibResponse = serde_json::from_str(json).unwrap();
        assert_eq!(
            resp.plain_lyrics.as_deref(),
            Some("Hello world\nSecond line")
        );
        assert!(resp.synced_lyrics.unwrap().contains("[00:12.00]"));
    }

    #[test]
    fn test_deserialize_lrclib_response_null_lyrics() {
        let json = r#"{
            "id": 456,
            "trackName": "Instrumental",
            "artistName": "Artist",
            "albumName": "Album",
            "duration": 180,
            "instrumental": true,
            "plainLyrics": null,
            "syncedLyrics": null
        }"#;
        let resp: LrclibResponse = serde_json::from_str(json).unwrap();
        assert!(resp.plain_lyrics.is_none());
        assert!(resp.synced_lyrics.is_none());
    }

    fn record(id: i64, track_name: &str, duration: f64) -> LrclibResponse {
        LrclibResponse {
            id,
            track_name: track_name.into(),
            duration,
            synced_lyrics: None,
            plain_lyrics: Some(String::new()),
        }
    }

    const UNCONDITIONAL_II: &str = "Unconditional II (Race and Religion) feat. Peter Gabriel";

    #[test]
    fn a_featured_credit_is_set_aside_however_it_is_written() {
        let want = "unconditional ii race and religion";
        for t in [
            UNCONDITIONAL_II,
            "Unconditional II (Race and Religion) [feat. Peter Gabriel]",
            "Unconditional II (Race and Religion) (Feat. Peter Gabriel)",
            "Unconditional II (Race And Religion)",
            "Unconditional II (Race and Religion) ft Peter Gabriel",
        ] {
            assert_eq!(normalise_title(t), want, "{t}");
        }
        assert_eq!(normalise_title("Left Behind"), "left behind");
        assert_eq!(normalise_title("Live and Let Die"), "live and let die");
        assert_eq!(normalise_title("Aftermath"), "aftermath");
    }

    #[test]
    fn editions_and_accents_are_the_same_title() {
        for (a, b) in [
            ("Song - Remastered 2011", "Song"),
            ("Song (Live)", "Song"),
            ("Song (Deluxe Edition)", "Song"),
            ("Song [2009 Remaster]", "Song"),
            ("Beyoncé", "Beyonce"),
            ("Don\u{2019}t Stop", "Don't Stop"),
        ] {
            assert_eq!(normalise_title(a), normalise_title(b), "{a} / {b}");
        }
        assert_ne!(normalise_title("Song (Part Two)"), normalise_title("Song"));
    }

    /// Arcade Fire's WE: a search for the ninth track must not answer with
    /// another track of the record, whatever order the hits come in.
    #[test]
    fn a_search_takes_only_this_title_at_this_length() {
        let hits = vec![
            record(1, "The Lightning I, II", 261.0),
            record(2, "The Lightning II", 154.0),
            record(3, "Unconditional II (Race and Religion)", 300.0),
            record(
                310454,
                "Unconditional II (Race and Religion) [feat. Peter Gabriel]",
                261.0,
            ),
        ];
        assert_eq!(
            pick(hits, UNCONDITIONAL_II, 260).map(|r| r.id),
            Some(310454)
        );
    }

    #[test]
    fn no_match_is_no_lyrics() {
        let hits = vec![
            record(1, "The Lightning I, II", 260.0),
            record(2, "Unconditional II (Race and Religion)", 263.0),
        ];
        assert!(pick(hits, UNCONDITIONAL_II, 260).is_none());
    }

    #[test]
    fn two_seconds_either_way_is_the_same_recording() {
        assert!(matches(
            &record(1, UNCONDITIONAL_II, 262.0),
            UNCONDITIONAL_II,
            260
        ));
        assert!(matches(
            &record(1, UNCONDITIONAL_II, 258.0),
            UNCONDITIONAL_II,
            260
        ));
        assert!(!matches(
            &record(1, UNCONDITIONAL_II, 262.5),
            UNCONDITIONAL_II,
            260
        ));
    }
}
