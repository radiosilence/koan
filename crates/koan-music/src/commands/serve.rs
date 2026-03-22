//! Subsonic-compatible server — exposes the koan library over the Subsonic REST API.
//!
//! Implements a subset of the Subsonic API (v1.16.1) so that Subsonic clients
//! (DSub, Symfonium, play:Sub, etc.) can browse, search, stream, star, and
//! scrobble against a koan library.
//!
//! Not yet wired into the CLI — this module is scaffolding + integration tests.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde::Serialize;

use koan_core::db::connection::Database;
use koan_core::db::queries;

// ---------------------------------------------------------------------------
// Subsonic API version
// ---------------------------------------------------------------------------

const API_VERSION: &str = "1.16.1";
const SERVER_TYPE: &str = "koan";

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

/// Server state shared across all handlers via axum State.
#[derive(Clone)]
pub struct ServeState {
    pub db_path: PathBuf,
    pub password: String,
}

impl ServeState {
    fn open_db(&self) -> Result<Database, SubsonicError> {
        Database::open(&self.db_path).map_err(|e| SubsonicError::Internal(e.to_string()))
    }
}

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum SubsonicError {
    /// Subsonic error code 10: required parameter missing.
    MissingParam(String),
    /// Subsonic error code 40: wrong username or password.
    AuthFailed,
    /// Subsonic error code 70: data not found.
    NotFound(String),
    /// Internal server error.
    Internal(String),
}

impl SubsonicError {
    fn code(&self) -> i32 {
        match self {
            Self::MissingParam(_) => 10,
            Self::AuthFailed => 40,
            Self::NotFound(_) => 70,
            Self::Internal(_) => 0,
        }
    }

    fn message(&self) -> String {
        match self {
            Self::MissingParam(p) => format!("Required parameter is missing: {p}"),
            Self::AuthFailed => "Wrong username or password.".into(),
            Self::NotFound(what) => format!("{what} not found."),
            Self::Internal(msg) => msg.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Response format
// ---------------------------------------------------------------------------

/// Query params common to every Subsonic request.
#[derive(Debug, serde::Deserialize, Default)]
struct SubsonicParams {
    /// Username
    u: Option<String>,
    /// Auth token (md5(password + salt))
    t: Option<String>,
    /// Salt
    s: Option<String>,
    /// API version
    v: Option<String>,
    /// Client name
    c: Option<String>,
    /// Response format: xml (default) or json
    f: Option<String>,
}

impl SubsonicParams {
    fn wants_json(&self) -> bool {
        self.f.as_deref() == Some("json")
    }
}

/// Top-level Subsonic response envelope.
#[derive(Serialize)]
struct SubsonicResponse {
    status: &'static str,
    version: &'static str,
    #[serde(rename = "type")]
    server_type: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<SubsonicErrorBody>,
    #[serde(flatten)]
    body: SubsonicBody,
}

#[derive(Serialize)]
struct SubsonicErrorBody {
    code: i32,
    message: String,
}

/// The payload portion of a Subsonic response. Each variant maps to a different
/// endpoint's response shape.
#[derive(Serialize, Default)]
struct SubsonicBody {
    #[serde(skip_serializing_if = "Option::is_none", rename = "artists")]
    artists: Option<ArtistsBody>,
    #[serde(skip_serializing_if = "Option::is_none")]
    album: Option<AlbumBody>,
    #[serde(skip_serializing_if = "Option::is_none")]
    song: Option<SongEntry>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "searchResult3")]
    search_result3: Option<SearchResult3Body>,
}

#[derive(Serialize)]
struct ArtistsBody {
    index: Vec<ArtistIndex>,
}

#[derive(Serialize)]
struct ArtistIndex {
    name: String,
    artist: Vec<ArtistEntry>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ArtistEntry {
    id: String,
    name: String,
    album_count: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AlbumBody {
    id: String,
    name: String,
    artist: String,
    artist_id: String,
    song_count: i64,
    year: Option<String>,
    genre: Option<String>,
    song: Vec<SongEntry>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct SongEntry {
    id: String,
    title: String,
    album: String,
    artist: String,
    track: Option<i32>,
    disc_number: Option<i32>,
    duration: Option<i64>,
    bit_rate: Option<i32>,
    suffix: Option<String>,
    content_type: Option<String>,
    album_id: Option<String>,
    artist_id: Option<String>,
}

#[derive(Serialize)]
struct SearchResult3Body {
    artist: Vec<ArtistEntry>,
    album: Vec<SearchAlbumEntry>,
    song: Vec<SongEntry>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct SearchAlbumEntry {
    id: String,
    name: String,
    artist: String,
}

fn track_to_song_entry(t: &queries::TrackRow) -> SongEntry {
    SongEntry {
        id: t.id.to_string(),
        title: t.title.clone(),
        album: t.album_title.clone(),
        artist: t.artist_name.clone(),
        track: t.track_number,
        disc_number: t.disc,
        duration: t.duration_ms.map(|ms| ms / 1000),
        bit_rate: t.bitrate,
        suffix: t.codec.as_deref().map(|c| c.to_lowercase()),
        content_type: t.codec.as_deref().map(|c| codec_to_mime(c).to_string()),
        album_id: t.album_id.map(|id| id.to_string()),
        artist_id: t.artist_id.map(|id| id.to_string()),
    }
}

fn ok_response(body: SubsonicBody) -> SubsonicResponse {
    SubsonicResponse {
        status: "ok",
        version: API_VERSION,
        server_type: SERVER_TYPE,
        error: None,
        body,
    }
}

fn error_response(err: SubsonicError) -> SubsonicResponse {
    SubsonicResponse {
        status: "failed",
        version: API_VERSION,
        server_type: SERVER_TYPE,
        error: Some(SubsonicErrorBody {
            code: err.code(),
            message: err.message(),
        }),
        body: SubsonicBody::default(),
    }
}

fn render_response(resp: SubsonicResponse, json: bool) -> Response {
    if json {
        let wrapper = serde_json::json!({ "subsonic-response": resp });
        (StatusCode::OK, axum::Json(wrapper)).into_response()
    } else {
        // XML format — use a simple serialisation. Full XML would use quick-xml,
        // but for now we produce a minimal valid XML envelope.
        let status = resp.status;
        let version = resp.version;
        let mut xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<subsonic-response status="{status}" version="{version}" type="{SERVER_TYPE}" xmlns="http://subsonic.org/restapi">"#
        );
        if let Some(err) = &resp.error {
            xml.push_str(&format!(
                r#"<error code="{}" message="{}"/>"#,
                err.code,
                quick_xml_escape(&err.message)
            ));
        }
        xml.push_str("</subsonic-response>");
        (
            StatusCode::OK,
            [("content-type", "application/xml; charset=UTF-8")],
            xml,
        )
            .into_response()
    }
}

fn quick_xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

// ---------------------------------------------------------------------------
// Param helpers
// ---------------------------------------------------------------------------

/// Extract a required `id` query param and parse it as i64.
fn require_id(id_param: &IdParam, entity: &str) -> Result<i64, SubsonicError> {
    let id_str = id_param
        .id
        .as_deref()
        .ok_or_else(|| SubsonicError::MissingParam("id".into()))?;
    id_str
        .parse()
        .map_err(|_| SubsonicError::NotFound(entity.into()))
}

// ---------------------------------------------------------------------------
// Auth
// ---------------------------------------------------------------------------

fn authenticate(params: &SubsonicParams, password: &str) -> Result<(), SubsonicError> {
    let username = params
        .u
        .as_deref()
        .ok_or_else(|| SubsonicError::MissingParam("u".into()))?;
    if username.is_empty() {
        return Err(SubsonicError::MissingParam("u".into()));
    }
    let token = params
        .t
        .as_deref()
        .ok_or_else(|| SubsonicError::MissingParam("t".into()))?;
    let salt = params
        .s
        .as_deref()
        .ok_or_else(|| SubsonicError::MissingParam("s".into()))?;

    let expected = format!("{:x}", md5::compute(format!("{password}{salt}")));
    if token != expected {
        return Err(SubsonicError::AuthFailed);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build an axum Router for the Subsonic REST API.
pub fn build_router(state: ServeState) -> Router {
    Router::new()
        .route("/rest/ping", get(handle_ping))
        .route("/rest/ping.view", get(handle_ping))
        .route("/rest/getArtists", get(handle_get_artists))
        .route("/rest/getArtists.view", get(handle_get_artists))
        .route("/rest/getAlbum", get(handle_get_album))
        .route("/rest/getAlbum.view", get(handle_get_album))
        .route("/rest/getSong", get(handle_get_song))
        .route("/rest/getSong.view", get(handle_get_song))
        .route("/rest/stream", get(handle_stream))
        .route("/rest/stream.view", get(handle_stream))
        .route("/rest/search3", get(handle_search3))
        .route("/rest/search3.view", get(handle_search3))
        .route("/rest/star", get(handle_id_stub))
        .route("/rest/star.view", get(handle_id_stub))
        .route("/rest/scrobble", get(handle_id_stub))
        .route("/rest/scrobble.view", get(handle_id_stub))
        .with_state(Arc::new(state))
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn handle_ping(
    State(state): State<Arc<ServeState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    if let Err(e) = authenticate(&params, &state.password) {
        return render_response(error_response(e), params.wants_json());
    }
    render_response(ok_response(SubsonicBody::default()), params.wants_json())
}

async fn handle_get_artists(
    State(state): State<Arc<ServeState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    if let Err(e) = authenticate(&params, &state.password) {
        return render_response(error_response(e), params.wants_json());
    }

    let db = match state.open_db() {
        Ok(db) => db,
        Err(e) => return render_response(error_response(e), params.wants_json()),
    };

    let artists = match queries::all_artists(&db.conn) {
        Ok(a) => a,
        Err(e) => {
            return render_response(
                error_response(SubsonicError::Internal(e.to_string())),
                params.wants_json(),
            );
        }
    };

    // Group artists by first letter for the index.
    let mut index_map: HashMap<String, Vec<ArtistEntry>> = HashMap::new();
    for artist in &artists {
        let first = artist
            .name
            .chars()
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_else(|| "#".into());
        let key = if first.chars().next().is_some_and(|c| c.is_alphabetic()) {
            first
        } else {
            "#".into()
        };

        // Count albums for this artist.
        let album_count = queries::albums_for_artist(&db.conn, artist.id)
            .map(|a| a.len() as i64)
            .unwrap_or(0);

        index_map.entry(key).or_default().push(ArtistEntry {
            id: artist.id.to_string(),
            name: artist.name.clone(),
            album_count,
        });
    }

    let mut indices: Vec<ArtistIndex> = index_map
        .into_iter()
        .map(|(name, artist)| ArtistIndex { name, artist })
        .collect();
    indices.sort_by(|a, b| a.name.cmp(&b.name));

    let body = SubsonicBody {
        artists: Some(ArtistsBody { index: indices }),
        ..Default::default()
    };
    render_response(ok_response(body), params.wants_json())
}

/// Extra query params for getAlbum.
#[derive(serde::Deserialize, Default)]
struct IdParam {
    id: Option<String>,
}

async fn handle_get_album(
    State(state): State<Arc<ServeState>>,
    Query(params): Query<SubsonicParams>,
    Query(id_param): Query<IdParam>,
) -> Response {
    if let Err(e) = authenticate(&params, &state.password) {
        return render_response(error_response(e), params.wants_json());
    }

    let album_id = match require_id(&id_param, "Album") {
        Ok(id) => id,
        Err(e) => return render_response(error_response(e), params.wants_json()),
    };

    let db = match state.open_db() {
        Ok(db) => db,
        Err(e) => return render_response(error_response(e), params.wants_json()),
    };

    let album = match queries::get_album(&db.conn, album_id) {
        Ok(Some(a)) => a,
        Ok(None) => {
            return render_response(
                error_response(SubsonicError::NotFound("Album".into())),
                params.wants_json(),
            );
        }
        Err(e) => {
            return render_response(
                error_response(SubsonicError::Internal(e.to_string())),
                params.wants_json(),
            );
        }
    };

    let tracks = queries::tracks_for_album(&db.conn, album_id).unwrap_or_default();

    let songs: Vec<SongEntry> = tracks.iter().map(track_to_song_entry).collect();

    let body = SubsonicBody {
        album: Some(AlbumBody {
            id: album.id.to_string(),
            name: album.title.clone(),
            artist: album.artist_name.clone(),
            artist_id: album.artist_id.to_string(),
            song_count: songs.len() as i64,
            year: album.date.clone(),
            genre: None,
            song: songs,
        }),
        ..Default::default()
    };
    render_response(ok_response(body), params.wants_json())
}

async fn handle_get_song(
    State(state): State<Arc<ServeState>>,
    Query(params): Query<SubsonicParams>,
    Query(id_param): Query<IdParam>,
) -> Response {
    if let Err(e) = authenticate(&params, &state.password) {
        return render_response(error_response(e), params.wants_json());
    }

    let song_id = match require_id(&id_param, "Song") {
        Ok(id) => id,
        Err(e) => return render_response(error_response(e), params.wants_json()),
    };

    let db = match state.open_db() {
        Ok(db) => db,
        Err(e) => return render_response(error_response(e), params.wants_json()),
    };

    let track = match queries::get_track_row(&db.conn, song_id) {
        Ok(Some(t)) => t,
        Ok(None) => {
            return render_response(
                error_response(SubsonicError::NotFound("Song".into())),
                params.wants_json(),
            );
        }
        Err(e) => {
            return render_response(
                error_response(SubsonicError::Internal(e.to_string())),
                params.wants_json(),
            );
        }
    };

    let body = SubsonicBody {
        song: Some(track_to_song_entry(&track)),
        ..Default::default()
    };
    render_response(ok_response(body), params.wants_json())
}

async fn handle_stream(
    State(state): State<Arc<ServeState>>,
    Query(params): Query<SubsonicParams>,
    Query(id_param): Query<IdParam>,
) -> Response {
    if let Err(e) = authenticate(&params, &state.password) {
        return render_response(error_response(e), params.wants_json());
    }

    let song_id = match require_id(&id_param, "Song") {
        Ok(id) => id,
        Err(e) => return render_response(error_response(e), params.wants_json()),
    };

    let db = match state.open_db() {
        Ok(db) => db,
        Err(e) => return render_response(error_response(e), params.wants_json()),
    };

    let track = match queries::get_track_row(&db.conn, song_id) {
        Ok(Some(t)) => t,
        Ok(None) => {
            return render_response(
                error_response(SubsonicError::NotFound("Song".into())),
                params.wants_json(),
            );
        }
        Err(e) => {
            return render_response(
                error_response(SubsonicError::Internal(e.to_string())),
                params.wants_json(),
            );
        }
    };

    // For now, return the path info. Full streaming requires reading the file.
    let path = track.path.unwrap_or_default();
    if path.is_empty() {
        return render_response(
            error_response(SubsonicError::NotFound("Song file".into())),
            params.wants_json(),
        );
    }

    // Stub: return 200 with the file path as a header (real impl reads the file).
    (
        StatusCode::OK,
        [
            (
                "content-type",
                track
                    .codec
                    .as_deref()
                    .map(codec_to_mime)
                    .unwrap_or("application/octet-stream")
                    .to_string(),
            ),
            ("x-koan-path", path),
        ],
        Vec::<u8>::new(),
    )
        .into_response()
}

/// Extra query params for search3.
#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Search3Params {
    query: Option<String>,
    artist_count: Option<usize>,
    album_count: Option<usize>,
    song_count: Option<usize>,
}

async fn handle_search3(
    State(state): State<Arc<ServeState>>,
    Query(params): Query<SubsonicParams>,
    Query(search): Query<Search3Params>,
) -> Response {
    if let Err(e) = authenticate(&params, &state.password) {
        return render_response(error_response(e), params.wants_json());
    }

    let query = match &search.query {
        Some(q) if !q.is_empty() => q.clone(),
        _ => {
            return render_response(
                error_response(SubsonicError::MissingParam("query".into())),
                params.wants_json(),
            );
        }
    };

    let db = match state.open_db() {
        Ok(db) => db,
        Err(e) => return render_response(error_response(e), params.wants_json()),
    };

    let tracks = queries::search_tracks(&db.conn, &query).unwrap_or_default();

    let artist_limit = search.artist_count.unwrap_or(20);
    let album_limit = search.album_count.unwrap_or(20);
    let song_limit = search.song_count.unwrap_or(20);

    // Collect unique artists and albums from matched tracks.
    let mut seen_artists: HashMap<i64, ArtistEntry> = HashMap::new();
    let mut seen_albums: HashMap<i64, SearchAlbumEntry> = HashMap::new();
    let mut songs = Vec::new();

    for t in &tracks {
        if let Some(aid) = t.artist_id
            && !seen_artists.contains_key(&aid)
            && seen_artists.len() < artist_limit
        {
            seen_artists.insert(
                aid,
                ArtistEntry {
                    id: aid.to_string(),
                    name: t.artist_name.clone(),
                    album_count: 0,
                },
            );
        }
        if let Some(alid) = t.album_id
            && !seen_albums.contains_key(&alid)
            && seen_albums.len() < album_limit
        {
            seen_albums.insert(
                alid,
                SearchAlbumEntry {
                    id: alid.to_string(),
                    name: t.album_title.clone(),
                    artist: t.artist_name.clone(),
                },
            );
        }
        if songs.len() < song_limit {
            songs.push(track_to_song_entry(t));
        }
    }

    let body = SubsonicBody {
        search_result3: Some(SearchResult3Body {
            artist: seen_artists.into_values().collect(),
            album: seen_albums.into_values().collect(),
            song: songs,
        }),
        ..Default::default()
    };
    render_response(ok_response(body), params.wants_json())
}

/// Stub handler for endpoints that require an `id` but don't yet do anything
/// beyond validating auth + params (star, scrobble, etc.).
async fn handle_id_stub(
    State(state): State<Arc<ServeState>>,
    Query(params): Query<SubsonicParams>,
    Query(id_param): Query<IdParam>,
) -> Response {
    if let Err(e) = authenticate(&params, &state.password) {
        return render_response(error_response(e), params.wants_json());
    }
    if let Err(e) = require_id(&id_param, "resource") {
        return render_response(error_response(e), params.wants_json());
    }
    render_response(ok_response(SubsonicBody::default()), params.wants_json())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn codec_to_mime(codec: &str) -> &'static str {
    match codec.to_uppercase().as_str() {
        "FLAC" => "audio/flac",
        "MP3" => "audio/mpeg",
        "AAC" | "M4A" => "audio/mp4",
        "OGG" | "VORBIS" => "audio/ogg",
        "OPUS" => "audio/opus",
        "WAV" => "audio/wav",
        "AIFF" | "AIF" => "audio/aiff",
        _ => "application/octet-stream",
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use http::Request;
    use koan_core::db::queries;
    use tempfile::TempDir;
    use tower::ServiceExt; // for oneshot()

    const TEST_PASSWORD: &str = "testpass123";

    /// Build auth query string params for a valid request.
    fn auth_query(extra: &str) -> String {
        let salt = "randomsalt";
        let token = format!("{:x}", md5::compute(format!("{TEST_PASSWORD}{salt}")));
        let base = format!("u=testuser&t={token}&s={salt}&v=1.16.1&c=testclient");
        if extra.is_empty() {
            base
        } else {
            format!("{base}&{extra}")
        }
    }

    /// Auth query for JSON responses.
    fn auth_query_json(extra: &str) -> String {
        auth_query(&format!(
            "f=json{}",
            if extra.is_empty() {
                "".into()
            } else {
                format!("&{extra}")
            }
        ))
    }

    /// Create a test DB with schema applied and return the router + temp dir.
    fn test_app() -> (Router, TempDir) {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test.db");
        let db = Database::open(&db_path).unwrap();
        koan_core::db::schema::create_tables(&db.conn).unwrap();
        drop(db);

        let state = ServeState {
            db_path,
            password: TEST_PASSWORD.into(),
        };
        (build_router(state), tmp)
    }

    /// Create a seeded test DB with 2 artists, 3 albums, 5 tracks.
    fn test_app_seeded() -> (Router, TempDir) {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test.db");
        let db = Database::open(&db_path).unwrap();
        koan_core::db::schema::create_tables(&db.conn).unwrap();

        let tracks = vec![
            ("Vordhosbn", "Aphex Twin", "Drukqs", 1, 1),
            ("Avril 14th", "Aphex Twin", "Drukqs", 2, 1),
            ("Roygbiv", "Boards of Canada", "MHTRTC", 1, 1),
            ("Aquarius", "Boards of Canada", "MHTRTC", 2, 1),
            (
                "Everything In Its Right Place",
                "Boards of Canada",
                "Geogaddi",
                1,
                1,
            ),
        ];

        for (title, artist, album, track_num, disc) in tracks {
            let meta = queries::TrackMeta {
                title: title.into(),
                artist: artist.into(),
                album_artist: Some(artist.into()),
                album: album.into(),
                track_number: Some(track_num),
                disc: Some(disc),
                date: Some("2001".into()),
                genre: Some("Electronic".into()),
                duration_ms: Some(240_000),
                path: Some(format!("/music/{artist}/{album}/{title}.flac")),
                codec: Some("FLAC".into()),
                sample_rate: Some(44100),
                bit_depth: Some(16),
                channels: Some(2),
                bitrate: Some(1000),
                size_bytes: Some(30_000_000),
                mtime: Some(1700000000),
                source: "local".into(),
                remote_id: None,
                remote_url: None,
                label: None,
            };
            queries::upsert_track(&db.conn, &meta).unwrap();
        }

        drop(db);

        let state = ServeState {
            db_path,
            password: TEST_PASSWORD.into(),
        };
        (build_router(state), tmp)
    }

    async fn get_body(resp: http::Response<Body>) -> Vec<u8> {
        use http_body_util::BodyExt;
        let body = resp.into_body();
        let bytes = body.collect().await.unwrap().to_bytes();
        bytes.to_vec()
    }

    async fn get_json(resp: http::Response<Body>) -> serde_json::Value {
        let bytes = get_body(resp).await;
        serde_json::from_slice(&bytes).unwrap()
    }

    // -----------------------------------------------------------------------
    // Auth tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn ping_valid_auth() {
        let (app, _tmp) = test_app();
        let uri = format!("/rest/ping?{}", auth_query(""));
        let resp = app
            .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn ping_json_valid_auth() {
        let (app, _tmp) = test_app();
        let uri = format!("/rest/ping?{}", auth_query_json(""));
        let resp = app
            .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let json = get_json(resp).await;
        let inner = &json["subsonic-response"];
        assert_eq!(inner["status"], "ok");
        assert_eq!(inner["version"], API_VERSION);
    }

    #[tokio::test]
    async fn ping_wrong_password_returns_error_40() {
        let (app, _tmp) = test_app();
        let bad_token = format!("{:x}", md5::compute("wrongpasswordrandomsalt"));
        let uri =
            format!("/rest/ping?u=testuser&t={bad_token}&s=randomsalt&v=1.16.1&c=test&f=json");
        let resp = app
            .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK); // Subsonic returns 200 even on auth failure

        let json = get_json(resp).await;
        let inner = &json["subsonic-response"];
        assert_eq!(inner["status"], "failed");
        assert_eq!(inner["error"]["code"], 40);
    }

    #[tokio::test]
    async fn ping_missing_auth_params_returns_error_10() {
        let (app, _tmp) = test_app();
        // Missing t and s params.
        let uri = "/rest/ping?u=testuser&v=1.16.1&c=test&f=json";
        let resp = app
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let json = get_json(resp).await;
        let inner = &json["subsonic-response"];
        assert_eq!(inner["status"], "failed");
        assert_eq!(inner["error"]["code"], 10);
    }

    #[tokio::test]
    async fn ping_missing_username_returns_error_10() {
        let (app, _tmp) = test_app();
        let uri = "/rest/ping?t=abc&s=def&v=1.16.1&c=test&f=json";
        let resp = app
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();

        let json = get_json(resp).await;
        assert_eq!(json["subsonic-response"]["status"], "failed");
        assert_eq!(json["subsonic-response"]["error"]["code"], 10);
    }

    // -----------------------------------------------------------------------
    // Response format tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn default_format_is_xml() {
        let (app, _tmp) = test_app();
        // No f= param — should default to XML.
        let uri = format!("/rest/ping?{}", auth_query(""));
        let resp = app
            .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
            .await
            .unwrap();

        let content_type = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(
            content_type.contains("xml"),
            "expected XML content-type, got: {content_type}"
        );

        let body = String::from_utf8(get_body(resp).await).unwrap();
        assert!(body.contains("<?xml"), "body should be XML: {body}");
        assert!(
            body.contains(r#"status="ok""#),
            "XML should contain status=ok: {body}"
        );
    }

    #[tokio::test]
    async fn json_format_wraps_in_subsonic_response() {
        let (app, _tmp) = test_app();
        let uri = format!("/rest/ping?{}", auth_query_json(""));
        let resp = app
            .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
            .await
            .unwrap();

        let json = get_json(resp).await;
        assert!(
            json.get("subsonic-response").is_some(),
            "JSON response must be wrapped in subsonic-response"
        );
    }

    // -----------------------------------------------------------------------
    // Ping tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn ping_returns_empty_success() {
        let (app, _tmp) = test_app();
        let uri = format!("/rest/ping?{}", auth_query_json(""));
        let resp = app
            .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
            .await
            .unwrap();

        let json = get_json(resp).await;
        let inner = &json["subsonic-response"];
        assert_eq!(inner["status"], "ok");
        assert_eq!(inner["version"], "1.16.1");
        assert_eq!(inner["type"], "koan");
        // No body fields for ping.
        assert!(inner.get("artists").is_none());
        assert!(inner.get("album").is_none());
        assert!(inner.get("song").is_none());
    }

    // -----------------------------------------------------------------------
    // Browse tests — getArtists
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn get_artists_returns_indexed_list() {
        let (app, _tmp) = test_app_seeded();
        let uri = format!("/rest/getArtists?{}", auth_query_json(""));
        let resp = app
            .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
            .await
            .unwrap();

        let json = get_json(resp).await;
        let inner = &json["subsonic-response"];
        assert_eq!(inner["status"], "ok");

        let indices = inner["artists"]["index"].as_array().unwrap();
        // Both "Aphex Twin" and "Boards of Canada" start with A and B.
        assert!(indices.len() >= 2, "expected at least 2 index entries");

        // Find the A index.
        let a_idx = indices.iter().find(|i| i["name"] == "A").unwrap();
        let a_artists = a_idx["artist"].as_array().unwrap();
        assert_eq!(a_artists.len(), 1);
        assert_eq!(a_artists[0]["name"], "Aphex Twin");

        // Find the B index.
        let b_idx = indices.iter().find(|i| i["name"] == "B").unwrap();
        let b_artists = b_idx["artist"].as_array().unwrap();
        assert_eq!(b_artists.len(), 1);
        assert_eq!(b_artists[0]["name"], "Boards of Canada");
    }

    #[tokio::test]
    async fn get_artists_empty_db() {
        let (app, _tmp) = test_app();
        let uri = format!("/rest/getArtists?{}", auth_query_json(""));
        let resp = app
            .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
            .await
            .unwrap();

        let json = get_json(resp).await;
        assert_eq!(json["subsonic-response"]["status"], "ok");
        let indices = json["subsonic-response"]["artists"]["index"]
            .as_array()
            .unwrap();
        assert!(indices.is_empty());
    }

    // -----------------------------------------------------------------------
    // Browse tests — getAlbum
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn get_album_with_tracks() {
        let (app, _tmp) = test_app_seeded();
        // Album ID 1 should be "Drukqs" (first inserted).
        let uri = format!("/rest/getAlbum?{}&id=1", auth_query_json(""));
        let resp = app
            .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
            .await
            .unwrap();

        let json = get_json(resp).await;
        let inner = &json["subsonic-response"];
        assert_eq!(inner["status"], "ok");

        let album = &inner["album"];
        assert_eq!(album["name"], "Drukqs");
        assert_eq!(album["artist"], "Aphex Twin");

        let songs = album["song"].as_array().unwrap();
        assert_eq!(songs.len(), 2);
        // Tracks should have required fields.
        assert!(!songs[0]["id"].as_str().unwrap().is_empty());
        assert!(!songs[0]["title"].as_str().unwrap().is_empty());
    }

    #[tokio::test]
    async fn get_album_nonexistent_returns_error_70() {
        let (app, _tmp) = test_app_seeded();
        let uri = format!("/rest/getAlbum?{}&id=99999", auth_query_json(""));
        let resp = app
            .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
            .await
            .unwrap();

        let json = get_json(resp).await;
        assert_eq!(json["subsonic-response"]["status"], "failed");
        assert_eq!(json["subsonic-response"]["error"]["code"], 70);
    }

    #[tokio::test]
    async fn get_album_missing_id_returns_error_10() {
        let (app, _tmp) = test_app_seeded();
        let uri = format!("/rest/getAlbum?{}", auth_query_json(""));
        let resp = app
            .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
            .await
            .unwrap();

        let json = get_json(resp).await;
        assert_eq!(json["subsonic-response"]["status"], "failed");
        assert_eq!(json["subsonic-response"]["error"]["code"], 10);
    }

    #[tokio::test]
    async fn get_album_invalid_id_type_returns_error_70() {
        let (app, _tmp) = test_app_seeded();
        let uri = format!("/rest/getAlbum?{}&id=notanumber", auth_query_json(""));
        let resp = app
            .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
            .await
            .unwrap();

        let json = get_json(resp).await;
        assert_eq!(json["subsonic-response"]["status"], "failed");
        assert_eq!(json["subsonic-response"]["error"]["code"], 70);
    }

    // -----------------------------------------------------------------------
    // getSong tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn get_song_valid_id() {
        let (app, _tmp) = test_app_seeded();
        let uri = format!("/rest/getSong?{}&id=1", auth_query_json(""));
        let resp = app
            .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
            .await
            .unwrap();

        let json = get_json(resp).await;
        let inner = &json["subsonic-response"];
        assert_eq!(inner["status"], "ok");

        let song = &inner["song"];
        assert!(!song["id"].as_str().unwrap().is_empty());
        assert!(!song["title"].as_str().unwrap().is_empty());
        assert!(!song["artist"].as_str().unwrap().is_empty());
    }

    #[tokio::test]
    async fn get_song_nonexistent_returns_error_70() {
        let (app, _tmp) = test_app_seeded();
        let uri = format!("/rest/getSong?{}&id=99999", auth_query_json(""));
        let resp = app
            .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
            .await
            .unwrap();

        let json = get_json(resp).await;
        assert_eq!(json["subsonic-response"]["status"], "failed");
        assert_eq!(json["subsonic-response"]["error"]["code"], 70);
    }

    #[tokio::test]
    async fn get_song_missing_id_returns_error_10() {
        let (app, _tmp) = test_app_seeded();
        let uri = format!("/rest/getSong?{}", auth_query_json(""));
        let resp = app
            .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
            .await
            .unwrap();

        let json = get_json(resp).await;
        assert_eq!(json["subsonic-response"]["error"]["code"], 10);
    }

    // -----------------------------------------------------------------------
    // stream tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn stream_valid_id_returns_200() {
        let (app, _tmp) = test_app_seeded();
        let uri = format!("/rest/stream?{}&id=1", auth_query(""));
        let resp = app
            .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        // Content-type should be audio.
        let ct = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(
            ct.starts_with("audio/"),
            "expected audio content type, got: {ct}"
        );
    }

    #[tokio::test]
    async fn stream_nonexistent_returns_error() {
        let (app, _tmp) = test_app_seeded();
        let uri = format!("/rest/stream?{}&id=99999&f=json", auth_query(""));
        let resp = app
            .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
            .await
            .unwrap();

        let json = get_json(resp).await;
        assert_eq!(json["subsonic-response"]["status"], "failed");
    }

    // -----------------------------------------------------------------------
    // search3 tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn search3_finds_tracks() {
        let (app, _tmp) = test_app_seeded();
        let uri = format!("/rest/search3?{}&query=Aphex", auth_query_json(""));
        let resp = app
            .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
            .await
            .unwrap();

        let json = get_json(resp).await;
        let inner = &json["subsonic-response"];
        assert_eq!(inner["status"], "ok");

        let result = &inner["searchResult3"];
        let songs = result["song"].as_array().unwrap();
        assert!(!songs.is_empty(), "search for 'Aphex' should find tracks");
        // All found songs should be by Aphex Twin.
        for song in songs {
            assert_eq!(song["artist"], "Aphex Twin");
        }
    }

    #[tokio::test]
    async fn search3_missing_query_returns_error_10() {
        let (app, _tmp) = test_app_seeded();
        let uri = format!("/rest/search3?{}", auth_query_json(""));
        let resp = app
            .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
            .await
            .unwrap();

        let json = get_json(resp).await;
        assert_eq!(json["subsonic-response"]["status"], "failed");
        assert_eq!(json["subsonic-response"]["error"]["code"], 10);
    }

    #[tokio::test]
    async fn search3_no_results() {
        let (app, _tmp) = test_app_seeded();
        let uri = format!(
            "/rest/search3?{}&query=zzzznonexistent",
            auth_query_json("")
        );
        let resp = app
            .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
            .await
            .unwrap();

        let json = get_json(resp).await;
        let inner = &json["subsonic-response"];
        assert_eq!(inner["status"], "ok");
        let result = &inner["searchResult3"];
        assert!(result["song"].as_array().unwrap().is_empty());
        assert!(result["artist"].as_array().unwrap().is_empty());
        assert!(result["album"].as_array().unwrap().is_empty());
    }

    // -----------------------------------------------------------------------
    // star / scrobble tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn star_valid_returns_ok() {
        let (app, _tmp) = test_app_seeded();
        let uri = format!("/rest/star?{}&id=1", auth_query_json(""));
        let resp = app
            .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
            .await
            .unwrap();

        let json = get_json(resp).await;
        assert_eq!(json["subsonic-response"]["status"], "ok");
    }

    #[tokio::test]
    async fn star_missing_id_returns_error_10() {
        let (app, _tmp) = test_app_seeded();
        let uri = format!("/rest/star?{}", auth_query_json(""));
        let resp = app
            .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
            .await
            .unwrap();

        let json = get_json(resp).await;
        assert_eq!(json["subsonic-response"]["error"]["code"], 10);
    }

    #[tokio::test]
    async fn scrobble_valid_returns_ok() {
        let (app, _tmp) = test_app_seeded();
        let uri = format!("/rest/scrobble?{}&id=1", auth_query_json(""));
        let resp = app
            .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
            .await
            .unwrap();

        let json = get_json(resp).await;
        assert_eq!(json["subsonic-response"]["status"], "ok");
    }

    #[tokio::test]
    async fn scrobble_missing_id_returns_error_10() {
        let (app, _tmp) = test_app_seeded();
        let uri = format!("/rest/scrobble?{}", auth_query_json(""));
        let resp = app
            .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
            .await
            .unwrap();

        let json = get_json(resp).await;
        assert_eq!(json["subsonic-response"]["error"]["code"], 10);
    }

    // -----------------------------------------------------------------------
    // .view suffix tests (Subsonic compat)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn ping_view_suffix_works() {
        let (app, _tmp) = test_app();
        let uri = format!("/rest/ping.view?{}", auth_query_json(""));
        let resp = app
            .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
            .await
            .unwrap();

        let json = get_json(resp).await;
        assert_eq!(json["subsonic-response"]["status"], "ok");
    }

    // -----------------------------------------------------------------------
    // codec_to_mime helper
    // -----------------------------------------------------------------------

    #[test]
    fn codec_to_mime_known() {
        assert_eq!(codec_to_mime("FLAC"), "audio/flac");
        assert_eq!(codec_to_mime("MP3"), "audio/mpeg");
        assert_eq!(codec_to_mime("AAC"), "audio/mp4");
        assert_eq!(codec_to_mime("OGG"), "audio/ogg");
        assert_eq!(codec_to_mime("OPUS"), "audio/opus");
        assert_eq!(codec_to_mime("WAV"), "audio/wav");
        assert_eq!(codec_to_mime("AIFF"), "audio/aiff");
    }

    #[test]
    fn codec_to_mime_unknown() {
        assert_eq!(codec_to_mime("UNKNOWN"), "application/octet-stream");
    }

    #[test]
    fn codec_to_mime_case_insensitive() {
        assert_eq!(codec_to_mime("flac"), "audio/flac");
        assert_eq!(codec_to_mime("mp3"), "audio/mpeg");
    }
}
