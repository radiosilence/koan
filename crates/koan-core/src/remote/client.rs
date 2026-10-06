use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

use serde::Deserialize;
use thiserror::Error;

use super::download::{self, DownloadError};

const API_VERSION: &str = "1.16.1";
const CLIENT_NAME: &str = "koan";
const PLAYBACK_REPORT_EXTENSION: &str = "playbackReport";

#[derive(Debug, Error)]
pub enum SubsonicError {
    #[error("http error: {0}")]
    Http(reqwest::Error),
    #[error("api error: {code} — {message}")]
    Api { code: i32, message: String },
    #[error("unexpected response format")]
    BadResponse,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("download error: {0}")]
    Download(#[from] DownloadError),
    #[error("entropy source unavailable: {0}")]
    Entropy(#[from] getrandom::Error),
}

/// Without the URL: every request is signed with the account's credentials in
/// its query, and the error's message would carry them to wherever it is shown
/// or logged.
impl From<reqwest::Error> for SubsonicError {
    fn from(e: reqwest::Error) -> Self {
        Self::Http(e.without_url())
    }
}

/// A Subsonic server and the credentials that sign requests to it.
///
/// Kept separate from `SubsonicClient` because constructing that builds two
/// blocking `reqwest` clients, each carrying its own runtime — doing so from
/// inside a tokio runtime panics. A caller that only needs a signed URL, such
/// as koan's own Subsonic proxy, holds this instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubsonicAuth {
    pub base_url: String,
    /// Who the credential signs in as. Not sent with an API key, which names
    /// its account itself, but still what the account is known by locally.
    pub username: String,
    pub credential: Credential,
}

/// What signs each request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Credential {
    Password(String),
    /// OpenSubsonic's `apiKeyAuthentication`. What redeeming a koan invite
    /// gives.
    ApiKey(String),
}

impl SubsonicAuth {
    pub fn new(base_url: &str, username: &str, password: &str) -> Self {
        Self::with(
            base_url,
            username,
            Credential::Password(password.to_string()),
        )
    }

    pub fn with(base_url: &str, username: &str, credential: Credential) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            username: username.to_string(),
            credential,
        }
    }

    /// Build auth query params: `apiKey`, or u then p (HTTPS) or t and s; then
    /// v, c, f.
    ///
    /// Over HTTPS a password goes as `p=enc:<hex>`: a koan server checks
    /// accounts against an argon2 hash, which token auth cannot be checked
    /// against. Over plain HTTP that would expose the password, so the salted
    /// token is sent instead, as every Subsonic server accepts.
    fn params(&self) -> Result<HashMap<String, String>, SubsonicError> {
        let mut params = HashMap::new();
        match &self.credential {
            Credential::ApiKey(key) => {
                params.insert("apiKey".into(), key.clone());
            }
            Credential::Password(password) => {
                params.insert("u".into(), self.username.clone());
                if self.base_url.starts_with("https://") {
                    let hex: String = password.bytes().map(|b| format!("{b:02x}")).collect();
                    params.insert("p".into(), format!("enc:{hex}"));
                } else {
                    let salt = random_salt()?;
                    let token = format!("{:x}", md5::compute(format!("{password}{salt}")));
                    params.insert("t".into(), token);
                    params.insert("s".into(), salt);
                }
            }
        }
        params.insert("v".into(), API_VERSION.into());
        params.insert("c".into(), CLIENT_NAME.into());
        params.insert("f".into(), "json".into());
        Ok(params)
    }

    /// The auth params as a query string. Every value is URL-safe as built:
    /// API keys are base64url.
    pub fn query(&self) -> Result<String, SubsonicError> {
        Ok(self
            .params()?
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join("&"))
    }

    /// Build the streaming URL for a track (doesn't make a request).
    pub fn stream_url(&self, track_id: &str) -> Result<String, SubsonicError> {
        Ok(format!(
            "{}/rest/stream?id={}&{}",
            self.base_url,
            track_id,
            self.query()?
        ))
    }
}

/// Subsonic/Navidrome API client.
///
/// Holds two HTTP clients with different timeout semantics: `http` bounds a
/// whole JSON request, which is right for small API responses read in one go;
/// `downloader` bounds only connect and per-read stalls, so a large track on a
/// slow link is never cut off for taking too long overall.
pub struct SubsonicClient {
    auth: SubsonicAuth,
    http: reqwest::blocking::Client,
    downloader: reqwest::blocking::Client,
    /// Whether the server is answering downloads, as the last of them found.
    outage: download::Outage,
    /// Whether the server offers `reportPlayback`, once it has said.
    playback_report: OnceLock<bool>,
}

/// What a playback report says the player is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackReportState {
    Playing,
    Paused,
    Stopped,
}

impl PlaybackReportState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Playing => "playing",
            Self::Paused => "paused",
            Self::Stopped => "stopped",
        }
    }
}

impl SubsonicClient {
    pub fn new(base_url: &str, username: &str, password: &str) -> Self {
        Self::from_auth(SubsonicAuth::new(base_url, username, password))
    }

    pub fn from_auth(auth: SubsonicAuth) -> Self {
        Self {
            auth,
            http: download::api_client().unwrap_or_else(|e| {
                log::warn!("falling back to default HTTP client: {}", e);
                reqwest::blocking::Client::new()
            }),
            downloader: download::download_client().unwrap_or_else(|e| {
                log::warn!("falling back to default download client: {}", e);
                reqwest::blocking::Client::new()
            }),
            outage: download::Outage::default(),
            playback_report: OnceLock::new(),
        }
    }

    /// Whether downloads from this server are waiting out an outage.
    pub fn outage(&self) -> &download::Outage {
        &self.outage
    }

    fn auth_params(&self) -> Result<HashMap<String, String>, SubsonicError> {
        self.auth.params()
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
        let url = format!("{}/rest/{}", self.auth.base_url, endpoint);
        let mut params = self.auth_params()?;
        for (k, v) in extra {
            params.insert((*k).to_string(), (*v).to_string());
        }

        let resp: SubsonicResponseWrapper = self.http.get(&url).query(&params).send()?.json()?;
        resp.subsonic_response.ok()
    }

    /// As `get_with_params`, with `form` in a POST body (OpenSubsonic's
    /// `formPost`) rather than the query string, for a secret that should not
    /// reach a proxy's access log.
    fn post_form(
        &self,
        endpoint: &str,
        form: &[(&str, &str)],
    ) -> Result<SubsonicResponse, SubsonicError> {
        let url = format!("{}/rest/{}", self.auth.base_url, endpoint);
        let params = self.auth_params()?;
        let body = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(form)
            .finish();
        let resp: SubsonicResponseWrapper = self
            .http
            .post(&url)
            .query(&params)
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(body)
            .send()?
            .json()?;
        resp.subsonic_response.ok()
    }

    /// As `get_with_params`, for a parameter given more than once: Subsonic
    /// batches by repeating `id` and `time`.
    fn get_with_pairs(
        &self,
        endpoint: &str,
        pairs: &[(&str, String)],
    ) -> Result<SubsonicResponse, SubsonicError> {
        let url = format!("{}/rest/{}", self.auth.base_url, endpoint);
        let params = self.auth_params()?;
        let resp: SubsonicResponseWrapper = self
            .http
            .get(&url)
            .query(&params)
            .query(pairs)
            .send()?
            .json()?;
        resp.subsonic_response.ok()
    }

    /// Detect a Subsonic error returned from an endpoint that should have sent
    /// binary data.
    ///
    /// Subsonic signals failure with HTTP 200 and a JSON or XML error body, so
    /// checking the status code proves nothing here — without this, an error
    /// response gets written to disk as if it were audio.
    fn reject_error_body(resp: &reqwest::blocking::Response) -> Result<(), SubsonicError> {
        let is_document = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|ct| ct.contains("json") || ct.contains("xml"));
        if is_document {
            return Err(SubsonicError::BadResponse);
        }
        if !resp.status().is_success() {
            return Err(SubsonicError::BadResponse);
        }
        Ok(())
    }

    /// Fetch cover art bytes for a song or album ID.
    ///
    /// Returns the raw image rather than a parsed response — `getCoverArt`
    /// answers with image data, not JSON, so it can't go through `get()`.
    /// `size` requests a square thumbnail; omit it for the original.
    pub fn get_cover_art(&self, id: &str, size: Option<u32>) -> Result<Vec<u8>, SubsonicError> {
        let url = format!("{}/rest/getCoverArt", self.base_url());
        let mut params = self.auth_params()?;
        params.insert("id".into(), id.to_string());
        if let Some(px) = size {
            params.insert("size".into(), px.to_string());
        }

        let resp = self.http.get(&url).query(&params).send()?;
        Self::reject_error_body(&resp)?;
        Ok(resp.bytes()?.to_vec())
    }

    /// Ping the server — verify connection and credentials.
    pub fn ping(&self) -> Result<(), SubsonicError> {
        self.get("ping")?;
        Ok(())
    }

    /// Which server this is, by its OpenSubsonic `type`. One round trip.
    pub fn server_type(&self) -> Result<Option<String>, SubsonicError> {
        Ok(self.get("ping")?.server_type)
    }

    /// What this server says it is and which OpenSubsonic extensions it
    /// offers: `ping`, then `getOpenSubsonicExtensions` when it speaks
    /// OpenSubsonic. A server that does not has no extensions to list.
    pub fn profile(&self) -> Result<crate::remote::profile::ServerProfile, SubsonicError> {
        let ping = self.get("ping")?;
        let extensions = if ping.open_subsonic {
            self.get("getOpenSubsonicExtensions")
                .ok()
                .and_then(|r| r.open_subsonic_extensions)
                .unwrap_or_default()
                .into_iter()
                .map(|e| (e.name, e.versions))
                .collect()
        } else {
            Vec::new()
        };
        Ok(crate::remote::profile::ServerProfile {
            kind: ping.server_type,
            version: ping.server_version,
            open_subsonic: ping.open_subsonic,
            extensions,
        })
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
    pub fn stream_url(&self, track_id: &str) -> Result<String, SubsonicError> {
        self.auth.stream_url(track_id)
    }

    /// Stream URL without auth params — safe for database storage.
    pub fn stream_url_template(&self, track_id: &str) -> String {
        format!("{}/rest/stream?id={}", self.auth.base_url, track_id)
    }

    /// Download a track to a local path.
    pub fn download(&self, track_id: &str, dest: &Path) -> Result<(), SubsonicError> {
        self.fetch_to_file("download", track_id, dest, None, |_, _| {})
    }

    /// Download a track with progress reporting.
    ///
    /// The callback receives `(bytes_downloaded, total_bytes)`; total is 0 when
    /// the server sends no Content-Length, and the count restarts from zero if
    /// an attempt is retried. `dest` only appears once the file is complete.
    ///
    /// A server that is not answering is waited out, however long that takes,
    /// until `cancelled` says the track is no longer wanted.
    pub fn download_with_progress(
        &self,
        track_id: &str,
        dest: &Path,
        cancelled: &dyn Fn() -> bool,
        on_progress: impl Fn(u64, u64),
    ) -> Result<(), SubsonicError> {
        let patience = download::Patience {
            outage: &self.outage,
            cancelled,
        };
        self.fetch_to_file("download", track_id, dest, Some(patience), on_progress)
    }

    /// Fetch a track through `/rest/stream` instead of `/rest/download`.
    ///
    /// `download` returns the untranscoded original and is what library sync
    /// wants from Navidrome. koan's own server implements only `stream`, so
    /// that is how the remote bridge pulls audio from a `koan serve` instance.
    pub fn stream_to_file(
        &self,
        track_id: &str,
        dest: &Path,
        on_progress: impl Fn(u64, u64),
    ) -> Result<(), SubsonicError> {
        self.fetch_to_file("stream", track_id, dest, None, on_progress)
    }

    fn fetch_to_file(
        &self,
        endpoint: &str,
        track_id: &str,
        dest: &Path,
        patience: Option<download::Patience<'_>>,
        on_progress: impl Fn(u64, u64),
    ) -> Result<(), SubsonicError> {
        let url = format!("{}/rest/{}", self.auth.base_url, endpoint);
        download::download_with_retries(
            dest,
            download::DEFAULT_ATTEMPTS,
            patience,
            || {
                // Fresh auth params per attempt — the salt must not be replayed.
                let mut params = self
                    .auth_params()
                    .map_err(|e| download::DownloadError::Request(e.to_string()))?;
                params.insert("id".into(), track_id.to_string());
                Ok(self.downloader.get(&url).query(&params))
            },
            on_progress,
        )?;
        Ok(())
    }

    /// One page of every song on the server, `size` from `offset`.
    ///
    /// An empty `search3` query lists the whole library on OpenSubsonic
    /// servers (Navidrome, koan). Older servers answer it with nothing or an
    /// error, which is the caller's cue to walk albums one at a time instead.
    pub fn all_songs_page(
        &self,
        size: u32,
        offset: u32,
    ) -> Result<Vec<SubsonicSong>, SubsonicError> {
        let size = size.to_string();
        let offset = offset.to_string();
        let resp = self.get_with_params(
            "search3",
            &[
                ("query", ""),
                ("artistCount", "0"),
                ("albumCount", "0"),
                ("songCount", &size),
                ("songOffset", &offset),
            ],
        )?;
        Ok(resp.search_result3.map(|r| r.song).unwrap_or_default())
    }

    /// How many songs the server says it has, from `getScanStatus`. `None`
    /// where the server does not count.
    pub fn song_count(&self) -> Result<Option<u64>, SubsonicError> {
        let resp = self.get("getScanStatus")?;
        Ok(resp.scan_status.and_then(|s| s.count))
    }

    /// When the server's library last changed, in milliseconds, as
    /// `getIndexes` reports it. Asked `since` the version last seen, so a
    /// library that has not moved answers with the timestamp alone. `None`
    /// where the server gives none.
    pub fn library_modified(&self, since: Option<i64>) -> Result<Option<i64>, SubsonicError> {
        let since = since.map(|v| v.to_string());
        let params: Vec<(&str, &str)> = since
            .as_deref()
            .map(|v| vec![("ifModifiedSince", v)])
            .unwrap_or_default();
        let resp = self.get_with_params("getIndexes", &params)?;
        Ok(resp.indexes.and_then(|i| i.last_modified))
    }

    /// Search for tracks/albums/artists.
    pub fn search(&self, query: &str) -> Result<SubsonicSearchResult, SubsonicError> {
        let resp = self.get_with_params("search3", &[("query", query)])?;
        Ok(resp.search_result3.unwrap_or_default())
    }

    /// Scrobble a play that counts, dated to `heard_at_ms` (ms since the
    /// epoch the listen began).
    pub fn scrobble(&self, track_id: &str, heard_at_ms: u64) -> Result<(), SubsonicError> {
        let at = heard_at_ms.to_string();
        self.get_with_params(
            "scrobble",
            &[("id", track_id), ("submission", "true"), ("time", &at)],
        )?;
        Ok(())
    }

    /// Scrobble several plays in one request, `(track id, started at ms)`.
    pub fn scrobble_many(&self, plays: &[(&str, i64)]) -> Result<(), SubsonicError> {
        let mut pairs = vec![("submission", "true".to_owned())];
        for (id, at) in plays {
            pairs.push(("id", (*id).to_owned()));
            pairs.push(("time", at.to_string()));
        }
        self.get_with_pairs("scrobble", &pairs)?;
        Ok(())
    }

    /// Tell the server where playback of a track stands, so its Now Playing
    /// follows pauses, seeks and stops.
    ///
    /// Uses OpenSubsonic's `reportPlayback` where the server offers it, always
    /// with `ignoreScrobble`: koan decides what counts as a play and scrobbles
    /// it itself. Elsewhere only `playing` can be said, as a now-playing
    /// `scrobble` from `position` (seconds, as Navidrome reads it); a pause or
    /// a stop has no equivalent there and is not sent.
    pub fn report_playback(
        &self,
        track_id: &str,
        state: PlaybackReportState,
        position_ms: u64,
    ) -> Result<(), SubsonicError> {
        if self.supports_playback_report() {
            let position = position_ms.to_string();
            self.get_with_params(
                "reportPlayback",
                &[
                    ("mediaId", track_id),
                    ("mediaType", "song"),
                    ("positionMs", &position),
                    ("state", state.as_str()),
                    ("ignoreScrobble", "true"),
                ],
            )?;
            return Ok(());
        }
        if state == PlaybackReportState::Playing {
            let position = (position_ms / 1000).to_string();
            self.get_with_params(
                "scrobble",
                &[
                    ("id", track_id),
                    ("submission", "false"),
                    ("position", &position),
                ],
            )?;
        }
        Ok(())
    }

    /// Whether the server advertises the `playbackReport` extension. Asked
    /// once per client; any answer, including an error, is kept. Only a
    /// failure to reach the server at all is asked again next time.
    fn supports_playback_report(&self) -> bool {
        if let Some(&known) = self.playback_report.get() {
            return known;
        }
        let supported = match self.get("getOpenSubsonicExtensions") {
            Ok(resp) => resp.has_extension(PLAYBACK_REPORT_EXTENSION),
            Err(SubsonicError::Http(e)) if e.is_connect() || e.is_timeout() => return false,
            Err(_) => false,
        };
        *self.playback_report.get_or_init(|| supported)
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

    /// Everything the server has starred: songs, albums and artists.
    ///
    /// Subsonic returns all three from one call, so asking for songs alone
    /// leaves a starred album invisible to us for no saving.
    pub fn get_starred_all(&self) -> Result<SubsonicStarred, SubsonicError> {
        let resp = self.get("getStarred2")?;
        Ok(resp.starred2.unwrap_or_default())
    }

    /// Star an album. Subsonic keys this off a different parameter to a song —
    /// `id` would be read as a track and silently star nothing.
    pub fn star_album(&self, album_id: &str) -> Result<(), SubsonicError> {
        self.get_with_params("star", &[("albumId", album_id)])?;
        Ok(())
    }

    pub fn unstar_album(&self, album_id: &str) -> Result<(), SubsonicError> {
        self.get_with_params("unstar", &[("albumId", album_id)])?;
        Ok(())
    }

    pub fn star_artist(&self, artist_id: &str) -> Result<(), SubsonicError> {
        self.get_with_params("star", &[("artistId", artist_id)])?;
        Ok(())
    }

    pub fn unstar_artist(&self, artist_id: &str) -> Result<(), SubsonicError> {
        self.get_with_params("unstar", &[("artistId", artist_id)])?;
        Ok(())
    }

    /// Create a sharing link for one or more resources (songs, albums, etc).
    /// Returns the created share including its ID which forms the public URL.
    pub fn create_share(
        &self,
        ids: &[&str],
        description: Option<&str>,
    ) -> Result<SubsonicShare, SubsonicError> {
        let url = format!("{}/rest/createShare", self.auth.base_url);
        let mut params = self.auth_params()?;
        if let Some(desc) = description {
            params.insert("description".into(), desc.to_string());
        }

        // Subsonic API takes `id` as a repeated param for multiple resources.
        let mut query: Vec<(String, String)> = params.into_iter().collect();
        for id in ids {
            query.push(("id".into(), (*id).to_string()));
        }

        let resp: SubsonicResponseWrapper = self.http.get(&url).query(&query).send()?.json()?;

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

        inner
            .shares
            .and_then(|s| s.share.into_iter().next())
            .ok_or(SubsonicError::BadResponse)
    }

    /// Get similar songs for a track (Subsonic getSimilarSongs2 endpoint).
    /// Returns up to `count` similar songs based on the server's algorithm.
    pub fn get_similar_songs(
        &self,
        song_id: &str,
        count: usize,
    ) -> Result<Vec<SubsonicSong>, SubsonicError> {
        let count_str = count.to_string();
        let resp = self.get_with_params(
            "getSimilarSongs2",
            &[("id", song_id), ("count", &count_str)],
        )?;
        Ok(resp.similar_songs2.and_then(|s| s.song).unwrap_or_default())
    }

    // --- Playlists ---------------------------------------------------------

    /// Every playlist the server will show this user, without their contents.
    pub fn get_playlists(&self) -> Result<Vec<SubsonicPlaylist>, SubsonicError> {
        let resp = self.get("getPlaylists")?;
        Ok(resp.playlists.map(|p| p.playlist).unwrap_or_default())
    }

    /// One playlist, with its songs in order.
    pub fn get_playlist(&self, id: &str) -> Result<SubsonicPlaylistFull, SubsonicError> {
        let resp = self.get_with_params("getPlaylist", &[("id", id)])?;
        resp.playlist.ok_or(SubsonicError::BadResponse)
    }

    /// Create a playlist, or replace an existing one's contents wholesale.
    ///
    /// `createPlaylist` is the only Subsonic call that can set a playlist's
    /// order: `updatePlaylist` appends and removes by index, which cannot
    /// express a reorder. Passing `playlist_id` turns this into "these songs,
    /// in this order, from now on", which is exactly what koan has after any
    /// edit — so every push takes this path and there is one way for the two
    /// sides to disagree instead of five.
    pub fn create_playlist(
        &self,
        playlist_id: Option<&str>,
        name: &str,
        song_ids: &[String],
    ) -> Result<Option<SubsonicPlaylistFull>, SubsonicError> {
        let url = format!("{}/rest/createPlaylist", self.auth.base_url);
        let mut params = self.auth_params()?;
        match playlist_id {
            Some(id) => {
                params.insert("playlistId".into(), id.to_string());
                // Navidrome keeps the stored name when updating, but a rename
                // that happened offline has to travel somehow.
                params.insert("name".into(), name.to_string());
            }
            None => {
                params.insert("name".into(), name.to_string());
            }
        }

        // Repeated `songId`, in order — that order is the playlist.
        let mut query: Vec<(String, String)> = params.into_iter().collect();
        for id in song_ids {
            query.push(("songId".into(), id.clone()));
        }

        let resp: SubsonicResponseWrapper = self.http.get(&url).query(&query).send()?.json()?;
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
        // Servers before 1.14.0 answer with an empty body, so an absent
        // playlist here is not an error — only a caller that needed the new id
        // has a problem, and it says so itself.
        Ok(inner.playlist)
    }

    /// Change what can be changed without touching the song list.
    pub fn update_playlist(
        &self,
        id: &str,
        name: Option<&str>,
        comment: Option<&str>,
        public: Option<bool>,
    ) -> Result<(), SubsonicError> {
        let mut extra: Vec<(&str, String)> = vec![("playlistId", id.to_string())];
        if let Some(name) = name {
            extra.push(("name", name.to_string()));
        }
        if let Some(comment) = comment {
            extra.push(("comment", comment.to_string()));
        }
        if let Some(public) = public {
            extra.push(("public", public.to_string()));
        }
        let borrowed: Vec<(&str, &str)> = extra.iter().map(|(k, v)| (*k, v.as_str())).collect();
        self.get_with_params("updatePlaylist", &borrowed)?;
        Ok(())
    }

    pub fn delete_playlist(&self, id: &str) -> Result<(), SubsonicError> {
        self.get_with_params("deletePlaylist", &[("id", id)])?;
        Ok(())
    }

    // -- Accounts: koan servers only, and only for an admin --

    pub fn koan_users(&self) -> Result<Vec<KoanUser>, SubsonicError> {
        Ok(self
            .get("koanUsers")?
            .users
            .map(|u| u.user)
            .unwrap_or_default())
    }

    /// `role` is `admin`, `user` or `readonly`.
    pub fn koan_create_user(
        &self,
        username: &str,
        role: &str,
    ) -> Result<KoanInvite, SubsonicError> {
        self.get_with_params("koanCreateUser", &[("username", username), ("role", role)])?
            .invite
            .ok_or(SubsonicError::BadResponse)
    }

    /// With `reset`, the account gets a new password, which comes back with the
    /// invite, and its devices sign out.
    pub fn koan_invite(&self, username: &str, reset: bool) -> Result<KoanInvite, SubsonicError> {
        let reset = if reset { "true" } else { "false" };
        self.get_with_params("koanInvite", &[("username", username), ("reset", reset)])?
            .invite
            .ok_or(SubsonicError::BadResponse)
    }

    /// Revoke the API key this client signs in with (`koanRevokeKey`).
    pub fn koan_revoke_own_key(&self) -> Result<(), SubsonicError> {
        self.get("koanRevokeKey")?;
        Ok(())
    }

    /// The device waiting on pairing `pair`, an id or a code, and where it
    /// asked from (`koanPairInfo`).
    pub fn koan_pair_info(&self, pair: &str) -> Result<KoanPair, SubsonicError> {
        self.get_with_params("koanPairInfo", &[("pair", pair)])?
            .pair
            .ok_or(SubsonicError::BadResponse)
    }

    /// Sign the device waiting on `pair` in as this account, or with
    /// `decline`, turn it away (`koanPairApprove`). Answers with its name.
    pub fn koan_pair_approve(&self, pair: &str, decline: bool) -> Result<String, SubsonicError> {
        let decline = if decline { "true" } else { "false" };
        self.get_with_params("koanPairApprove", &[("pair", pair), ("decline", decline)])?
            .pair
            .map(|p| p.device)
            .ok_or(SubsonicError::BadResponse)
    }

    pub fn koan_set_user_role(&self, username: &str, role: &str) -> Result<(), SubsonicError> {
        self.get_with_params("koanSetUserRole", &[("username", username), ("role", role)])?;
        Ok(())
    }

    pub fn koan_delete_user(&self, username: &str) -> Result<(), SubsonicError> {
        self.get_with_params("koanDeleteUser", &[("username", username)])?;
        Ok(())
    }

    /// Have the server hand `command` (a link command, as JSON) to the device
    /// `to` on this account: a koan extension, `koanDevices`.
    pub fn koan_command(&self, to: &str, command: &str) -> Result<(), SubsonicError> {
        self.get_with_params("koanCommand", &[("to", to), ("command", command)])?;
        Ok(())
    }

    // -- Play history: koan servers offering `koanHistory` --

    /// The account's plays and forgettings after `since`, at most `count` of
    /// each.
    pub fn koan_history(
        &self,
        since: crate::db::queries::HistoryCursor,
        count: u32,
    ) -> Result<KoanHistoryPage, SubsonicError> {
        self.get_with_params(
            "koanHistory",
            &[("since", &since.to_string()), ("count", &count.to_string())],
        )?
        .koan_history
        .ok_or(SubsonicError::BadResponse)
    }

    /// Forget these plays, `(track id, started at ms)`, for every device on
    /// the account.
    pub fn koan_forget_plays(&self, plays: &[(&str, i64)]) -> Result<(), SubsonicError> {
        let mut pairs = Vec::new();
        for (id, at) in plays {
            pairs.push(("id", (*id).to_owned()));
            pairs.push(("time", at.to_string()));
        }
        self.get_with_pairs("koanForgetPlays", &pairs)?;
        Ok(())
    }

    /// Forget every play up to `at_ms`, for every device on the account.
    pub fn koan_forget_plays_through(&self, at_ms: i64) -> Result<(), SubsonicError> {
        self.get_with_params("koanForgetPlays", &[("through", &at_ms.to_string())])?;
        Ok(())
    }

    // -- Scrobbling: koan servers offering `koanScrobbling` --

    /// Where the account's plays are forwarded.
    pub fn koan_scrobbling(&self) -> Result<KoanScrobbling, SubsonicError> {
        self.get("koanScrobbling")?
            .koan_scrobbling
            .ok_or(SubsonicError::BadResponse)
    }

    /// Connect the account's ListenBrainz with its user token, which the
    /// server checks with ListenBrainz before keeping.
    pub fn koan_scrobbling_connect(&self, token: &str) -> Result<KoanScrobbling, SubsonicError> {
        self.post_form("koanScrobblingConnect", &[("token", token)])?
            .koan_scrobbling
            .ok_or(SubsonicError::BadResponse)
    }

    pub fn koan_scrobbling_disconnect(&self) -> Result<KoanScrobbling, SubsonicError> {
        self.get("koanScrobblingDisconnect")?
            .koan_scrobbling
            .ok_or(SubsonicError::BadResponse)
    }

    pub fn auth(&self) -> &SubsonicAuth {
        &self.auth
    }

    /// The configured server base URL (for constructing share links etc).
    pub fn base_url(&self) -> &str {
        &self.auth.base_url
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
    /// The OpenSubsonic server name ("navidrome", "koan"); absent on servers
    /// that predate OpenSubsonic.
    #[serde(rename = "type")]
    server_type: Option<String>,
    server_version: Option<String>,
    #[serde(default)]
    open_subsonic: bool,
    open_subsonic_extensions: Option<Vec<SubsonicExtension>>,
    error: Option<SubsonicApiError>,
    artists: Option<SubsonicArtists>,
    album: Option<SubsonicAlbumFull>,
    album_list2: Option<SubsonicAlbumList>,
    search_result3: Option<SubsonicSearchResult>,
    starred2: Option<SubsonicStarred>,
    shares: Option<SubsonicShares>,
    similar_songs2: Option<SubsonicSimilarSongs>,
    playlists: Option<SubsonicPlaylists>,
    playlist: Option<SubsonicPlaylistFull>,
    scan_status: Option<SubsonicScanStatus>,
    indexes: Option<SubsonicIndexes>,
    users: Option<KoanUsers>,
    invite: Option<KoanInvite>,
    join: Option<KoanJoined>,
    pair: Option<KoanPair>,
    koan_history: Option<KoanHistoryPage>,
    koan_scrobbling: Option<KoanScrobbling>,
}

/// The services a koan server forwards the account's plays to
/// (`koanScrobbling`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct KoanScrobbling {
    #[serde(default)]
    pub service: Vec<KoanScrobbleService>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct KoanScrobbleService {
    /// `listenbrainz`.
    pub name: String,
    /// The account on the service.
    pub account: String,
    /// When it was connected, in ms since the epoch.
    pub connected: i64,
    /// Plays the service has not accepted yet.
    #[serde(default)]
    pub pending: i64,
    /// Why the service stopped accepting the token, while it does.
    pub error: Option<String>,
}

/// A page of a koan server's play history (`koanHistory`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct KoanHistoryPage {
    /// Where the next page starts.
    pub cursor: String,
    #[serde(default)]
    pub more: bool,
    #[serde(default)]
    pub play: Vec<KoanPlay>,
    #[serde(default)]
    pub forgotten: Vec<KoanForgotten>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KoanPlay {
    /// The track's id.
    pub id: String,
    /// The play's place in the server's history.
    #[serde(default)]
    pub seq: Option<i64>,
    /// When it started, in ms since the epoch.
    pub played: i64,
    pub listened_ms: Option<i64>,
}

/// A play forgotten, or with no `id` every play up to `played`.
#[derive(Debug, Clone, Deserialize)]
pub struct KoanForgotten {
    pub id: Option<String>,
    pub played: i64,
}

/// A pairing a koan server holds, as `koanPairInfo` and `koanPairApprove`
/// describe it: the device, the address it asked from, and whether that
/// address is on a private network.
#[derive(Debug, Clone, Deserialize)]
pub struct KoanPair {
    pub device: String,
    #[serde(default)]
    pub from: String,
    #[serde(default)]
    pub local: bool,
}

#[derive(Debug, Deserialize)]
struct KoanUsers {
    #[serde(default)]
    user: Vec<KoanUser>,
}

/// An account on a koan server, as its admins see it.
#[derive(Debug, Clone, Deserialize)]
pub struct KoanUser {
    pub username: String,
    pub role: String,
}

/// What a koan server hands back for an invite: the token the link carries,
/// and the password when one was just made. The link is built from the
/// address the client already reaches the server at.
#[derive(Debug, Clone, Deserialize)]
pub struct KoanInvite {
    pub username: String,
    pub token: String,
    pub password: Option<String>,
}

/// An invite redeemed: the account it was for and the API key made for this
/// device.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KoanJoined {
    pub username: String,
    pub api_key: String,
}

/// Trade a koan invite token for an API key (`koanJoin`), naming the key for
/// `device`. Unauthenticated: the token is the credential.
pub fn redeem_invite(
    base_url: &str,
    token: &str,
    device: &str,
) -> Result<KoanJoined, SubsonicError> {
    let url = format!("{}/rest/koanJoin", base_url.trim_end_matches('/'));
    let resp: SubsonicResponseWrapper = download::api_client()?
        .get(&url)
        .query(&[
            ("invite", token),
            ("name", device),
            ("v", API_VERSION),
            ("c", CLIENT_NAME),
            ("f", "json"),
        ])
        .send()?
        .json()?;
    resp.subsonic_response
        .ok()?
        .join
        .ok_or(SubsonicError::BadResponse)
}

/// Whether the server at `base_url` lists `extension`, asked without
/// credentials: OpenSubsonic answers `getOpenSubsonicExtensions` to anyone, so
/// a client can learn what a server is before it sends a password to it.
pub fn offers_unsigned(base_url: &str, extension: &str) -> Result<bool, SubsonicError> {
    let url = format!(
        "{}/rest/getOpenSubsonicExtensions",
        base_url.trim_end_matches('/')
    );
    let resp: SubsonicResponseWrapper = download::api_client()?
        .get(&url)
        .query(&[("v", API_VERSION), ("c", CLIENT_NAME), ("f", "json")])
        .send()?
        .json()?;
    Ok(resp.subsonic_response.ok()?.has_extension(extension))
}

/// Trade an account's password for an API key named for `device`
/// (`koanSignIn`). The password goes as `p=enc:`, over plain HTTP too: this
/// once, so that it is never sent again. Only for a server that offers
/// `profile::SIGN_IN`.
pub fn koan_sign_in(
    base_url: &str,
    username: &str,
    password: &str,
    device: &str,
) -> Result<KoanJoined, SubsonicError> {
    let url = format!("{}/rest/koanSignIn", base_url.trim_end_matches('/'));
    let hex: String = password.bytes().map(|b| format!("{b:02x}")).collect();
    let p = format!("enc:{hex}");
    let resp: SubsonicResponseWrapper = download::api_client()?
        .get(&url)
        .query(&[
            ("u", username),
            ("p", &p),
            ("name", device),
            ("v", API_VERSION),
            ("c", CLIENT_NAME),
            ("f", "json"),
        ])
        .send()?
        .json()?;
    resp.subsonic_response
        .ok()?
        .join
        .ok_or(SubsonicError::BadResponse)
}

impl SubsonicResponse {
    fn ok(self) -> Result<Self, SubsonicError> {
        if self.status == "ok" {
            return Ok(self);
        }
        Err(self
            .error
            .map_or(SubsonicError::BadResponse, |err| SubsonicError::Api {
                code: err.code,
                message: err.message,
            }))
    }

    fn has_extension(&self, name: &str) -> bool {
        self.open_subsonic_extensions
            .iter()
            .flatten()
            .any(|ext| ext.name == name)
    }
}

#[derive(Debug, Deserialize)]
struct SubsonicExtension {
    name: String,
    #[serde(default)]
    versions: Vec<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubsonicIndexes {
    last_modified: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct SubsonicScanStatus {
    count: Option<u64>,
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
    // OpenSubsonic. Both arrive in `getArtists`, so keeping them costs no
    // extra request.
    #[serde(default, deserialize_with = "non_empty")]
    pub music_brainz_id: Option<String>,
    #[serde(default, deserialize_with = "non_empty")]
    pub sort_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubsonicAlbum {
    pub id: String,
    pub name: String,
    pub artist: Option<String>,
    pub artist_id: Option<String>,
    pub song_count: Option<i32>,
    /// Seconds, summed over the album's songs.
    #[serde(default)]
    pub duration: Option<i64>,
    pub year: Option<i32>,
    #[serde(default, deserialize_with = "non_empty")]
    pub genre: Option<String>,
    pub created: Option<String>,
    // OpenSubsonic. All of these arrive in `getAlbumList2`, which the sync
    // already pages through.
    #[serde(default, deserialize_with = "non_empty")]
    pub music_brainz_id: Option<String>,
    #[serde(default, deserialize_with = "non_empty")]
    pub sort_name: Option<String>,
    #[serde(default)]
    pub record_labels: Vec<SubsonicName>,
}

/// A bare `{"name": "..."}` object. The server uses this shape for record
/// labels, genres and moods alike.
#[derive(Debug, Clone, Deserialize)]
pub struct SubsonicName {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubsonicAlbumFull {
    pub id: String,
    pub name: String,
    pub artist: Option<String>,
    pub artist_id: Option<String>,
    pub year: Option<i32>,
    #[serde(default, deserialize_with = "non_empty")]
    pub genre: Option<String>,
    pub song_count: Option<i32>,
    pub created: Option<String>,
    #[serde(default, deserialize_with = "non_empty")]
    pub music_brainz_id: Option<String>,
    #[serde(default, deserialize_with = "non_empty")]
    pub sort_name: Option<String>,
    #[serde(default)]
    pub record_labels: Vec<SubsonicName>,
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
    #[serde(default, deserialize_with = "non_empty")]
    pub genre: Option<String>,
    pub duration: Option<i64>,
    pub bit_rate: Option<i32>,
    pub suffix: Option<String>,
    pub content_type: Option<String>,
    /// Sent by a koan server only; a suffix cannot tell ALAC from AAC.
    #[serde(default, deserialize_with = "non_empty")]
    pub codec: Option<String>,
    pub album_id: Option<String>,
    pub artist_id: Option<String>,
    // OpenSubsonic. Absent on a plain Subsonic server, which is why they are
    // Options rather than defaults — a missing sample rate is not 0 Hz.
    pub sampling_rate: Option<i32>,
    pub bit_depth: Option<i32>,
    pub channel_count: Option<i32>,
    #[serde(default, deserialize_with = "non_empty")]
    pub music_brainz_id: Option<String>,
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
    #[serde(default)]
    pub album: Vec<SubsonicAlbum>,
    #[serde(default)]
    pub artist: Vec<SubsonicArtist>,
}

#[derive(Debug, Deserialize)]
pub struct SubsonicSimilarSongs {
    pub song: Option<Vec<SubsonicSong>>,
}

#[derive(Debug, Default, Deserialize)]
struct SubsonicPlaylists {
    #[serde(default)]
    playlist: Vec<SubsonicPlaylist>,
}

/// A playlist as the server describes it, without its songs.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubsonicPlaylist {
    pub id: String,
    pub name: String,
    pub comment: Option<String>,
    pub owner: Option<String>,
    #[serde(default)]
    pub public: bool,
    pub song_count: Option<i64>,
    pub duration: Option<i64>,
    pub created: Option<String>,
    pub changed: Option<String>,
    /// OpenSubsonic: the server takes no edits to its contents (a smart
    /// playlist).
    #[serde(default)]
    pub readonly: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubsonicPlaylistFull {
    #[serde(flatten)]
    pub playlist: SubsonicPlaylist,
    #[serde(default)]
    pub entry: Vec<SubsonicSong>,
}

#[derive(Debug, Deserialize)]
struct SubsonicShares {
    #[serde(default)]
    share: Vec<SubsonicShare>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubsonicShare {
    pub id: String,
    pub url: Option<String>,
    pub description: Option<String>,
    pub username: Option<String>,
    pub created: Option<String>,
    pub expires: Option<String>,
    pub visit_count: Option<i64>,
}

/// An empty string as absent. OpenSubsonic servers send every field they
/// support, empty where there is no value — koan's own sends
/// `musicBrainzId: ""` for an untagged track. Kept as `Some("")`, that id
/// would match every other untagged track: the MusicBrainz dedup would pair
/// unrelated tracks on it, scanning the whole table per insert to do so, and
/// album enrichment would write `""` over a missing id.
fn non_empty<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(Option::<String>::deserialize(d)?.filter(|s| !s.is_empty()))
}

/// Generate a random hex salt string for Subsonic auth.
///
/// The salt goes on the wire next to `md5(password + salt)`, so it has to be
/// unpredictable — a clock- or counter-derived fallback would make the token
/// precomputable from a captured exchange. A request without OS entropy fails
/// rather than authenticating weakly.
fn random_salt() -> Result<String, getrandom::Error> {
    let mut buf = [0u8; 12];
    getrandom::fill(&mut buf)?;
    Ok(buf.iter().map(|b| format!("{:02x}", b)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(json: &str) -> SubsonicResponse {
        serde_json::from_str::<SubsonicResponseWrapper>(json)
            .unwrap()
            .subsonic_response
    }

    #[test]
    fn http_errors_leave_the_signed_url_out() {
        // Nothing listens on port 1, so this fails to connect.
        let e = reqwest::blocking::get("http://127.0.0.1:1/rest/stream?u=owner&p=enc:736563726574")
            .unwrap_err();
        assert!(e.to_string().contains("enc:736563726574"));
        let shown = SubsonicError::from(e).to_string();
        assert!(!shown.contains("enc:"), "{shown}");
        let e = reqwest::blocking::get("http://127.0.0.1:1/rest/stream?p=enc:736563726574")
            .unwrap_err();
        assert!(!DownloadError::from(e).to_string().contains("enc:"));
    }

    #[test]
    fn playback_report_is_read_from_the_advertised_extensions() {
        let navidrome = response(
            r#"{"subsonic-response": {
                "status": "ok", "version": "1.16.1", "type": "navidrome",
                "openSubsonic": true,
                "openSubsonicExtensions": [
                    {"name": "transcodeOffset", "versions": [1]},
                    {"name": "playbackReport", "versions": [1]}
                ]
            }}"#,
        );
        assert!(navidrome.has_extension(PLAYBACK_REPORT_EXTENSION));

        let without = response(
            r#"{"subsonic-response": {
                "status": "ok", "version": "1.16.1",
                "openSubsonicExtensions": [{"name": "songLyrics", "versions": [1, 2]}]
            }}"#,
        );
        assert!(!without.has_extension(PLAYBACK_REPORT_EXTENSION));

        let plain = response(r#"{"subsonic-response": {"status": "ok", "version": "1.16.1"}}"#);
        assert!(!plain.has_extension(PLAYBACK_REPORT_EXTENSION));
    }

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

    /// An OpenSubsonic server reports the figures that make a track's quality
    /// legible. Ignoring them left every remote-only track with no sample rate
    /// and no bit depth at all.
    #[test]
    fn opensubsonic_quality_fields_are_read() {
        let json = r#"{
            "id": "000XtGC7jsWEbOjDsZi4Xw",
            "title": "Anguish",
            "suffix": "flac",
            "bitRate": 913,
            "samplingRate": 44100,
            "bitDepth": 16,
            "channelCount": 2
        }"#;

        let song: SubsonicSong = serde_json::from_str(json).unwrap();

        assert_eq!(song.sampling_rate, Some(44100));
        assert_eq!(song.bit_depth, Some(16));
        assert_eq!(song.channel_count, Some(2));
    }

    /// A koan server names the codec, which the suffix cannot: ALAC and AAC
    /// are both m4a.
    #[test]
    fn a_koan_server_names_the_codec() {
        let json = r#"{"id": "1", "title": "Seawhite", "suffix": "m4a", "codec": "ALAC"}"#;
        let song: SubsonicSong = serde_json::from_str(json).unwrap();
        assert_eq!(song.codec.as_deref(), Some("ALAC"));

        let json = r#"{"id": "1", "title": "Seawhite", "suffix": "m4a"}"#;
        let song: SubsonicSong = serde_json::from_str(json).unwrap();
        assert_eq!(song.codec, None);
    }

    /// A plain Subsonic server omits them, and a missing sample rate is not
    /// 0 Hz — the fields have to stay absent rather than default.
    #[test]
    fn a_plain_subsonic_song_has_no_quality_figures() {
        let json = r#"{"id": "1", "title": "Track", "bitRate": 320}"#;
        let song: SubsonicSong = serde_json::from_str(json).unwrap();

        assert_eq!(song.sampling_rate, None);
        assert_eq!(song.bit_depth, None);
        assert_eq!(song.channel_count, None);
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
        let params = client.auth_params().unwrap();

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
    fn test_auth_params_over_https_send_the_hex_password() {
        let client = SubsonicClient::new("https://koan.example", "alice", "hi");
        let params = client.auth_params().unwrap();
        assert_eq!(params["p"], "enc:6869");
        assert!(!params.contains_key("t") && !params.contains_key("s"));
    }

    #[test]
    fn test_auth_params_with_an_api_key_name_no_user() {
        let auth = SubsonicAuth::with(
            "http://koan.example",
            "alice",
            Credential::ApiKey("k3y".into()),
        );
        let params = auth.params().unwrap();
        assert_eq!(params.get("apiKey").map(String::as_str), Some("k3y"));
        for absent in ["u", "p", "t", "s"] {
            assert!(!params.contains_key(absent), "{absent}");
        }
    }

    #[test]
    fn test_auth_params_token_is_md5_of_password_plus_salt() {
        let client = SubsonicClient::new("http://localhost:4533", "bob", "letmein");
        let params = client.auth_params().unwrap();

        let salt = &params["s"];
        let token = &params["t"];

        // The token must equal md5(password + salt).
        let expected = format!("{:x}", md5::compute(format!("letmein{}", salt)));
        assert_eq!(token, &expected);
    }

    #[test]
    fn test_auth_params_salt_is_different_each_call() {
        let client = SubsonicClient::new("http://localhost:4533", "user", "pass");
        let params1 = client.auth_params().unwrap();
        let params2 = client.auth_params().unwrap();

        // Salts should differ across calls (random); tokens will differ too.
        // There is a negligible probability they collide — acceptable in tests.
        assert_ne!(params1["s"], params2["s"], "salt should be random per call");
    }

    // --- stream_url ---

    #[test]
    fn test_stream_url_has_auth() {
        let client = SubsonicClient::new("http://myserver:4533", "user", "pass");
        let url = client.stream_url("track-123").unwrap();

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

        let url_with = client_with_slash.stream_url("1").unwrap();
        let url_without = client_no_slash.stream_url("1").unwrap();

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
    fn empty_opensubsonic_ids_are_absent() {
        let json = r#"{"id": "1", "title": "T", "musicBrainzId": ""}"#;
        let song: SubsonicSong = serde_json::from_str(json).unwrap();
        assert_eq!(song.music_brainz_id, None);

        let json = r#"{"id": "2", "name": "A", "musicBrainzId": "", "sortName": ""}"#;
        let album: SubsonicAlbum = serde_json::from_str(json).unwrap();
        assert_eq!((album.music_brainz_id, album.sort_name), (None, None));

        let json = r#"{"id": "3", "name": "A", "musicBrainzId": "mb-1"}"#;
        let album: SubsonicAlbumFull = serde_json::from_str(json).unwrap();
        assert_eq!(album.music_brainz_id.as_deref(), Some("mb-1"));
        assert_eq!(album.sort_name, None);
    }

    #[test]
    fn test_deserialize_scan_status_count() {
        let json = r#"{"subsonic-response":{"status":"ok","scanStatus":{"scanning":false,"count":49700}}}"#;
        let wrapper: SubsonicResponseWrapper = serde_json::from_str(json).unwrap();
        let status = wrapper.subsonic_response.scan_status.unwrap();
        assert_eq!(status.count, Some(49_700));
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
