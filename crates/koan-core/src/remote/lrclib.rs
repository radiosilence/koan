use serde::Deserialize;
use thiserror::Error;

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
    pub synced_lyrics: Option<String>,
    pub plain_lyrics: Option<String>,
}

/// Fetch lyrics from LRCLIB for a given track.
pub fn get_lyrics(
    artist: &str,
    title: &str,
    album: &str,
    duration_secs: u64,
) -> Result<LrclibResponse, LrclibError> {
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

    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(LrclibError::NotFound);
    }

    let body: LrclibResponse = resp.error_for_status()?.json()?;
    Ok(body)
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
}
