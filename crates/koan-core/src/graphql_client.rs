//! Lightweight GraphQL client for connecting to a `koan serve` instance.
//!
//! Uses blocking reqwest — call from a background thread when used from the TUI.

use std::sync::Arc;

use parking_lot::Mutex;
use reqwest::StatusCode;
use serde_json::Value;

use crate::config::Config;

/// A GraphQL client that talks to a koan server.
///
/// Clones share one session, so a token refreshed by one thread is the token
/// every other thread uses next.
#[derive(Clone)]
pub struct GraphQLClient {
    url: String,
    http: reqwest::blocking::Client,
    session: Option<Arc<Session>>,
}

/// A sign-in to a server with auth enabled: the refresh token `koan auth login`
/// stored, and the access token it last bought.
struct Session {
    tokens: Mutex<Tokens>,
    /// Called with each new refresh token. The server spends the old one on
    /// every refresh, so a copy not written back is dead.
    on_rotate: Box<dyn Fn(&str) + Send + Sync>,
}

struct Tokens {
    access: Option<String>,
    refresh: String,
    /// Why the server refused the refresh token. Set once and kept: the token
    /// will not become valid again, and the bridge polls several times a second.
    refused: Option<String>,
}

impl GraphQLClient {
    pub fn new(server_url: &str) -> Self {
        let url = format!("{}/graphql", server_url.trim_end_matches('/'));
        Self {
            url,
            http: reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("failed to build HTTP client"),
            session: None,
        }
    }

    /// A client carrying the sign-in stored in `[auth]`, when it is for this
    /// server. Rotated refresh tokens are written back through `Config::persist`.
    pub fn from_config(server_url: &str) -> Self {
        let client = Self::new(server_url);
        let cfg = Config::load().unwrap_or_default();
        if cfg.auth.refresh_token.is_empty()
            || cfg.auth.server.trim_end_matches('/') != client.server_url()
        {
            return client;
        }
        client.with_session(cfg.auth.refresh_token, |token| {
            if let Err(e) = Config::persist(|cfg| cfg.auth.refresh_token = token.to_owned()) {
                log::warn!("could not store the rotated refresh token: {e}");
            }
        })
    }

    /// Authenticate with `refresh_token`, exchanging it for an access token
    /// when the server first refuses a request.
    pub fn with_session(
        mut self,
        refresh_token: impl Into<String>,
        on_rotate: impl Fn(&str) + Send + Sync + 'static,
    ) -> Self {
        self.session = Some(Arc::new(Session {
            tokens: Mutex::new(Tokens {
                access: None,
                refresh: refresh_token.into(),
                refused: None,
            }),
            on_rotate: Box::new(on_rotate),
        }));
        self
    }

    /// Whether requests carry a stored sign-in.
    pub fn has_session(&self) -> bool {
        self.session.is_some()
    }

    /// Execute a raw GraphQL query/mutation.
    ///
    /// Access tokens are short-lived, so a 401 is answered by one refresh and
    /// one retry; a second refusal is `GraphQLError::Unauthorized`.
    pub fn execute(&self, query: &str, variables: Option<Value>) -> Result<Value, GraphQLError> {
        let mut body = serde_json::json!({ "query": query });
        if let Some(vars) = variables {
            body["variables"] = vars;
        }

        let access = self
            .session
            .as_ref()
            .and_then(|s| s.tokens.lock().access.clone());
        let mut resp = self.post(&body, access.as_deref())?;
        if resp.status() == StatusCode::UNAUTHORIZED
            && let Some(session) = &self.session
        {
            let fresh = self.refresh(session, access.as_deref())?;
            resp = self.post(&body, Some(&fresh))?;
        }

        let status = resp.status();
        if status == StatusCode::UNAUTHORIZED {
            return Err(GraphQLError::Unauthorized(reason(resp)));
        }
        if !status.is_success() {
            return Err(GraphQLError::Http(format!("{status}: {}", reason(resp))));
        }

        let resp: Value = resp.json().map_err(|e| GraphQLError::Http(e.to_string()))?;

        if let Some(errors) = resp.get("errors")
            && let Some(arr) = errors.as_array()
            && !arr.is_empty()
        {
            let msg = arr[0]
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error");
            return Err(GraphQLError::Query(msg.to_string()));
        }

        Ok(resp.get("data").cloned().unwrap_or(Value::Null))
    }

    fn post(
        &self,
        body: &Value,
        access: Option<&str>,
    ) -> Result<reqwest::blocking::Response, GraphQLError> {
        let mut req = self.http.post(&self.url).json(body);
        if let Some(token) = access {
            req = req.bearer_auth(token);
        }
        req.send().map_err(|e| GraphQLError::Http(e.to_string()))
    }

    /// A new access token, replacing `stale`.
    ///
    /// The lock is held across the request: the server spends a refresh token
    /// on use, so two threads refreshing at once would leave one holding a
    /// revoked token. A thread that waited finds `stale` already replaced and
    /// takes the new token instead.
    fn refresh(&self, session: &Session, stale: Option<&str>) -> Result<String, GraphQLError> {
        let mut tokens = session.tokens.lock();
        if let Some(access) = &tokens.access
            && Some(access.as_str()) != stale
        {
            return Ok(access.clone());
        }
        if let Some(reason) = &tokens.refused {
            return Err(GraphQLError::Unauthorized(reason.clone()));
        }

        let resp = self
            .http
            .post(format!("{}/auth/refresh", self.server_url()))
            .json(&serde_json::json!({ "refresh_token": tokens.refresh }))
            .send()
            .map_err(|e| GraphQLError::Http(e.to_string()))?;
        if resp.status() == StatusCode::UNAUTHORIZED {
            let refused = format!("the stored sign-in was refused: {}", reason(resp));
            tokens.access = None;
            tokens.refused = Some(refused.clone());
            return Err(GraphQLError::Unauthorized(refused));
        }
        if !resp.status().is_success() {
            let status = resp.status();
            return Err(GraphQLError::Http(format!(
                "/auth/refresh {status}: {}",
                reason(resp)
            )));
        }

        let body: Value = resp.json().map_err(|e| GraphQLError::Http(e.to_string()))?;
        let (Some(access), Some(refresh)) = (
            body["access_token"].as_str(),
            body["refresh_token"].as_str(),
        ) else {
            return Err(GraphQLError::Http(
                "malformed /auth/refresh response".into(),
            ));
        };
        tokens.access = Some(access.to_owned());
        tokens.refresh = refresh.to_owned();
        (session.on_rotate)(refresh);
        Ok(access.to_owned())
    }

    // -----------------------------------------------------------------------
    // Typed helpers
    // -----------------------------------------------------------------------

    pub fn now_playing(&self) -> Result<NowPlaying, GraphQLError> {
        let data = self.execute(
            "{ nowPlaying { state positionMs durationMs queueItemId \
             track { trackId title artist album codec sampleRate bitDepth bitrateKbps channels durationMs } } }",
            None,
        )?;
        let np = &data["nowPlaying"];
        Ok(NowPlaying {
            state: np["state"].as_str().unwrap_or("STOPPED").to_string(),
            position_ms: np["positionMs"].as_u64().unwrap_or(0),
            duration_ms: np["durationMs"].as_u64(),
            queue_item_id: np["queueItemId"].as_str().map(String::from),
            track: np.get("track").and_then(|t| {
                if t.is_null() {
                    return None;
                }
                Some(NowPlayingTrack {
                    track_id: t["trackId"].as_i64(),
                    title: t["title"].as_str().unwrap_or("").to_string(),
                    artist: t["artist"].as_str().unwrap_or("").to_string(),
                    album: t["album"].as_str().unwrap_or("").to_string(),
                    codec: t["codec"].as_str().unwrap_or("").to_string(),
                    sample_rate: t["sampleRate"].as_u64().unwrap_or(0) as u32,
                    bit_depth: t["bitDepth"].as_u64().map(|v| v as u16),
                    bitrate_kbps: t["bitrateKbps"].as_u64().map(|v| v as u32),
                    channels: t["channels"].as_u64().unwrap_or(0) as u16,
                    duration_ms: t["durationMs"].as_u64().unwrap_or(0),
                })
            }),
        })
    }

    pub fn queue(&self) -> Result<Vec<QueueEntry>, GraphQLError> {
        let data = self.execute(
            "{ queue { queueItemId trackId title artist album codec trackNumber disc durationMs isCurrent } }",
            None,
        )?;
        let entries = data["queue"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|e| QueueEntry {
                        queue_item_id: e["queueItemId"].as_str().unwrap_or("").to_string(),
                        track_id: e["trackId"].as_i64(),
                        title: e["title"].as_str().unwrap_or("").to_string(),
                        artist: e["artist"].as_str().unwrap_or("").to_string(),
                        album: e["album"].as_str().unwrap_or("").to_string(),
                        codec: e["codec"].as_str().map(String::from),
                        track_number: e["trackNumber"].as_i64(),
                        disc: e["disc"].as_i64(),
                        duration_ms: e["durationMs"].as_u64(),
                        is_current: e["isCurrent"].as_bool().unwrap_or(false),
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(entries)
    }

    pub fn search(&self, query: &str, limit: u32) -> Result<Vec<TrackResult>, GraphQLError> {
        let data = self.execute(
            "query($search: String!, $first: Int) { tracks(search: $search, first: $first) { edges { node { id title artist album albumId artistId disc trackNumber durationMs codec genre source } } } }",
            Some(serde_json::json!({ "search": query, "first": limit })),
        )?;
        parse_track_edges(&data["tracks"])
    }

    pub fn artists(&self) -> Result<Vec<ArtistResult>, GraphQLError> {
        let data = self.execute("{ artists { edges { node { id name } } } }", None)?;
        let edges = data["artists"]["edges"].as_array();
        Ok(edges
            .map(|arr| {
                arr.iter()
                    .map(|e| {
                        let n = &e["node"];
                        ArtistResult {
                            id: n["id"].as_i64().unwrap_or(0),
                            name: n["name"].as_str().unwrap_or("").to_string(),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    pub fn albums_for_artist(&self, artist_id: i64) -> Result<Vec<AlbumResult>, GraphQLError> {
        let data = self.execute(
            "query($artistId: Int!) { albums(artistId: $artistId) { edges { node { id title artistName date codec } } } }",
            Some(serde_json::json!({ "artistId": artist_id })),
        )?;
        parse_album_edges(&data["albums"])
    }

    pub fn tracks_for_album(&self, album_id: i64) -> Result<Vec<TrackResult>, GraphQLError> {
        let data = self.execute(
            "query($albumId: Int!) { tracks(albumId: $albumId) { edges { node { id title artist album albumId artistId disc trackNumber durationMs codec genre source } } } }",
            Some(serde_json::json!({ "albumId": album_id })),
        )?;
        parse_track_edges(&data["tracks"])
    }

    pub fn fuzzy_search(
        &self,
        query: &str,
        kind: &str,
        limit: u32,
    ) -> Result<Vec<FuzzyMatch>, GraphQLError> {
        let data = self.execute(
            "query($query: String!, $kind: FuzzySearchKind!, $limit: Int) { fuzzySearch(query: $query, kind: $kind, limit: $limit) { id name rank kind } }",
            Some(serde_json::json!({ "query": query, "kind": kind, "limit": limit })),
        )?;
        Ok(data["fuzzySearch"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|e| FuzzyMatch {
                        id: e["id"].as_i64().unwrap_or(0),
                        name: e["name"].as_str().unwrap_or("").to_string(),
                        rank: e["rank"].as_i64().unwrap_or(0) as i32,
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    // -- Mutations --

    pub fn pause(&self) -> Result<(), GraphQLError> {
        self.execute("mutation { pause { ok } }", None)?;
        Ok(())
    }

    pub fn resume(&self) -> Result<(), GraphQLError> {
        self.execute("mutation { resume { ok } }", None)?;
        Ok(())
    }

    pub fn stop(&self) -> Result<(), GraphQLError> {
        self.execute("mutation { stop { ok } }", None)?;
        Ok(())
    }

    pub fn next(&self) -> Result<(), GraphQLError> {
        self.execute("mutation { next { ok } }", None)?;
        Ok(())
    }

    pub fn previous(&self) -> Result<(), GraphQLError> {
        self.execute("mutation { previous { ok } }", None)?;
        Ok(())
    }

    pub fn seek(&self, position_ms: u64) -> Result<(), GraphQLError> {
        self.execute(
            "mutation($positionMs: Int!) { seek(positionMs: $positionMs) { ok } }",
            Some(serde_json::json!({ "positionMs": position_ms })),
        )?;
        Ok(())
    }

    pub fn play(&self, queue_item_id: &str) -> Result<(), GraphQLError> {
        self.execute(
            "mutation($queueItemId: String!) { play(queueItemId: $queueItemId) { ok } }",
            Some(serde_json::json!({ "queueItemId": queue_item_id })),
        )?;
        Ok(())
    }

    pub fn add_to_queue(&self, track_ids: &[i64]) -> Result<Vec<String>, GraphQLError> {
        let data = self.execute(
            "mutation($trackIds: [Int!]!) { addToQueue(trackIds: $trackIds) { ok addedCount queueItemIds } }",
            Some(serde_json::json!({ "trackIds": track_ids })),
        )?;
        Ok(data["addToQueue"]["queueItemIds"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default())
    }

    pub fn replace_queue(&self, track_ids: &[i64]) -> Result<Vec<String>, GraphQLError> {
        let data = self.execute(
            "mutation($trackIds: [Int!]!) { replaceQueue(trackIds: $trackIds) { ok addedCount queueItemIds } }",
            Some(serde_json::json!({ "trackIds": track_ids })),
        )?;
        Ok(data["replaceQueue"]["queueItemIds"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default())
    }

    pub fn clear_queue(&self) -> Result<(), GraphQLError> {
        self.execute("mutation { clearQueue { ok } }", None)?;
        Ok(())
    }

    pub fn favourite(&self, track_id: i64) -> Result<(), GraphQLError> {
        self.execute(
            "mutation($trackId: Int!) { favourite(trackId: $trackId) { id } }",
            Some(serde_json::json!({ "trackId": track_id })),
        )?;
        Ok(())
    }

    pub fn unfavourite(&self, track_id: i64) -> Result<(), GraphQLError> {
        self.execute(
            "mutation($trackId: Int!) { unfavourite(trackId: $trackId) { id } }",
            Some(serde_json::json!({ "trackId": track_id })),
        )?;
        Ok(())
    }

    pub fn save_queue_as_playlist(&self, name: &str) -> Result<(), GraphQLError> {
        self.execute(
            "mutation($name: String!) { saveQueueAsPlaylist(name: $name) { id } }",
            Some(serde_json::json!({ "name": name })),
        )?;
        Ok(())
    }

    pub fn play_playlist(&self, id: i64, shuffled: bool) -> Result<(), GraphQLError> {
        self.execute(
            "mutation($id: Int!, $shuffled: Boolean!)              { playPlaylist(id: $id, shuffled: $shuffled) { ok } }",
            Some(serde_json::json!({ "id": id, "shuffled": shuffled })),
        )?;
        Ok(())
    }

    pub fn enable_radio(&self) -> Result<(), GraphQLError> {
        self.execute("mutation { enableRadio { ok } }", None)?;
        Ok(())
    }

    pub fn disable_radio(&self) -> Result<(), GraphQLError> {
        self.execute("mutation { disableRadio { ok } }", None)?;
        Ok(())
    }

    pub fn library_stats(&self) -> Result<Value, GraphQLError> {
        self.execute(
            "{ libraryStats { totalTracks totalArtists totalAlbums localTracks remoteTracks cachedTracks } }",
            None,
        )
    }

    /// Server URL (without /graphql path).
    pub fn server_url(&self) -> &str {
        self.url.trim_end_matches("/graphql")
    }
}

// ---------------------------------------------------------------------------
// Result types
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum GraphQLError {
    #[error("http error: {0}")]
    Http(String),
    #[error("query error: {0}")]
    Query(String),
    #[error("unauthorised: {0}")]
    Unauthorized(String),
}

/// What the server said when it refused a request: the `message` of a JSON
/// body, or the body as text.
fn reason(resp: reqwest::blocking::Response) -> String {
    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    let message = serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|v| v["message"].as_str().map(str::to_owned))
        .unwrap_or(text);
    if message.trim().is_empty() {
        status.to_string()
    } else {
        message.trim().to_owned()
    }
}

#[derive(Debug, Clone)]
pub struct NowPlaying {
    pub state: String,
    pub position_ms: u64,
    pub duration_ms: Option<u64>,
    pub queue_item_id: Option<String>,
    pub track: Option<NowPlayingTrack>,
}

#[derive(Debug, Clone)]
pub struct NowPlayingTrack {
    /// Library row id on the server. `None` for a queue entry the server built
    /// from a file with no database row, which cannot be streamed.
    pub track_id: Option<i64>,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub codec: String,
    pub sample_rate: u32,
    pub bit_depth: Option<u16>,
    pub bitrate_kbps: Option<u32>,
    pub channels: u16,
    pub duration_ms: u64,
}

#[derive(Debug, Clone)]
pub struct QueueEntry {
    pub queue_item_id: String,
    pub track_id: Option<i64>,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub codec: Option<String>,
    pub track_number: Option<i64>,
    pub disc: Option<i64>,
    pub duration_ms: Option<u64>,
    pub is_current: bool,
}

#[derive(Debug, Clone)]
pub struct TrackResult {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_id: Option<i64>,
    pub artist_id: Option<i64>,
    pub disc: Option<i32>,
    pub track_number: Option<i32>,
    pub duration_ms: Option<i64>,
    pub codec: Option<String>,
    pub genre: Option<String>,
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct ArtistResult {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct AlbumResult {
    pub id: i64,
    pub title: String,
    pub artist_name: String,
    pub date: Option<String>,
    pub codec: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FuzzyMatch {
    pub id: i64,
    pub name: String,
    pub rank: i32,
}

// ---------------------------------------------------------------------------
// Parse helpers
// ---------------------------------------------------------------------------

fn parse_track_edges(connection: &Value) -> Result<Vec<TrackResult>, GraphQLError> {
    Ok(connection["edges"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|e| {
                    let n = &e["node"];
                    TrackResult {
                        id: n["id"].as_i64().unwrap_or(0),
                        title: n["title"].as_str().unwrap_or("").to_string(),
                        artist: n["artist"].as_str().unwrap_or("").to_string(),
                        album: n["album"].as_str().unwrap_or("").to_string(),
                        album_id: n["albumId"].as_i64(),
                        artist_id: n["artistId"].as_i64(),
                        disc: n["disc"].as_i64().map(|v| v as i32),
                        track_number: n["trackNumber"].as_i64().map(|v| v as i32),
                        duration_ms: n["durationMs"].as_i64(),
                        codec: n["codec"].as_str().map(String::from),
                        genre: n["genre"].as_str().map(String::from),
                        source: n["source"].as_str().unwrap_or("local").to_string(),
                    }
                })
                .collect()
        })
        .unwrap_or_default())
}

fn parse_album_edges(connection: &Value) -> Result<Vec<AlbumResult>, GraphQLError> {
    Ok(connection["edges"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|e| {
                    let n = &e["node"];
                    AlbumResult {
                        id: n["id"].as_i64().unwrap_or(0),
                        title: n["title"].as_str().unwrap_or("").to_string(),
                        artist_name: n["artistName"].as_str().unwrap_or("").to_string(),
                        date: n["date"].as_str().map(String::from),
                        codec: n["codec"].as_str().map(String::from),
                    }
                })
                .collect()
        })
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_constructs_url() {
        let c = GraphQLClient::new("http://localhost:4000");
        assert_eq!(c.url, "http://localhost:4000/graphql");
    }

    #[test]
    fn client_trailing_slash() {
        let c = GraphQLClient::new("http://localhost:4000/");
        assert_eq!(c.url, "http://localhost:4000/graphql");
    }

    /// A request as the test server saw it.
    struct Seen {
        path: String,
        bearer: Option<String>,
        body: String,
    }

    /// An HTTP server on a loopback port answering each request with
    /// `respond`. Returns its base URL and every request it received.
    fn serve(
        respond: impl Fn(&Seen) -> (u16, &'static str) + Send + 'static,
    ) -> (String, Arc<Mutex<Vec<String>>>) {
        use std::io::{BufRead, BufReader, Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let log = Arc::new(Mutex::new(Vec::new()));
        let seen_log = log.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let path = line.split_whitespace().nth(1).unwrap_or("").to_owned();
                let (mut bearer, mut length) = (None, 0);
                loop {
                    line.clear();
                    reader.read_line(&mut line).unwrap();
                    let header = line.trim_end();
                    if header.is_empty() {
                        break;
                    }
                    let (name, value) = header.split_once(": ").unwrap_or((header, ""));
                    match name.to_ascii_lowercase().as_str() {
                        "authorization" => {
                            bearer = value.strip_prefix("Bearer ").map(str::to_owned)
                        }
                        "content-length" => length = value.parse().unwrap_or(0),
                        _ => {}
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let seen = Seen {
                    path,
                    bearer,
                    body: String::from_utf8_lossy(&body).into_owned(),
                };
                seen_log.lock().push(seen.path.clone());
                let (code, reply) = respond(&seen);
                write!(
                    stream,
                    "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                    reply.len()
                )
                .unwrap();
            }
        });
        (base, log)
    }

    /// A server that takes `access-2` and trades `refresh-1` for it once.
    fn signed_in_server(seen: &Seen) -> (u16, &'static str) {
        match seen.path.as_str() {
            "/graphql" if seen.bearer.as_deref() == Some("access-2") => {
                (200, r#"{"data":{"ok":true}}"#)
            }
            "/auth/refresh" if seen.body.contains("refresh-1") => (
                200,
                r#"{"access_token":"access-2","refresh_token":"refresh-2"}"#,
            ),
            "/auth/refresh" => (401, r#"{"message":"invalid or expired refresh token"}"#),
            _ => (401, "missing or invalid Authorization header"),
        }
    }

    #[test]
    fn refused_request_refreshes_once_and_retries() {
        let (base, log) = serve(signed_in_server);
        let rotated = Arc::new(Mutex::new(Vec::new()));
        let stored = rotated.clone();
        let client = GraphQLClient::new(&base)
            .with_session("refresh-1", move |t| stored.lock().push(t.to_owned()));

        let data = client.execute("{ ok }", None).unwrap();
        assert_eq!(data["ok"], true);
        assert_eq!(*rotated.lock(), ["refresh-2"]);

        // The new access token is kept: no second refresh.
        client.clone().execute("{ ok }", None).unwrap();
        assert_eq!(
            *log.lock(),
            ["/graphql", "/auth/refresh", "/graphql", "/graphql"]
        );
    }

    #[test]
    fn refused_refresh_is_unauthorised() {
        let (base, log) = serve(signed_in_server);
        let client = GraphQLClient::new(&base).with_session("revoked", |_| {});

        for _ in 0..2 {
            match client.execute("{ ok }", None) {
                Err(GraphQLError::Unauthorized(msg)) => {
                    assert!(msg.contains("invalid or expired refresh token"), "{msg}")
                }
                other => panic!("expected Unauthorized, got {other:?}"),
            }
        }
        // A refused refresh token is not offered again.
        assert_eq!(*log.lock(), ["/graphql", "/auth/refresh", "/graphql"]);
    }

    #[test]
    fn unauthorised_without_a_session_says_why() {
        let (base, log) = serve(signed_in_server);

        match GraphQLClient::new(&base).execute("{ ok }", None) {
            Err(GraphQLError::Unauthorized(msg)) => {
                assert_eq!(msg, "missing or invalid Authorization header")
            }
            other => panic!("expected Unauthorized, got {other:?}"),
        }
        assert_eq!(*log.lock(), ["/graphql"]);
    }
}
