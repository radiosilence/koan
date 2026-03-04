use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;
use thiserror::Error;

const API_VERSION: &str = "1.16.1";
const CLIENT_NAME: &str = "koan";

#[derive(Debug, Error)]
pub enum SubsonicError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("api error: {code} — {message}")]
    Api { code: i32, message: String },
    #[error("unexpected response format")]
    BadResponse,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Subsonic/Navidrome API client.
pub struct SubsonicClient {
    base_url: String,
    username: String,
    password: String,
    http: reqwest::blocking::Client,
}

impl SubsonicClient {
    pub fn new(base_url: &str, username: &str, password: &str) -> Self {
        let base_url = base_url.trim_end_matches('/').to_string();
        Self {
            base_url,
            username: username.to_string(),
            password: password.to_string(),
            http: reqwest::blocking::Client::new(),
        }
    }

    /// Build auth query params: u, t (token), s (salt), v, c, f.
    fn auth_params(&self) -> HashMap<String, String> {
        let salt = random_salt();

        let token = format!("{:x}", md5::compute(format!("{}{}", self.password, salt)));

        let mut params = HashMap::new();
        params.insert("u".into(), self.username.clone());
        params.insert("t".into(), token);
        params.insert("s".into(), salt);
        params.insert("v".into(), API_VERSION.into());
        params.insert("c".into(), CLIENT_NAME.into());
        params.insert("f".into(), "json".into());
        params
    }

    /// Make a GET request to a Subsonic API endpoint.
    fn get(&self, endpoint: &str) -> Result<SubsonicResponse, SubsonicError> {
        self.get_with_params(endpoint, &[])
    }

    fn get_with_params(
        &self,
        endpoint: &str,
        extra: &[(&str, &str)],
    ) -> Result<SubsonicResponse, SubsonicError> {
        let url = format!("{}/rest/{}", self.base_url, endpoint);
        let mut params = self.auth_params();
        for (k, v) in extra {
            params.insert((*k).to_string(), (*v).to_string());
        }

        let resp: SubsonicResponseWrapper = self.http.get(&url).query(&params).send()?.json()?;

        let inner = resp.subsonic_response;
        if inner.status != "ok" {
            if let Some(err) = inner.error {
                return Err(SubsonicError::Api {
                    code: err.code,
                    message: err.message,
                });
            }
            return Err(SubsonicError::BadResponse);
        }

        Ok(inner)
    }

    /// Ping the server — verify connection and credentials.
    pub fn ping(&self) -> Result<(), SubsonicError> {
        self.get("ping")?;
        Ok(())
    }

    /// Get all artists (indexed).
    pub fn get_artists(&self) -> Result<Vec<SubsonicArtist>, SubsonicError> {
        let resp = self.get("getArtists")?;
        let artists_data = resp.artists.ok_or(SubsonicError::BadResponse)?;
        let mut all = Vec::new();
        for index in artists_data.index {
            all.extend(index.artist);
        }
        Ok(all)
    }

    /// Get an album by ID, including its tracks.
    pub fn get_album(&self, id: &str) -> Result<SubsonicAlbumFull, SubsonicError> {
        let resp = self.get_with_params("getAlbum", &[("id", id)])?;
        resp.album.ok_or(SubsonicError::BadResponse)
    }

    /// Get a paginated list of albums.
    pub fn get_album_list(
        &self,
        list_type: &str,
        size: u32,
        offset: u32,
    ) -> Result<Vec<SubsonicAlbum>, SubsonicError> {
        let size_str = size.to_string();
        let offset_str = offset.to_string();
        let resp = self.get_with_params(
            "getAlbumList2",
            &[
                ("type", list_type),
                ("size", &size_str),
                ("offset", &offset_str),
            ],
        )?;
        Ok(resp.album_list2.map(|al| al.album).unwrap_or_default())
    }

    /// Build the streaming URL for a track (doesn't make a request).
    pub fn stream_url(&self, track_id: &str) -> String {
        let params = self.auth_params();
        let query: String = params
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join("&");
        format!("{}/rest/stream?id={}&{}", self.base_url, track_id, query)
    }

    /// Stream URL without auth params — safe for database storage.
    pub fn stream_url_template(&self, track_id: &str) -> String {
        format!("{}/rest/stream?id={}", self.base_url, track_id)
    }

    /// Download a track to a local path.
    pub fn download(&self, track_id: &str, dest: &Path) -> Result<(), SubsonicError> {
        self.download_with_progress(track_id, dest, |_, _| {})
    }

    /// Download a track with progress reporting.
    /// The callback receives (bytes_downloaded, total_bytes). Total may be 0
    /// if the server doesn't send Content-Length.
    pub fn download_with_progress(
        &self,
        track_id: &str,
        dest: &Path,
        on_progress: impl Fn(u64, u64),
    ) -> Result<(), SubsonicError> {
        use std::io::Write;

        let url = format!("{}/rest/download", self.base_url);
        let mut params = self.auth_params();
        params.insert("id".into(), track_id.to_string());

        let mut resp = self.http.get(&url).query(&params).send()?;
        let total = resp.content_length().unwrap_or(0);

        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut file = std::fs::File::create(dest)?;
        let mut downloaded: u64 = 0;
        let mut buf = [0u8; 64 * 1024]; // 64KB chunks
        loop {
            let n = std::io::Read::read(&mut resp, &mut buf).map_err(SubsonicError::Io)?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n])?;
            downloaded += n as u64;
            on_progress(downloaded, total);
        }
        file.flush()?;
        Ok(())
    }

    /// Search for tracks/albums/artists.
    pub fn search(&self, query: &str) -> Result<SubsonicSearchResult, SubsonicError> {
        let resp = self.get_with_params("search3", &[("query", query)])?;
        Ok(resp.search_result3.unwrap_or_default())
    }

    /// Report a play (scrobble).
    pub fn scrobble(&self, track_id: &str) -> Result<(), SubsonicError> {
        self.get_with_params("scrobble", &[("id", track_id)])?;
        Ok(())
    }

    /// Star (favourite) a track on the server.
    pub fn star(&self, track_id: &str) -> Result<(), SubsonicError> {
        self.get_with_params("star", &[("id", track_id)])?;
        Ok(())
    }

    /// Unstar (unfavourite) a track on the server.
    pub fn unstar(&self, track_id: &str) -> Result<(), SubsonicError> {
        self.get_with_params("unstar", &[("id", track_id)])?;
        Ok(())
    }

    /// Get all starred (favourite) songs from the server.
    pub fn get_starred(&self) -> Result<Vec<SubsonicSong>, SubsonicError> {
        let resp = self.get("getStarred2")?;
        Ok(resp.starred2.map(|s| s.song).unwrap_or_default())
    }
}

// --- Response types ---

#[derive(Debug, Deserialize)]
struct SubsonicResponseWrapper {
    #[serde(rename = "subsonic-response")]
    subsonic_response: SubsonicResponse,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubsonicResponse {
    status: String,
    error: Option<SubsonicApiError>,
    artists: Option<SubsonicArtists>,
    album: Option<SubsonicAlbumFull>,
    album_list2: Option<SubsonicAlbumList>,
    search_result3: Option<SubsonicSearchResult>,
    starred2: Option<SubsonicStarred>,
}

#[derive(Debug, Deserialize)]
struct SubsonicApiError {
    code: i32,
    message: String,
}

#[derive(Debug, Deserialize)]
struct SubsonicArtists {
    index: Vec<SubsonicArtistIndex>,
}

#[derive(Debug, Deserialize)]
struct SubsonicArtistIndex {
    artist: Vec<SubsonicArtist>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubsonicArtist {
    pub id: String,
    pub name: String,
    pub album_count: Option<i32>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubsonicAlbum {
    pub id: String,
    pub name: String,
    pub artist: Option<String>,
    pub artist_id: Option<String>,
    pub song_count: Option<i32>,
    pub year: Option<i32>,
    pub genre: Option<String>,
    pub created: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubsonicAlbumFull {
    pub id: String,
    pub name: String,
    pub artist: Option<String>,
    pub artist_id: Option<String>,
    pub year: Option<i32>,
    pub genre: Option<String>,
    pub song_count: Option<i32>,
    pub created: Option<String>,
    #[serde(default)]
    pub song: Vec<SubsonicSong>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubsonicSong {
    pub id: String,
    pub title: String,
    pub album: Option<String>,
    pub artist: Option<String>,
    pub track: Option<i32>,
    pub disc_number: Option<i32>,
    pub year: Option<i32>,
    pub genre: Option<String>,
    pub duration: Option<i64>,
    pub bit_rate: Option<i32>,
    pub suffix: Option<String>,
    pub content_type: Option<String>,
    pub album_id: Option<String>,
    pub artist_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SubsonicAlbumList {
    #[serde(default)]
    album: Vec<SubsonicAlbum>,
}

#[derive(Debug, Default, Deserialize)]
pub struct SubsonicSearchResult {
    #[serde(default)]
    pub artist: Vec<SubsonicArtist>,
    #[serde(default)]
    pub album: Vec<SubsonicAlbum>,
    #[serde(default)]
    pub song: Vec<SubsonicSong>,
}

#[derive(Debug, Default, Deserialize)]
pub struct SubsonicStarred {
    #[serde(default)]
    pub song: Vec<SubsonicSong>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- SubsonicSong deserialization ---

    #[test]
    fn test_deserialize_subsonic_song() {
        let json = r#"{
            "id": "42",
            "title": "Space Oddity",
            "album": "Space Oddity",
            "artist": "David Bowie",
            "track": 1,
            "discNumber": 1,
            "year": 1969,
            "genre": "Rock",
            "duration": 314,
            "bitRate": 320,
            "suffix": "mp3",
            "contentType": "audio/mpeg",
            "albumId": "7",
            "artistId": "3"
        }"#;

        let song: SubsonicSong = serde_json::from_str(json).unwrap();

        assert_eq!(song.id, "42");
        assert_eq!(song.title, "Space Oddity");
        assert_eq!(song.album.as_deref(), Some("Space Oddity"));
        assert_eq!(song.artist.as_deref(), Some("David Bowie"));
        assert_eq!(song.track, Some(1));
        assert_eq!(song.disc_number, Some(1));
        assert_eq!(song.year, Some(1969));
        assert_eq!(song.genre.as_deref(), Some("Rock"));
        assert_eq!(song.duration, Some(314));
        assert_eq!(song.bit_rate, Some(320));
        assert_eq!(song.suffix.as_deref(), Some("mp3"));
        assert_eq!(song.content_type.as_deref(), Some("audio/mpeg"));
        assert_eq!(song.album_id.as_deref(), Some("7"));
        assert_eq!(song.artist_id.as_deref(), Some("3"));
    }

    #[test]
    fn test_deserialize_subsonic_song_optional_fields_absent() {
        // Only the required fields (id, title) — all Option fields should be None.
        let json = r#"{"id": "99", "title": "Minimal Track"}"#;

        let song: SubsonicSong = serde_json::from_str(json).unwrap();

        assert_eq!(song.id, "99");
        assert_eq!(song.title, "Minimal Track");
        assert!(song.album.is_none());
        assert!(song.artist.is_none());
        assert!(song.track.is_none());
        assert!(song.disc_number.is_none());
        assert!(song.year.is_none());
        assert!(song.duration.is_none());
        assert!(song.bit_rate.is_none());
    }

    // --- SubsonicAlbum deserialization ---

    #[test]
    fn test_deserialize_album_list() {
        let json = r#"{
            "subsonic-response": {
                "status": "ok",
                "version": "1.16.1",
                "albumList2": {
                    "album": [
                        {
                            "id": "1",
                            "name": "Abbey Road",
                            "artist": "The Beatles",
                            "artistId": "10",
                            "songCount": 17,
                            "year": 1969,
                            "genre": "Rock",
                            "created": "2020-01-01T00:00:00"
                        },
                        {
                            "id": "2",
                            "name": "Led Zeppelin IV",
                            "artist": "Led Zeppelin",
                            "artistId": "11",
                            "songCount": 8,
                            "year": 1971,
                            "genre": "Hard Rock",
                            "created": "2020-01-02T00:00:00"
                        }
                    ]
                }
            }
        }"#;

        let wrapper: SubsonicResponseWrapper = serde_json::from_str(json).unwrap();
        let album_list = wrapper
            .subsonic_response
            .album_list2
            .expect("album_list2 should be present");

        assert_eq!(album_list.album.len(), 2);

        let first = &album_list.album[0];
        assert_eq!(first.id, "1");
        assert_eq!(first.name, "Abbey Road");
        assert_eq!(first.artist.as_deref(), Some("The Beatles"));
        assert_eq!(first.artist_id.as_deref(), Some("10"));
        assert_eq!(first.song_count, Some(17));
        assert_eq!(first.year, Some(1969));

        let second = &album_list.album[1];
        assert_eq!(second.id, "2");
        assert_eq!(second.name, "Led Zeppelin IV");
        assert_eq!(second.song_count, Some(8));
    }

    // --- SubsonicClient auth params ---

    #[test]
    fn test_auth_params_format() {
        let client = SubsonicClient::new("http://localhost:4533", "alice", "secret");
        let params = client.auth_params();

        // Must contain exactly these six keys.
        assert!(params.contains_key("u"), "missing 'u' param");
        assert!(params.contains_key("t"), "missing 't' param");
        assert!(params.contains_key("s"), "missing 's' param");
        assert!(params.contains_key("v"), "missing 'v' param");
        assert!(params.contains_key("c"), "missing 'c' param");
        assert!(params.contains_key("f"), "missing 'f' param");
        assert_eq!(params.len(), 6);

        assert_eq!(params["u"], "alice");
        assert_eq!(params["v"], "1.16.1");
        assert_eq!(params["c"], "koan");
        assert_eq!(params["f"], "json");
    }

    #[test]
    fn test_auth_params_token_is_md5_of_password_plus_salt() {
        let client = SubsonicClient::new("http://localhost:4533", "bob", "letmein");
        let params = client.auth_params();

        let salt = &params["s"];
        let token = &params["t"];

        // The token must equal md5(password + salt).
        let expected = format!("{:x}", md5::compute(format!("letmein{}", salt)));
        assert_eq!(token, &expected);
    }

    #[test]
    fn test_auth_params_salt_is_different_each_call() {
        let client = SubsonicClient::new("http://localhost:4533", "user", "pass");
        let params1 = client.auth_params();
        let params2 = client.auth_params();

        // Salts should differ across calls (random); tokens will differ too.
        // There is a negligible probability they collide — acceptable in tests.
        assert_ne!(params1["s"], params2["s"], "salt should be random per call");
    }

    // --- stream_url ---

    #[test]
    fn test_stream_url_has_auth() {
        let client = SubsonicClient::new("http://myserver:4533", "user", "pass");
        let url = client.stream_url("track-123");

        assert!(url.contains("track-123"), "url must include the track id");
        assert!(url.contains("u=user"), "url must include username param");
        assert!(url.contains("v=1.16.1"), "url must include api version");
        assert!(url.contains("c=koan"), "url must include client name");
        assert!(url.contains("f=json"), "url must include format param");
        assert!(url.contains("/rest/stream"), "url must target /rest/stream");
        assert!(
            url.starts_with("http://myserver:4533"),
            "url must use the configured base_url"
        );
    }

    #[test]
    fn test_stream_url_base_url_trailing_slash_normalised() {
        // SubsonicClient::new strips trailing slashes from base_url.
        let client_with_slash = SubsonicClient::new("http://myserver:4533/", "u", "p");
        let client_no_slash = SubsonicClient::new("http://myserver:4533", "u", "p");

        let url_with = client_with_slash.stream_url("1");
        let url_without = client_no_slash.stream_url("1");

        // Both should produce the same path prefix (no double slash).
        assert!(
            url_with.contains("/rest/stream"),
            "should not have double slash"
        );
        assert!(!url_with.contains("//rest"), "should not have double slash");
        // Both base URLs normalise to the same path structure.
        assert_eq!(
            url_with.split('?').next(),
            url_without.split('?').next(),
            "path segment should be identical regardless of trailing slash"
        );
    }

    // --- SubsonicAlbumFull deserialization ---

    #[test]
    fn test_deserialize_album_full_with_songs() {
        let json = r#"{
            "id": "5",
            "name": "Kind of Blue",
            "artist": "Miles Davis",
            "artistId": "20",
            "year": 1959,
            "genre": "Jazz",
            "songCount": 5,
            "created": "2021-06-01T00:00:00",
            "song": [
                {"id": "101", "title": "So What"},
                {"id": "102", "title": "Freddie Freeloader"},
                {"id": "103", "title": "Blue in Green"}
            ]
        }"#;

        let album: SubsonicAlbumFull = serde_json::from_str(json).unwrap();

        assert_eq!(album.id, "5");
        assert_eq!(album.name, "Kind of Blue");
        assert_eq!(album.artist.as_deref(), Some("Miles Davis"));
        assert_eq!(album.year, Some(1959));
        assert_eq!(album.song.len(), 3);
        assert_eq!(album.song[0].title, "So What");
        assert_eq!(album.song[2].id, "103");
    }

    #[test]
    fn test_deserialize_album_full_empty_song_list() {
        // When `song` key is absent, the #[serde(default)] should yield an empty Vec.
        let json = r#"{"id": "9", "name": "No Tracks Yet"}"#;

        let album: SubsonicAlbumFull = serde_json::from_str(json).unwrap();

        assert_eq!(album.id, "9");
        assert!(album.song.is_empty(), "song list should default to empty");
    }

    // --- SubsonicSearchResult deserialization ---

    #[test]
    fn test_deserialize_search_result_mixed() {
        let json = r#"{
            "artist": [{"id": "1", "name": "Artist One"}],
            "album":  [{"id": "2", "name": "Album One"}],
            "song":   [{"id": "3", "title": "Song One"}]
        }"#;

        let result: SubsonicSearchResult = serde_json::from_str(json).unwrap();

        assert_eq!(result.artist.len(), 1);
        assert_eq!(result.artist[0].name, "Artist One");
        assert_eq!(result.album.len(), 1);
        assert_eq!(result.album[0].name, "Album One");
        assert_eq!(result.song.len(), 1);
        assert_eq!(result.song[0].title, "Song One");
    }

    #[test]
    fn test_deserialize_search_result_defaults_to_empty() {
        // All three lists are #[serde(default)], so an empty object is valid.
        let result: SubsonicSearchResult = serde_json::from_str("{}").unwrap();

        assert!(result.artist.is_empty());
        assert!(result.album.is_empty());
        assert!(result.song.is_empty());
    }
}

/// Generate a random hex salt string for Subsonic auth.
fn random_salt() -> String {
    let mut buf = [0u8; 12];
    getrandom::getrandom(&mut buf).expect("failed to generate random salt");
    buf.iter().map(|b| format!("{:02x}", b)).collect()
}
