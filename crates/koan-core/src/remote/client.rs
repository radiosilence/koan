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

/// Generate a random hex salt string for Subsonic auth.
fn random_salt() -> String {
    let mut buf = [0u8; 12];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        let _ = f.read_exact(&mut buf);
    }
    buf.iter().map(|b| format!("{:02x}", b)).collect()
}
