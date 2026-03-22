use std::io::Cursor;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use koan_core::config::Config;
use koan_core::db::connection::Database;
use koan_core::db::queries;
use koan_core::index::metadata::extract_cover_art;
use serde::Deserialize;
use tokio::io::AsyncReadExt as _;

use super::open_db;

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct AppState {
    db_path: std::path::PathBuf,
    config: Arc<Config>,
}

impl AppState {
    fn open_db(&self) -> Result<Database, SubsonicError> {
        Database::open(&self.db_path).map_err(|e| SubsonicError::Internal(e.to_string()))
    }
}

// ---------------------------------------------------------------------------
// Subsonic XML response helpers
// ---------------------------------------------------------------------------

const API_VERSION: &str = "1.16.1";
const SERVER_NAME: &str = "koan";

#[derive(Debug)]
enum SubsonicError {
    Auth(String),
    NotFound(String),
    MissingParam(String),
    Internal(String),
}

impl IntoResponse for SubsonicError {
    fn into_response(self) -> Response {
        let (code, message) = match self {
            SubsonicError::Auth(m) => (40, m),
            SubsonicError::NotFound(m) => (70, m),
            SubsonicError::MissingParam(m) => (10, m),
            SubsonicError::Internal(m) => (0, m),
        };
        subsonic_error_response(code, &message).into_response()
    }
}

/// Build a successful Subsonic XML response wrapping `inner_xml`.
fn subsonic_ok(inner_xml: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<subsonic-response xmlns="http://subsonic.org/restapi" status="ok" version="{}" serverVersion="{}">
{}
</subsonic-response>"#,
        API_VERSION, SERVER_NAME, inner_xml
    )
}

/// Build a Subsonic error XML response.
fn subsonic_error_response(code: i32, message: &str) -> Response {
    let body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<subsonic-response xmlns="http://subsonic.org/restapi" status="failed" version="{}">
  <error code="{}" message="{}"/>
</subsonic-response>"#,
        API_VERSION,
        code,
        xml_escape(message)
    );
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/xml; charset=utf-8")],
        body,
    )
        .into_response()
}

fn xml_response(body: String) -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/xml; charset=utf-8")],
        body,
    )
        .into_response()
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

// ---------------------------------------------------------------------------
// Auth — Subsonic MD5+salt token verification
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct AuthParams {
    u: Option<String>,
    p: Option<String>,
    t: Option<String>,
    s: Option<String>,
    #[allow(dead_code)]
    v: Option<String>,
    #[allow(dead_code)]
    c: Option<String>,
    #[allow(dead_code)]
    f: Option<String>,
}

fn verify_auth(params: &AuthParams, config: &Config) -> Result<(), SubsonicError> {
    let username = params
        .u
        .as_deref()
        .ok_or_else(|| SubsonicError::MissingParam("missing parameter: u".into()))?;

    // Check username matches configured remote username.
    if username != config.remote.username {
        return Err(SubsonicError::Auth("wrong username or password".into()));
    }

    // Get the stored password.
    let stored_password = if !config.remote.password.is_empty() {
        config.remote.password.clone()
    } else {
        koan_core::credentials::get_password(&config.remote.url)
            .map_err(|_| SubsonicError::Auth("no password configured".into()))?
    };

    // Support both token+salt auth and plaintext password.
    if let (Some(token), Some(salt)) = (params.t.as_deref(), params.s.as_deref()) {
        let expected = format!("{:x}", md5::compute(format!("{}{}", stored_password, salt)));
        if token != expected {
            return Err(SubsonicError::Auth("wrong username or password".into()));
        }
    } else if let Some(password) = params.p.as_deref() {
        // Plaintext or hex-encoded password (enc: prefix).
        let plain = if let Some(hex) = password.strip_prefix("enc:") {
            hex_decode(hex)
        } else {
            password.to_string()
        };
        if plain != stored_password {
            return Err(SubsonicError::Auth("wrong username or password".into()));
        }
    } else {
        return Err(SubsonicError::MissingParam(
            "missing authentication parameters".into(),
        ));
    }

    Ok(())
}

fn hex_decode(hex: &str) -> String {
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect();
    String::from_utf8(bytes).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// GET /rest/ping
// ---------------------------------------------------------------------------

async fn ping(
    State(state): State<AppState>,
    Query(auth): Query<AuthParams>,
) -> Result<Response, SubsonicError> {
    verify_auth(&auth, &state.config)?;
    Ok(xml_response(subsonic_ok("")))
}

// ---------------------------------------------------------------------------
// GET /rest/search3
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Search3Params {
    #[serde(flatten)]
    auth: AuthParams,
    query: Option<String>,
    artist_count: Option<u32>,
    album_count: Option<u32>,
    song_count: Option<u32>,
}

async fn search3(
    State(state): State<AppState>,
    Query(params): Query<Search3Params>,
) -> Result<Response, SubsonicError> {
    verify_auth(&params.auth, &state.config)?;

    let query = params
        .query
        .as_deref()
        .ok_or_else(|| SubsonicError::MissingParam("missing parameter: query".into()))?;

    let artist_count = params.artist_count.unwrap_or(20);
    let album_count = params.album_count.unwrap_or(20);
    let song_count = params.song_count.unwrap_or(20);

    let db = state.open_db()?;

    // FTS5 search for songs — we'll extract unique artists and albums from results.
    let total_needed = (artist_count + album_count + song_count).max(100);
    let tracks = queries::search_tracks_paged(&db.conn, query, total_needed, 0)
        .map_err(|e| SubsonicError::Internal(e.to_string()))?;

    // Collect unique artists.
    let mut seen_artists = std::collections::HashSet::new();
    let mut artist_xml = String::new();
    let mut artist_n = 0u32;
    for t in &tracks {
        if artist_n >= artist_count {
            break;
        }
        if let Some(aid) = t.artist_id
            && seen_artists.insert(aid)
        {
            artist_xml.push_str(&format!(
                r#"    <artist id="{}" name="{}"/>"#,
                aid,
                xml_escape(&t.artist_name)
            ));
            artist_xml.push('\n');
            artist_n += 1;
        }
    }

    // Collect unique albums.
    let mut seen_albums = std::collections::HashSet::new();
    let mut album_xml = String::new();
    let mut album_n = 0u32;
    for t in &tracks {
        if album_n >= album_count {
            break;
        }
        if let Some(alid) = t.album_id
            && seen_albums.insert(alid)
        {
            album_xml.push_str(&format!(
                r#"    <album id="{}" name="{}" artist="{}"/>"#,
                alid,
                xml_escape(&t.album_title),
                xml_escape(&t.album_artist_name),
            ));
            album_xml.push('\n');
            album_n += 1;
        }
    }

    // Songs.
    let mut song_xml = String::new();
    for t in tracks.iter().take(song_count as usize) {
        song_xml.push_str(&track_to_xml(t));
        song_xml.push('\n');
    }

    let inner = format!(
        "  <searchResult3>\n{}{}{}\n  </searchResult3>",
        artist_xml, album_xml, song_xml
    );
    Ok(xml_response(subsonic_ok(&inner)))
}

fn track_to_xml(t: &queries::TrackRow) -> String {
    let mut attrs = format!(
        r#"    <song id="{}" title="{}" album="{}" artist="{}""#,
        t.id,
        xml_escape(&t.title),
        xml_escape(&t.album_title),
        xml_escape(&t.artist_name),
    );
    if let Some(n) = t.track_number {
        attrs.push_str(&format!(r#" track="{}""#, n));
    }
    if let Some(d) = t.disc {
        attrs.push_str(&format!(r#" discNumber="{}""#, d));
    }
    if let Some(ms) = t.duration_ms {
        attrs.push_str(&format!(r#" duration="{}""#, ms / 1000));
    }
    if let Some(br) = t.bitrate {
        attrs.push_str(&format!(r#" bitRate="{}""#, br));
    }
    if let Some(ref codec) = t.codec {
        let (suffix, content_type) = codec_to_mime(codec);
        attrs.push_str(&format!(
            r#" suffix="{}" contentType="{}""#,
            suffix, content_type
        ));
    }
    if let Some(ref g) = t.genre {
        attrs.push_str(&format!(r#" genre="{}""#, xml_escape(g)));
    }
    if let Some(aid) = t.album_id {
        attrs.push_str(&format!(r#" albumId="{}""#, aid));
    }
    if let Some(aid) = t.artist_id {
        attrs.push_str(&format!(r#" artistId="{}""#, aid));
    }
    attrs.push_str("/>");
    attrs
}

fn codec_to_mime(codec: &str) -> (&str, &str) {
    match codec.to_uppercase().as_str() {
        "FLAC" => ("flac", "audio/flac"),
        "MP3" => ("mp3", "audio/mpeg"),
        "AAC" | "M4A" => ("m4a", "audio/mp4"),
        "OPUS" => ("opus", "audio/opus"),
        "VORBIS" | "OGG" => ("ogg", "audio/ogg"),
        "WAV" => ("wav", "audio/wav"),
        "AIFF" => ("aiff", "audio/aiff"),
        "WAVPACK" | "WV" => ("wv", "audio/x-wavpack"),
        "APE" => ("ape", "audio/x-ape"),
        _ => ("bin", "application/octet-stream"),
    }
}

fn extension_to_mime(ext: &str) -> &str {
    match ext.to_lowercase().as_str() {
        "flac" => "audio/flac",
        "mp3" => "audio/mpeg",
        "m4a" | "aac" | "mp4" => "audio/mp4",
        "opus" => "audio/opus",
        "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        "aiff" | "aif" => "audio/aiff",
        "wv" => "audio/x-wavpack",
        "ape" => "audio/x-ape",
        _ => "application/octet-stream",
    }
}

// ---------------------------------------------------------------------------
// GET /rest/stream — serve audio file with HTTP Range support
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct StreamParams {
    #[serde(flatten)]
    auth: AuthParams,
    id: Option<i64>,
}

async fn stream(
    State(state): State<AppState>,
    Query(params): Query<StreamParams>,
    headers: HeaderMap,
) -> Result<Response, SubsonicError> {
    verify_auth(&params.auth, &state.config)?;

    let track_id = params
        .id
        .ok_or_else(|| SubsonicError::MissingParam("missing parameter: id".into()))?;

    let db = state.open_db()?;
    let track = queries::get_track_row(&db.conn, track_id)
        .map_err(|e| SubsonicError::Internal(e.to_string()))?
        .ok_or_else(|| SubsonicError::NotFound(format!("track {} not found", track_id)))?;

    // Resolve file path: prefer local path, then cached_path.
    let file_path = track
        .path
        .as_deref()
        .or(track.cached_path.as_deref())
        .ok_or_else(|| SubsonicError::NotFound("track has no local file".into()))?;

    let path = std::path::PathBuf::from(file_path);
    let metadata = tokio::fs::metadata(&path).await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            SubsonicError::NotFound("file not found on disk".into())
        } else {
            SubsonicError::Internal(e.to_string())
        }
    })?;
    let total_size = metadata.len();

    // Determine content type from extension.
    let content_type = path
        .extension()
        .and_then(|e| e.to_str())
        .map(extension_to_mime)
        .unwrap_or("application/octet-stream");

    // Parse Range header.
    if let Some(range_header) = headers.get(header::RANGE) {
        let range_str = range_header
            .to_str()
            .map_err(|_| SubsonicError::Internal("invalid range header".into()))?;

        if let Some((start, end)) = parse_range(range_str, total_size) {
            let length = end - start + 1;

            let mut file = tokio::fs::File::open(&path)
                .await
                .map_err(|e| SubsonicError::Internal(e.to_string()))?;
            tokio::io::AsyncSeekExt::seek(&mut file, std::io::SeekFrom::Start(start))
                .await
                .map_err(|e| SubsonicError::Internal(e.to_string()))?;

            let stream = tokio_util::io::ReaderStream::new(file.take(length));
            let body = axum::body::Body::from_stream(stream);

            let resp = Response::builder()
                .status(StatusCode::PARTIAL_CONTENT)
                .header(header::CONTENT_TYPE, content_type)
                .header(header::CONTENT_LENGTH, length)
                .header(
                    header::CONTENT_RANGE,
                    format!("bytes {}-{}/{}", start, end, total_size),
                )
                .header(header::ACCEPT_RANGES, "bytes")
                .body(body)
                .map_err(|e| SubsonicError::Internal(e.to_string()))?;

            return Ok(resp);
        }
    }

    // No range — serve full file.
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|e| SubsonicError::Internal(e.to_string()))?;
    let stream = tokio_util::io::ReaderStream::new(file);
    let body = axum::body::Body::from_stream(stream);

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, total_size)
        .header(header::ACCEPT_RANGES, "bytes")
        .body(body)
        .map_err(|e| SubsonicError::Internal(e.to_string()))
}

/// Parse `Range: bytes=START-END` header. Returns (start, end) inclusive.
fn parse_range(range: &str, total: u64) -> Option<(u64, u64)> {
    let bytes_prefix = range.strip_prefix("bytes=")?;
    let mut parts = bytes_prefix.splitn(2, '-');
    let start_str = parts.next()?.trim();
    let end_str = parts.next()?.trim();

    if start_str.is_empty() {
        // Suffix range: bytes=-500 means last 500 bytes.
        let suffix: u64 = end_str.parse().ok()?;
        let start = total.saturating_sub(suffix);
        Some((start, total - 1))
    } else {
        let start: u64 = start_str.parse().ok()?;
        let end = if end_str.is_empty() {
            total - 1
        } else {
            end_str.parse::<u64>().ok()?.min(total - 1)
        };
        if start > end || start >= total {
            return None;
        }
        Some((start, end))
    }
}

// ---------------------------------------------------------------------------
// GET /rest/getCoverArt
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CoverArtParams {
    #[serde(flatten)]
    auth: AuthParams,
    id: Option<i64>,
    size: Option<u32>,
}

async fn get_cover_art(
    State(state): State<AppState>,
    Query(params): Query<CoverArtParams>,
) -> Result<Response, SubsonicError> {
    verify_auth(&params.auth, &state.config)?;

    let track_id = params
        .id
        .ok_or_else(|| SubsonicError::MissingParam("missing parameter: id".into()))?;

    let db = state.open_db()?;
    let track = queries::get_track_row(&db.conn, track_id)
        .map_err(|e| SubsonicError::Internal(e.to_string()))?
        .ok_or_else(|| SubsonicError::NotFound(format!("track {} not found", track_id)))?;

    let file_path = track
        .path
        .as_deref()
        .or(track.cached_path.as_deref())
        .ok_or_else(|| SubsonicError::NotFound("track has no local file".into()))?;

    let path = std::path::PathBuf::from(file_path);
    let art_bytes = extract_cover_art(&path)
        .ok_or_else(|| SubsonicError::NotFound("no cover art embedded".into()))?;

    // Detect image format from magic bytes.
    let (content_type, is_png) = if art_bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        ("image/png", true)
    } else {
        ("image/jpeg", false)
    };

    // Optionally resize.
    let final_bytes = if let Some(size) = params.size {
        resize_image(&art_bytes, size, is_png)?
    } else {
        art_bytes
    };

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "max-age=86400"),
        ],
        final_bytes,
    )
        .into_response())
}

fn resize_image(data: &[u8], size: u32, output_png: bool) -> Result<Vec<u8>, SubsonicError> {
    let img = image::load_from_memory(data)
        .map_err(|e| SubsonicError::Internal(format!("image decode error: {}", e)))?;

    let resized = img.resize(size, size, image::imageops::FilterType::Lanczos3);
    let format = if output_png {
        image::ImageFormat::Png
    } else {
        image::ImageFormat::Jpeg
    };

    let mut buf = Cursor::new(Vec::new());
    resized
        .write_to(&mut buf, format)
        .map_err(|e| SubsonicError::Internal(format!("image encode error: {}", e)))?;
    Ok(buf.into_inner())
}

// ---------------------------------------------------------------------------
// Server startup
// ---------------------------------------------------------------------------

pub fn cmd_serve(port: Option<u16>) {
    let _db = open_db(); // ensure DB exists / migrations run
    let db_path = koan_core::config::db_path();
    let cfg = Config::load().unwrap_or_default();
    let port = port.unwrap_or(4040);

    let state = AppState {
        db_path,
        config: Arc::new(cfg),
    };

    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    rt.block_on(async {
        let app = axum::Router::new()
            .route("/rest/ping", get(ping))
            .route("/rest/ping.view", get(ping))
            .route("/rest/search3", get(search3))
            .route("/rest/search3.view", get(search3))
            .route("/rest/stream", get(stream))
            .route("/rest/stream.view", get(stream))
            .route("/rest/getCoverArt", get(get_cover_art))
            .route("/rest/getCoverArt.view", get(get_cover_art))
            .with_state(state);

        let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
        eprintln!("koan subsonic server listening on http://0.0.0.0:{}", port);

        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .expect("failed to bind");
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown_signal())
            .await
            .expect("server error");
    });
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to listen for ctrl+c");
    eprintln!("\nshutting down...");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_range_full() {
        assert_eq!(parse_range("bytes=0-999", 5000), Some((0, 999)));
    }

    #[test]
    fn test_parse_range_open_end() {
        assert_eq!(parse_range("bytes=1000-", 5000), Some((1000, 4999)));
    }

    #[test]
    fn test_parse_range_suffix() {
        assert_eq!(parse_range("bytes=-500", 5000), Some((4500, 4999)));
    }

    #[test]
    fn test_parse_range_out_of_bounds() {
        assert_eq!(parse_range("bytes=5000-6000", 5000), None);
    }

    #[test]
    fn test_parse_range_clamps_end() {
        assert_eq!(parse_range("bytes=4000-9999", 5000), Some((4000, 4999)));
    }

    #[test]
    fn test_xml_escape() {
        assert_eq!(
            xml_escape(r#"Tom & Jerry <"special">"#),
            "Tom &amp; Jerry &lt;&quot;special&quot;&gt;"
        );
    }

    #[test]
    fn test_codec_to_mime() {
        assert_eq!(codec_to_mime("FLAC"), ("flac", "audio/flac"));
        assert_eq!(codec_to_mime("MP3"), ("mp3", "audio/mpeg"));
        assert_eq!(codec_to_mime("AAC"), ("m4a", "audio/mp4"));
        assert_eq!(codec_to_mime("Opus"), ("opus", "audio/opus"));
    }

    #[test]
    fn test_extension_to_mime() {
        assert_eq!(extension_to_mime("flac"), "audio/flac");
        assert_eq!(extension_to_mime("mp3"), "audio/mpeg");
        assert_eq!(extension_to_mime("m4a"), "audio/mp4");
        assert_eq!(extension_to_mime("FLAC"), "audio/flac");
    }

    #[test]
    fn test_hex_decode() {
        assert_eq!(hex_decode("68656c6c6f"), "hello");
        assert_eq!(hex_decode(""), "");
    }

    #[test]
    fn test_subsonic_ok_format() {
        let xml = subsonic_ok("  <test/>");
        assert!(xml.contains(r#"status="ok""#));
        assert!(xml.contains("<test/>"));
        assert!(xml.contains(API_VERSION));
    }

    #[test]
    fn test_track_to_xml_basic() {
        let track = queries::TrackRow {
            id: 42,
            album_id: Some(5),
            artist_id: Some(3),
            artist_name: "Test Artist".into(),
            album_artist_name: "Test Artist".into(),
            album_title: "Test Album".into(),
            disc: Some(1),
            track_number: Some(7),
            title: "Test Song".into(),
            duration_ms: Some(240_000),
            path: Some("/music/test.flac".into()),
            codec: Some("FLAC".into()),
            sample_rate: Some(44100),
            bit_depth: Some(16),
            channels: Some(2),
            bitrate: Some(1000),
            genre: Some("Electronic".into()),
            source: "local".into(),
            remote_id: None,
            cached_path: None,
        };
        let xml = track_to_xml(&track);
        assert!(xml.contains(r#"id="42""#));
        assert!(xml.contains(r#"title="Test Song""#));
        assert!(xml.contains(r#"artist="Test Artist""#));
        assert!(xml.contains(r#"album="Test Album""#));
        assert!(xml.contains(r#"track="7""#));
        assert!(xml.contains(r#"duration="240""#));
        assert!(xml.contains(r#"suffix="flac""#));
        assert!(xml.contains(r#"contentType="audio/flac""#));
        assert!(xml.contains(r#"genre="Electronic""#));
    }

    #[test]
    fn test_track_to_xml_special_chars() {
        let track = queries::TrackRow {
            id: 1,
            album_id: None,
            artist_id: None,
            artist_name: "Tom & Jerry".into(),
            album_artist_name: "Tom & Jerry".into(),
            album_title: "\"Best\" <Hits>".into(),
            disc: None,
            track_number: None,
            title: "Rock & Roll".into(),
            duration_ms: None,
            path: None,
            codec: None,
            sample_rate: None,
            bit_depth: None,
            channels: None,
            bitrate: None,
            genre: None,
            source: "local".into(),
            remote_id: None,
            cached_path: None,
        };
        let xml = track_to_xml(&track);
        assert!(xml.contains("Tom &amp; Jerry"));
        assert!(xml.contains("&quot;Best&quot; &lt;Hits&gt;"));
        assert!(xml.contains("Rock &amp; Roll"));
    }
}
