//! Subsonic-compatible REST API layer.
//!
//! Implements a subset of the Subsonic/OpenSubsonic REST API backed by the
//! local koan database.  Supports both XML (default) and JSON (`f=json`)
//! responses.  Auth uses MD5+salt tokens *and* legacy plaintext passwords.

use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::PathBuf;
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

const SUBSONIC_API_VERSION: &str = "1.16.1";
const SUBSONIC_XMLNS: &str = "http://subsonic.org/restapi";

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct AppState {
    db_path: PathBuf,
    username: String,
    password: String,
}

impl AppState {
    fn open_db(&self) -> Result<Database, SubsonicError> {
        Database::open(&self.db_path).map_err(|e| SubsonicError::from(e.to_string()))
    }
}

// ---------------------------------------------------------------------------
// Subsonic errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
enum SubsonicErrorCode {
    Generic = 0,
    MissingParameter = 10,
    WrongAuth = 40,
    NotFound = 70,
}

#[derive(Debug)]
struct SubsonicError {
    code: SubsonicErrorCode,
    message: String,
}

impl SubsonicError {
    fn wrong_auth() -> Self {
        Self {
            code: SubsonicErrorCode::WrongAuth,
            message: "Wrong username or password".into(),
        }
    }

    fn missing_param(name: &str) -> Self {
        Self {
            code: SubsonicErrorCode::MissingParameter,
            message: format!("Required parameter '{}' is missing", name),
        }
    }

    fn not_found(what: &str) -> Self {
        Self {
            code: SubsonicErrorCode::NotFound,
            message: format!("{} not found", what),
        }
    }

    fn internal(msg: impl Into<String>) -> Self {
        Self {
            code: SubsonicErrorCode::Generic,
            message: msg.into(),
        }
    }
}

impl From<String> for SubsonicError {
    fn from(s: String) -> Self {
        Self {
            code: SubsonicErrorCode::Generic,
            message: s,
        }
    }
}

impl IntoResponse for SubsonicError {
    fn into_response(self) -> Response {
        // Default to XML for error responses produced via `?` in handlers.
        SubsonicResponse::error(false, &self)
    }
}

// ---------------------------------------------------------------------------
// Query params (common to all endpoints)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct SubsonicParams {
    u: Option<String>,
    t: Option<String>,
    s: Option<String>,
    p: Option<String>,
    #[allow(dead_code)]
    v: Option<String>,
    #[allow(dead_code)]
    c: Option<String>,
    f: Option<String>,
}

impl SubsonicParams {
    fn wants_json(&self) -> bool {
        self.f.as_deref() == Some("json")
    }
}

// ---------------------------------------------------------------------------
// Response builder (XML + JSON)
// ---------------------------------------------------------------------------

struct SubsonicResponse;

impl SubsonicResponse {
    fn ok(json: bool) -> XmlBuilder {
        XmlBuilder {
            json,
            children: Vec::new(),
        }
    }

    fn error(json: bool, err: &SubsonicError) -> Response {
        if json {
            let body = serde_json::json!({
                "subsonic-response": {
                    "status": "failed",
                    "version": SUBSONIC_API_VERSION,
                    "error": {
                        "code": err.code as i32,
                        "message": err.message,
                    }
                }
            });
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
                serde_json::to_string(&body).unwrap(),
            )
                .into_response()
        } else {
            let xml = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<subsonic-response xmlns="{}" status="failed" version="{}">
  <error code="{}" message="{}"/>
</subsonic-response>"#,
                SUBSONIC_XMLNS,
                SUBSONIC_API_VERSION,
                err.code as i32,
                xml_escape(&err.message),
            );
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "application/xml; charset=utf-8")],
                xml,
            )
                .into_response()
        }
    }
}

// ---------------------------------------------------------------------------
// Lightweight XML/JSON builder
// ---------------------------------------------------------------------------

struct XmlBuilder {
    json: bool,
    children: Vec<XmlNode>,
}

#[derive(Clone)]
struct XmlNode {
    tag: String,
    attrs: Vec<(String, String)>,
    children: Vec<XmlNode>,
    is_array: bool,
    array_child_tag: Option<String>,
}

impl XmlNode {
    fn new(tag: &str) -> Self {
        Self {
            tag: tag.into(),
            attrs: Vec::new(),
            children: Vec::new(),
            is_array: false,
            array_child_tag: None,
        }
    }

    fn attr(mut self, key: &str, value: &str) -> Self {
        self.attrs.push((key.into(), value.into()));
        self
    }

    fn attr_opt(self, key: &str, value: Option<&str>) -> Self {
        match value {
            Some(v) => self.attr(key, v),
            None => self,
        }
    }

    fn attr_opt_i32(self, key: &str, value: Option<i32>) -> Self {
        match value {
            Some(v) => self.attr(key, &v.to_string()),
            None => self,
        }
    }

    fn attr_opt_i64(self, key: &str, value: Option<i64>) -> Self {
        match value {
            Some(v) => self.attr(key, &v.to_string()),
            None => self,
        }
    }

    fn child(mut self, node: XmlNode) -> Self {
        self.children.push(node);
        self
    }

    fn array_of(mut self, child_tag: &str) -> Self {
        self.is_array = true;
        self.array_child_tag = Some(child_tag.into());
        self
    }

    fn to_xml(&self, indent: usize) -> String {
        let pad = "  ".repeat(indent);
        let mut s = format!("<{}", self.tag);
        for (k, v) in &self.attrs {
            s.push_str(&format!(" {}=\"{}\"", k, xml_escape(v)));
        }
        if self.children.is_empty() {
            s.push_str("/>");
            return format!("{}{}", pad, s);
        }
        s.push('>');
        let mut out = format!("{}{}\n", pad, s);
        for child in &self.children {
            out.push_str(&child.to_xml(indent + 1));
            out.push('\n');
        }
        out.push_str(&format!("{}</{}>", pad, self.tag));
        out
    }

    fn to_json_value(&self) -> serde_json::Value {
        let mut obj = serde_json::Map::new();
        for (k, v) in &self.attrs {
            obj.insert(k.clone(), serde_json::Value::String(v.clone()));
        }
        if self.is_array {
            let child_tag = self.array_child_tag.as_deref().unwrap_or("item");
            let arr: Vec<serde_json::Value> =
                self.children.iter().map(|c| c.to_json_value()).collect();
            obj.insert(child_tag.into(), serde_json::Value::Array(arr));
        } else {
            let mut groups: BTreeMap<String, Vec<serde_json::Value>> = BTreeMap::new();
            for child in &self.children {
                groups
                    .entry(child.tag.clone())
                    .or_default()
                    .push(child.to_json_value());
            }
            for (tag, values) in groups {
                if values.len() == 1 {
                    obj.insert(tag, values.into_iter().next().unwrap());
                } else {
                    obj.insert(tag, serde_json::Value::Array(values));
                }
            }
        }
        serde_json::Value::Object(obj)
    }
}

impl XmlBuilder {
    fn child(mut self, node: XmlNode) -> Self {
        self.children.push(node);
        self
    }

    fn build(self) -> Response {
        if self.json {
            let mut inner = serde_json::Map::new();
            inner.insert("status".into(), serde_json::Value::String("ok".into()));
            inner.insert(
                "version".into(),
                serde_json::Value::String(SUBSONIC_API_VERSION.into()),
            );
            for child in &self.children {
                inner.insert(child.tag.clone(), child.to_json_value());
            }
            let wrapper = serde_json::json!({ "subsonic-response": inner });
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
                serde_json::to_string(&wrapper).unwrap(),
            )
                .into_response()
        } else {
            let mut xml = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<subsonic-response xmlns=\"{}\" status=\"ok\" version=\"{}\">\n",
                SUBSONIC_XMLNS, SUBSONIC_API_VERSION,
            );
            for child in &self.children {
                xml.push_str(&child.to_xml(1));
                xml.push('\n');
            }
            xml.push_str("</subsonic-response>");
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "application/xml; charset=utf-8")],
                xml,
            )
                .into_response()
        }
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\'', "&apos;")
}

// ---------------------------------------------------------------------------
// Auth — MD5+salt token *and* legacy plaintext password
// ---------------------------------------------------------------------------

fn validate_auth(params: &SubsonicParams, state: &AppState) -> Result<(), SubsonicError> {
    let username = params
        .u
        .as_deref()
        .ok_or_else(|| SubsonicError::missing_param("u"))?;

    if username != state.username {
        return Err(SubsonicError::wrong_auth());
    }

    // Token-based: t = md5(password + s)
    if let (Some(token), Some(salt)) = (params.t.as_deref(), params.s.as_deref()) {
        let expected = format!("{:x}", md5::compute(format!("{}{}", state.password, salt)));
        if token == expected {
            return Ok(());
        }
        return Err(SubsonicError::wrong_auth());
    }

    // Legacy plaintext (with optional enc: hex prefix)
    if let Some(ref p) = params.p {
        let plain = if let Some(hex) = p.strip_prefix("enc:") {
            hex_decode(hex)
        } else {
            p.clone()
        };
        if plain == state.password {
            return Ok(());
        }
        return Err(SubsonicError::wrong_auth());
    }

    Err(SubsonicError::missing_param("t and s, or p"))
}

fn hex_decode(hex: &str) -> String {
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

// ---------------------------------------------------------------------------
// Helpers: track/album → XmlNode
// ---------------------------------------------------------------------------

fn track_to_xml_node(track: &queries::TrackRow) -> XmlNode {
    let duration_secs = track.duration_ms.map(|ms| ms / 1000);
    let (suffix, content_type) = track
        .codec
        .as_deref()
        .map(codec_to_mime)
        .unwrap_or(("bin", "application/octet-stream"));
    XmlNode::new("song")
        .attr("id", &track.id.to_string())
        .attr("title", &track.title)
        .attr("album", &track.album_title)
        .attr("artist", &track.artist_name)
        .attr_opt_i32("track", track.track_number)
        .attr_opt_i32("discNumber", track.disc)
        .attr_opt_i64("duration", duration_secs)
        .attr_opt_i32("bitRate", track.bitrate)
        .attr_opt("suffix", Some(suffix))
        .attr_opt("contentType", Some(content_type))
        .attr_opt("genre", track.genre.as_deref())
        .attr_opt(
            "albumId",
            track.album_id.map(|id| id.to_string()).as_deref(),
        )
        .attr_opt(
            "artistId",
            track.artist_id.map(|id| id.to_string()).as_deref(),
        )
}

fn year_from_date(date: Option<&str>) -> Option<String> {
    date.and_then(|d| {
        if d.len() >= 4 {
            Some(d[..4].to_string())
        } else {
            None
        }
    })
}

fn album_to_xml_node(album: &queries::AlbumRow, track_count: Option<i32>) -> XmlNode {
    let year_str = year_from_date(album.date.as_deref());
    let count = track_count.unwrap_or(0);
    XmlNode::new("album")
        .attr("id", &album.id.to_string())
        .attr("name", &album.title)
        .attr("artist", &album.artist_name)
        .attr("artistId", &album.artist_id.to_string())
        .attr("songCount", &count.to_string())
        .attr_opt("year", year_str.as_deref())
}

fn album_counts_by_artist(db: &Database) -> BTreeMap<i64, i64> {
    let albums = queries::all_albums(&db.conn).unwrap_or_default();
    let mut map: BTreeMap<i64, i64> = BTreeMap::new();
    for album in albums {
        *map.entry(album.artist_id).or_insert(0) += 1;
    }
    map
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

/// Resolve a track's file path (local preferred, then cached).
fn track_file_path(track: &queries::TrackRow) -> Option<&str> {
    track.path.as_deref().or(track.cached_path.as_deref())
}

// ---------------------------------------------------------------------------
// Endpoint param structs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct IdParam {
    id: Option<String>,
    #[serde(flatten)]
    auth: SubsonicParams,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AlbumListParams {
    #[serde(rename = "type")]
    list_type: Option<String>,
    size: Option<i64>,
    offset: Option<i64>,
    #[serde(flatten)]
    auth: SubsonicParams,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Search3Params {
    #[serde(flatten)]
    auth: SubsonicParams,
    query: Option<String>,
    artist_count: Option<u32>,
    album_count: Option<u32>,
    song_count: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct StreamParams {
    #[serde(flatten)]
    auth: SubsonicParams,
    id: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct CoverArtParams {
    #[serde(flatten)]
    auth: SubsonicParams,
    id: Option<i64>,
    size: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StarParams {
    #[serde(flatten)]
    auth: SubsonicParams,
    id: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScrobbleParams {
    #[serde(flatten)]
    auth: SubsonicParams,
    id: Option<i64>,
    time: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RandomSongsParams {
    #[serde(flatten)]
    auth: SubsonicParams,
    size: Option<u32>,
    genre: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SimilarSongs2Params {
    #[serde(flatten)]
    auth: SubsonicParams,
    id: Option<i64>,
    count: Option<usize>,
}

// ===========================================================================
// Endpoints — browsing
// ===========================================================================

async fn ping(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    if let Err(e) = validate_auth(&params, &state) {
        return SubsonicResponse::error(params.wants_json(), &e);
    }
    SubsonicResponse::ok(params.wants_json()).build()
}

async fn get_license(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    if let Err(e) = validate_auth(&params, &state) {
        return SubsonicResponse::error(params.wants_json(), &e);
    }
    SubsonicResponse::ok(params.wants_json())
        .child(
            XmlNode::new("license")
                .attr("valid", "true")
                .attr("email", "koan@localhost"),
        )
        .build()
}

async fn get_artists(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    if let Err(e) = validate_auth(&params, &state) {
        return SubsonicResponse::error(params.wants_json(), &e);
    }
    let json = params.wants_json();

    let db = match state.open_db() {
        Ok(db) => db,
        Err(e) => return SubsonicResponse::error(json, &e),
    };

    let artists = match queries::all_artists(&db.conn) {
        Ok(a) => a,
        Err(e) => return SubsonicResponse::error(json, &SubsonicError::from(e.to_string())),
    };

    // Group by first letter.
    let mut index_map: BTreeMap<String, Vec<&queries::ArtistRow>> = BTreeMap::new();
    for artist in &artists {
        let letter = artist
            .sort_name
            .as_deref()
            .unwrap_or(&artist.name)
            .chars()
            .next()
            .map(|c| {
                let upper = c.to_uppercase().to_string();
                if upper
                    .chars()
                    .next()
                    .is_some_and(|ch| ch.is_ascii_alphabetic())
                {
                    upper
                } else {
                    "#".to_string()
                }
            })
            .unwrap_or_else(|| "#".to_string());
        index_map.entry(letter).or_default().push(artist);
    }

    let album_counts = album_counts_by_artist(&db);

    let mut artists_node = XmlNode::new("artists").array_of("index");
    for (letter, group) in &index_map {
        let mut index_node = XmlNode::new("index")
            .attr("name", letter)
            .array_of("artist");
        for artist in group {
            let count = album_counts.get(&artist.id).copied().unwrap_or(0);
            index_node = index_node.child(
                XmlNode::new("artist")
                    .attr("id", &artist.id.to_string())
                    .attr("name", &artist.name)
                    .attr("albumCount", &count.to_string()),
            );
        }
        artists_node = artists_node.child(index_node);
    }

    SubsonicResponse::ok(json).child(artists_node).build()
}

async fn get_artist(State(state): State<Arc<AppState>>, Query(params): Query<IdParam>) -> Response {
    if let Err(e) = validate_auth(&params.auth, &state) {
        return SubsonicResponse::error(params.auth.wants_json(), &e);
    }
    let json = params.auth.wants_json();

    let artist_id: i64 = match params.id.as_deref().and_then(|s| s.parse().ok()) {
        Some(id) => id,
        None => return SubsonicResponse::error(json, &SubsonicError::missing_param("id")),
    };

    let db = match state.open_db() {
        Ok(db) => db,
        Err(e) => return SubsonicResponse::error(json, &e),
    };

    let all = match queries::all_artists(&db.conn) {
        Ok(a) => a,
        Err(e) => return SubsonicResponse::error(json, &SubsonicError::from(e.to_string())),
    };
    let artist = match all.into_iter().find(|a| a.id == artist_id) {
        Some(a) => a,
        None => return SubsonicResponse::error(json, &SubsonicError::not_found("Artist")),
    };

    let albums = match queries::albums_for_artist(&db.conn, artist_id) {
        Ok(a) => a,
        Err(e) => return SubsonicResponse::error(json, &SubsonicError::from(e.to_string())),
    };

    let mut artist_node = XmlNode::new("artist")
        .attr("id", &artist.id.to_string())
        .attr("name", &artist.name)
        .attr("albumCount", &albums.len().to_string())
        .array_of("album");

    for album in &albums {
        artist_node = artist_node.child(album_to_xml_node(album, album.total_tracks));
    }

    SubsonicResponse::ok(json).child(artist_node).build()
}

async fn get_album(State(state): State<Arc<AppState>>, Query(params): Query<IdParam>) -> Response {
    if let Err(e) = validate_auth(&params.auth, &state) {
        return SubsonicResponse::error(params.auth.wants_json(), &e);
    }
    let json = params.auth.wants_json();

    let album_id: i64 = match params.id.as_deref().and_then(|s| s.parse().ok()) {
        Some(id) => id,
        None => return SubsonicResponse::error(json, &SubsonicError::missing_param("id")),
    };

    let db = match state.open_db() {
        Ok(db) => db,
        Err(e) => return SubsonicResponse::error(json, &e),
    };

    let album = match queries::get_album(&db.conn, album_id) {
        Ok(Some(a)) => a,
        _ => return SubsonicResponse::error(json, &SubsonicError::not_found("Album")),
    };

    let tracks = queries::tracks_for_album(&db.conn, album_id).unwrap_or_default();
    let mut album_node = album_to_xml_node(&album, Some(tracks.len() as i32)).array_of("song");

    for track in &tracks {
        album_node = album_node.child(track_to_xml_node(track));
    }

    SubsonicResponse::ok(json).child(album_node).build()
}

async fn get_album_list2(
    State(state): State<Arc<AppState>>,
    Query(params): Query<AlbumListParams>,
) -> Response {
    if let Err(e) = validate_auth(&params.auth, &state) {
        return SubsonicResponse::error(params.auth.wants_json(), &e);
    }
    let json = params.auth.wants_json();

    let list_type = params.list_type.as_deref().unwrap_or("alphabeticalByName");
    let size = params.size.unwrap_or(20).min(500) as usize;
    let offset = params.offset.unwrap_or(0).max(0) as usize;

    let db = match state.open_db() {
        Ok(db) => db,
        Err(e) => return SubsonicResponse::error(json, &e),
    };

    let mut albums = match queries::all_albums(&db.conn) {
        Ok(a) => a,
        Err(e) => return SubsonicResponse::error(json, &SubsonicError::from(e.to_string())),
    };

    match list_type {
        "alphabeticalByName" => albums.sort_by(|a, b| a.title.cmp(&b.title)),
        "alphabeticalByArtist" => albums.sort_by(|a, b| {
            a.artist_name
                .cmp(&b.artist_name)
                .then(a.title.cmp(&b.title))
        }),
        "newest" => albums.sort_by(|a, b| b.date.cmp(&a.date)),
        "random" => {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let seed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            albums.sort_by(|a, b| {
                let mut ha = DefaultHasher::new();
                (a.id, seed).hash(&mut ha);
                let mut hb = DefaultHasher::new();
                (b.id, seed).hash(&mut hb);
                ha.finish().cmp(&hb.finish())
            });
        }
        _ => {}
    }

    let page: Vec<_> = albums.into_iter().skip(offset).take(size).collect();

    let mut list_node = XmlNode::new("albumList2").array_of("album");
    for album in &page {
        list_node = list_node.child(album_to_xml_node(album, album.total_tracks));
    }

    SubsonicResponse::ok(json).child(list_node).build()
}

async fn get_song(State(state): State<Arc<AppState>>, Query(params): Query<IdParam>) -> Response {
    if let Err(e) = validate_auth(&params.auth, &state) {
        return SubsonicResponse::error(params.auth.wants_json(), &e);
    }
    let json = params.auth.wants_json();

    let track_id: i64 = match params.id.as_deref().and_then(|s| s.parse().ok()) {
        Some(id) => id,
        None => return SubsonicResponse::error(json, &SubsonicError::missing_param("id")),
    };

    let db = match state.open_db() {
        Ok(db) => db,
        Err(e) => return SubsonicResponse::error(json, &e),
    };

    let track = match queries::get_track_row(&db.conn, track_id) {
        Ok(Some(t)) => t,
        _ => return SubsonicResponse::error(json, &SubsonicError::not_found("Song")),
    };

    SubsonicResponse::ok(json)
        .child(track_to_xml_node(&track))
        .build()
}

// ===========================================================================
// Endpoints — search
// ===========================================================================

async fn search3(
    State(state): State<Arc<AppState>>,
    Query(params): Query<Search3Params>,
) -> Response {
    if let Err(e) = validate_auth(&params.auth, &state) {
        return SubsonicResponse::error(params.auth.wants_json(), &e);
    }
    let json = params.auth.wants_json();

    let query = match params.query.as_deref() {
        Some(q) => q,
        None => return SubsonicResponse::error(json, &SubsonicError::missing_param("query")),
    };

    let artist_count = params.artist_count.unwrap_or(20);
    let album_count = params.album_count.unwrap_or(20);
    let song_count = params.song_count.unwrap_or(20);

    let db = match state.open_db() {
        Ok(db) => db,
        Err(e) => return SubsonicResponse::error(json, &e),
    };

    let total_needed = (artist_count + album_count + song_count).max(100);
    let tracks = match queries::search_tracks_paged(&db.conn, query, total_needed, 0) {
        Ok(t) => t,
        Err(e) => return SubsonicResponse::error(json, &SubsonicError::from(e.to_string())),
    };

    let mut result_node = XmlNode::new("searchResult3");

    // Unique artists.
    let mut seen_artists = std::collections::HashSet::new();
    let mut artist_n = 0u32;
    for t in &tracks {
        if artist_n >= artist_count {
            break;
        }
        if let Some(aid) = t.artist_id
            && seen_artists.insert(aid)
        {
            result_node = result_node.child(
                XmlNode::new("artist")
                    .attr("id", &aid.to_string())
                    .attr("name", &t.artist_name),
            );
            artist_n += 1;
        }
    }

    // Unique albums.
    let mut seen_albums = std::collections::HashSet::new();
    let mut album_n = 0u32;
    for t in &tracks {
        if album_n >= album_count {
            break;
        }
        if let Some(alid) = t.album_id
            && seen_albums.insert(alid)
        {
            result_node = result_node.child(
                XmlNode::new("album")
                    .attr("id", &alid.to_string())
                    .attr("name", &t.album_title)
                    .attr("artist", &t.album_artist_name),
            );
            album_n += 1;
        }
    }

    // Songs.
    for t in tracks.iter().take(song_count as usize) {
        result_node = result_node.child(track_to_xml_node(t));
    }

    SubsonicResponse::ok(json).child(result_node).build()
}

// ===========================================================================
// Endpoints — streaming
// ===========================================================================

async fn stream(
    State(state): State<Arc<AppState>>,
    Query(params): Query<StreamParams>,
    headers: HeaderMap,
) -> Result<Response, SubsonicError> {
    validate_auth(&params.auth, &state)?;

    let track_id = params
        .id
        .ok_or_else(|| SubsonicError::missing_param("id"))?;

    let db = state.open_db()?;
    let track = queries::get_track_row(&db.conn, track_id)
        .map_err(|e| SubsonicError::internal(e.to_string()))?
        .ok_or_else(|| SubsonicError::not_found("Track"))?;

    let file_path = track_file_path(&track)
        .ok_or_else(|| SubsonicError::not_found("Track has no local file"))?;

    let path = PathBuf::from(file_path);
    let metadata = tokio::fs::metadata(&path).await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            SubsonicError::not_found("File not found on disk")
        } else {
            SubsonicError::internal(e.to_string())
        }
    })?;
    let total_size = metadata.len();

    let content_type = path
        .extension()
        .and_then(|e| e.to_str())
        .map(extension_to_mime)
        .unwrap_or("application/octet-stream");

    // Parse Range header for seeking support.
    if let Some(range_header) = headers.get(header::RANGE) {
        let range_str = range_header
            .to_str()
            .map_err(|_| SubsonicError::internal("invalid range header"))?;

        if let Some((start, end)) = parse_range(range_str, total_size) {
            let length = end - start + 1;

            let mut file = tokio::fs::File::open(&path)
                .await
                .map_err(|e| SubsonicError::internal(e.to_string()))?;
            tokio::io::AsyncSeekExt::seek(&mut file, std::io::SeekFrom::Start(start))
                .await
                .map_err(|e| SubsonicError::internal(e.to_string()))?;

            let stream = tokio_util::io::ReaderStream::new(file.take(length));
            let body = axum::body::Body::from_stream(stream);

            return Response::builder()
                .status(StatusCode::PARTIAL_CONTENT)
                .header(header::CONTENT_TYPE, content_type)
                .header(header::CONTENT_LENGTH, length)
                .header(
                    header::CONTENT_RANGE,
                    format!("bytes {}-{}/{}", start, end, total_size),
                )
                .header(header::ACCEPT_RANGES, "bytes")
                .body(body)
                .map_err(|e| SubsonicError::internal(e.to_string()));
        }
    }

    // No range — serve full file.
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|e| SubsonicError::internal(e.to_string()))?;
    let stream = tokio_util::io::ReaderStream::new(file);
    let body = axum::body::Body::from_stream(stream);

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, total_size)
        .header(header::ACCEPT_RANGES, "bytes")
        .body(body)
        .map_err(|e| SubsonicError::internal(e.to_string()))
}

fn parse_range(range: &str, total: u64) -> Option<(u64, u64)> {
    let bytes_prefix = range.strip_prefix("bytes=")?;
    let mut parts = bytes_prefix.splitn(2, '-');
    let start_str = parts.next()?.trim();
    let end_str = parts.next()?.trim();

    if start_str.is_empty() {
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

// ===========================================================================
// Endpoints — cover art
// ===========================================================================

async fn get_cover_art(
    State(state): State<Arc<AppState>>,
    Query(params): Query<CoverArtParams>,
) -> Result<Response, SubsonicError> {
    validate_auth(&params.auth, &state)?;

    let track_id = params
        .id
        .ok_or_else(|| SubsonicError::missing_param("id"))?;

    let db = state.open_db()?;
    let track = queries::get_track_row(&db.conn, track_id)
        .map_err(|e| SubsonicError::internal(e.to_string()))?
        .ok_or_else(|| SubsonicError::not_found("Track"))?;

    let file_path = track_file_path(&track)
        .ok_or_else(|| SubsonicError::not_found("Track has no local file"))?;

    let path = PathBuf::from(file_path);
    let art_bytes = extract_cover_art(&path)
        .ok_or_else(|| SubsonicError::not_found("No cover art embedded"))?;

    let (content_type, is_png) = if art_bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        ("image/png", true)
    } else {
        ("image/jpeg", false)
    };

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
        .map_err(|e| SubsonicError::internal(format!("image decode error: {}", e)))?;
    let resized = img.resize(size, size, image::imageops::FilterType::Lanczos3);
    let format = if output_png {
        image::ImageFormat::Png
    } else {
        image::ImageFormat::Jpeg
    };
    let mut buf = Cursor::new(Vec::new());
    resized
        .write_to(&mut buf, format)
        .map_err(|e| SubsonicError::internal(format!("image encode error: {}", e)))?;
    Ok(buf.into_inner())
}

// ===========================================================================
// Endpoints — interaction (star, unstar, scrobble, etc.)
// ===========================================================================

async fn star(State(state): State<Arc<AppState>>, Query(params): Query<StarParams>) -> Response {
    if let Err(e) = validate_auth(&params.auth, &state) {
        return SubsonicResponse::error(params.auth.wants_json(), &e);
    }
    let json = params.auth.wants_json();
    toggle_star(&state, params.id, json, queries::add_favourite)
}

async fn unstar(State(state): State<Arc<AppState>>, Query(params): Query<StarParams>) -> Response {
    if let Err(e) = validate_auth(&params.auth, &state) {
        return SubsonicResponse::error(params.auth.wants_json(), &e);
    }
    let json = params.auth.wants_json();
    toggle_star(&state, params.id, json, queries::remove_favourite)
}

fn toggle_star(
    state: &AppState,
    id: Option<i64>,
    json: bool,
    op: fn(&rusqlite::Connection, &std::path::Path) -> rusqlite::Result<()>,
) -> Response {
    let track_id = match id {
        Some(id) => id,
        None => return SubsonicResponse::error(json, &SubsonicError::missing_param("id")),
    };

    let db = match state.open_db() {
        Ok(db) => db,
        Err(e) => return SubsonicResponse::error(json, &e),
    };

    let track = match queries::get_track_row(&db.conn, track_id) {
        Ok(Some(t)) => t,
        Ok(None) => return SubsonicResponse::error(json, &SubsonicError::not_found("Track")),
        Err(e) => return SubsonicResponse::error(json, &SubsonicError::from(e.to_string())),
    };

    let path_str = track_file_path(&track).unwrap_or("");
    if let Err(e) = op(&db.conn, std::path::Path::new(path_str)) {
        return SubsonicResponse::error(json, &SubsonicError::from(e.to_string()));
    }

    SubsonicResponse::ok(json).build()
}

async fn get_starred2(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    if let Err(e) = validate_auth(&params, &state) {
        return SubsonicResponse::error(params.wants_json(), &e);
    }
    let json = params.wants_json();

    let db = match state.open_db() {
        Ok(db) => db,
        Err(e) => return SubsonicResponse::error(json, &e),
    };

    let favourites = match queries::load_favourites(&db.conn) {
        Ok(f) => f,
        Err(e) => return SubsonicResponse::error(json, &SubsonicError::from(e.to_string())),
    };

    let mut starred_node = XmlNode::new("starred2").array_of("song");
    for fav_path in &favourites {
        let path_str = fav_path.to_string_lossy();
        if let Ok(Some(track_id)) = queries::track_id_by_path(&db.conn, &path_str)
            && let Ok(Some(track)) = queries::get_track_row(&db.conn, track_id)
        {
            starred_node = starred_node.child(track_to_xml_node(&track));
        }
    }

    SubsonicResponse::ok(json).child(starred_node).build()
}

async fn scrobble(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ScrobbleParams>,
) -> Response {
    if let Err(e) = validate_auth(&params.auth, &state) {
        return SubsonicResponse::error(params.auth.wants_json(), &e);
    }
    let json = params.auth.wants_json();

    let track_id = match params.id {
        Some(id) => id,
        None => return SubsonicResponse::error(json, &SubsonicError::missing_param("id")),
    };

    let db = match state.open_db() {
        Ok(db) => db,
        Err(e) => return SubsonicResponse::error(json, &e),
    };

    match queries::get_track_row(&db.conn, track_id) {
        Ok(Some(_)) => {}
        Ok(None) => return SubsonicResponse::error(json, &SubsonicError::not_found("Track")),
        Err(e) => return SubsonicResponse::error(json, &SubsonicError::from(e.to_string())),
    }

    let result = if let Some(time_ms) = params.time {
        db.conn
            .execute(
                "INSERT INTO play_history (track_id, played_at, duration_ms) VALUES (?1, ?2, NULL)",
                rusqlite::params![track_id, time_ms / 1000],
            )
            .map(|_| ())
            .map_err(|e| e.into())
    } else {
        queries::record_play(&db.conn, track_id, None)
    };

    if let Err(e) = result {
        return SubsonicResponse::error(
            json,
            &SubsonicError::from(format!("Database error: {}", e)),
        );
    }

    SubsonicResponse::ok(json).build()
}

async fn get_random_songs(
    State(state): State<Arc<AppState>>,
    Query(params): Query<RandomSongsParams>,
) -> Response {
    if let Err(e) = validate_auth(&params.auth, &state) {
        return SubsonicResponse::error(params.auth.wants_json(), &e);
    }
    let json = params.auth.wants_json();

    let size = params.size.unwrap_or(10);
    let genre = params.genre.as_deref();
    let fetch_count = if genre.is_some() { size * 5 } else { size };

    let db = match state.open_db() {
        Ok(db) => db,
        Err(e) => return SubsonicResponse::error(json, &e),
    };

    let tracks = match queries::random_tracks(&db.conn, fetch_count, None) {
        Ok(t) => t,
        Err(e) => return SubsonicResponse::error(json, &SubsonicError::from(e.to_string())),
    };

    let mut node = XmlNode::new("randomSongs").array_of("song");
    let mut count = 0u32;
    for t in &tracks {
        if count >= size {
            break;
        }
        if genre.is_some_and(|g| t.genre.as_deref() != Some(g)) {
            continue;
        }
        node = node.child(track_to_xml_node(t));
        count += 1;
    }

    SubsonicResponse::ok(json).child(node).build()
}

async fn get_similar_songs2(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SimilarSongs2Params>,
) -> Response {
    if let Err(e) = validate_auth(&params.auth, &state) {
        return SubsonicResponse::error(params.auth.wants_json(), &e);
    }
    let json = params.auth.wants_json();

    let track_id = match params.id {
        Some(id) => id,
        None => return SubsonicResponse::error(json, &SubsonicError::missing_param("id")),
    };
    let count = params.count.unwrap_or(50);

    let db = match state.open_db() {
        Ok(db) => db,
        Err(e) => return SubsonicResponse::error(json, &e),
    };

    let track = match queries::get_track_row(&db.conn, track_id) {
        Ok(Some(t)) => t,
        Ok(None) => return SubsonicResponse::error(json, &SubsonicError::not_found("Track")),
        Err(e) => return SubsonicResponse::error(json, &SubsonicError::from(e.to_string())),
    };

    let artist_id = match track.artist_id {
        Some(id) => id,
        None => {
            return SubsonicResponse::ok(json)
                .child(XmlNode::new("similarSongs2"))
                .build();
        }
    };

    let similar = match queries::get_similar_artists(&db.conn, artist_id) {
        Ok(s) => s,
        Err(e) => return SubsonicResponse::error(json, &SubsonicError::from(e.to_string())),
    };

    let mut node = XmlNode::new("similarSongs2").array_of("song");
    let mut total = 0usize;
    for (artist_row, _score) in &similar {
        if total >= count {
            break;
        }
        let artist_tracks = match queries::tracks_for_artist(&db.conn, artist_row.id) {
            Ok(t) => t,
            Err(_) => continue,
        };
        for t in &artist_tracks {
            if total >= count {
                break;
            }
            node = node.child(track_to_xml_node(t));
            total += 1;
        }
    }

    SubsonicResponse::ok(json).child(node).build()
}

// ===========================================================================
// Public router
// ===========================================================================

/// Build a Subsonic-compatible REST API router.
///
/// Auth credentials come from `Config::load()`.  If remote is not configured,
/// falls back to admin/admin.
pub fn subsonic_router(db_path: PathBuf) -> axum::Router {
    let cfg = Config::load().unwrap_or_default();

    let (username, password) = if cfg.remote.username.is_empty() {
        ("admin".to_string(), "admin".to_string())
    } else {
        let pass = super::get_remote_password(&cfg);
        (cfg.remote.username.clone(), pass)
    };

    let state = Arc::new(AppState {
        db_path,
        username,
        password,
    });

    axum::Router::new()
        // Browsing
        .route("/rest/ping", get(ping))
        .route("/rest/ping.view", get(ping))
        .route("/rest/getLicense", get(get_license))
        .route("/rest/getLicense.view", get(get_license))
        .route("/rest/getArtists", get(get_artists))
        .route("/rest/getArtists.view", get(get_artists))
        .route("/rest/getArtist", get(get_artist))
        .route("/rest/getArtist.view", get(get_artist))
        .route("/rest/getAlbum", get(get_album))
        .route("/rest/getAlbum.view", get(get_album))
        .route("/rest/getAlbumList2", get(get_album_list2))
        .route("/rest/getAlbumList2.view", get(get_album_list2))
        .route("/rest/getSong", get(get_song))
        .route("/rest/getSong.view", get(get_song))
        // Search
        .route("/rest/search3", get(search3))
        .route("/rest/search3.view", get(search3))
        // Streaming + media
        .route("/rest/stream", get(stream))
        .route("/rest/stream.view", get(stream))
        .route("/rest/getCoverArt", get(get_cover_art))
        .route("/rest/getCoverArt.view", get(get_cover_art))
        // Interaction
        .route("/rest/star", get(star))
        .route("/rest/star.view", get(star))
        .route("/rest/unstar", get(unstar))
        .route("/rest/unstar.view", get(unstar))
        .route("/rest/getStarred2", get(get_starred2))
        .route("/rest/getStarred2.view", get(get_starred2))
        .route("/rest/scrobble", get(scrobble))
        .route("/rest/scrobble.view", get(scrobble))
        .route("/rest/getRandomSongs", get(get_random_songs))
        .route("/rest/getRandomSongs.view", get(get_random_songs))
        .route("/rest/getSimilarSongs2", get(get_similar_songs2))
        .route("/rest/getSimilarSongs2.view", get(get_similar_songs2))
        .with_state(state)
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use koan_core::db::queries::TrackMeta;
    use tower::ServiceExt;

    fn test_state() -> (Arc<AppState>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let db = Database::open(&db_path).unwrap();
        koan_core::db::schema::create_tables(&db.conn).unwrap();

        let state = Arc::new(AppState {
            db_path,
            username: "testuser".into(),
            password: "testpass".into(),
        });
        (state, dir)
    }

    fn build_test_router(state: Arc<AppState>) -> axum::Router {
        // Re-use the same route set but with our test state.
        axum::Router::new()
            .route("/rest/ping", get(ping))
            .route("/rest/ping.view", get(ping))
            .route("/rest/getLicense", get(get_license))
            .route("/rest/getLicense.view", get(get_license))
            .route("/rest/getArtists", get(get_artists))
            .route("/rest/getArtists.view", get(get_artists))
            .route("/rest/getArtist", get(get_artist))
            .route("/rest/getArtist.view", get(get_artist))
            .route("/rest/getAlbum", get(get_album))
            .route("/rest/getAlbum.view", get(get_album))
            .route("/rest/getAlbumList2", get(get_album_list2))
            .route("/rest/getAlbumList2.view", get(get_album_list2))
            .route("/rest/getSong", get(get_song))
            .route("/rest/getSong.view", get(get_song))
            .route("/rest/search3", get(search3))
            .route("/rest/search3.view", get(search3))
            .route("/rest/star", get(star))
            .route("/rest/star.view", get(star))
            .route("/rest/unstar", get(unstar))
            .route("/rest/unstar.view", get(unstar))
            .route("/rest/getStarred2", get(get_starred2))
            .route("/rest/getStarred2.view", get(get_starred2))
            .route("/rest/scrobble", get(scrobble))
            .route("/rest/scrobble.view", get(scrobble))
            .route("/rest/getRandomSongs", get(get_random_songs))
            .route("/rest/getRandomSongs.view", get(get_random_songs))
            .route("/rest/getSimilarSongs2", get(get_similar_songs2))
            .route("/rest/getSimilarSongs2.view", get(get_similar_songs2))
            .with_state(state)
    }

    fn auth_query(extra: &str) -> String {
        let salt = "abc123";
        let token = format!("{:x}", md5::compute(format!("testpass{}", salt)));
        let base = format!("u=testuser&t={}&s={}&v=1.16.1&c=test", token, salt);
        if extra.is_empty() {
            base
        } else {
            format!("{}&{}", base, extra)
        }
    }

    fn seed_data(state: &AppState) {
        let db = Database::open(&state.db_path).unwrap();
        let meta = TrackMeta {
            title: "Test Song".into(),
            artist: "Test Artist".into(),
            album: "Test Album".into(),
            album_artist: Some("Test Artist".into()),
            track_number: Some(1),
            disc: Some(1),
            duration_ms: Some(240_000),
            codec: Some("FLAC".into()),
            sample_rate: Some(44100),
            bit_depth: Some(16),
            channels: Some(2),
            bitrate: Some(1411),
            genre: Some("Rock".into()),
            path: Some("/music/test.flac".into()),
            date: Some("2020".into()),
            label: None,
            size_bytes: None,
            mtime: None,
            source: "local".into(),
            remote_id: None,
            remote_url: None,
        };
        queries::upsert_track(&db.conn, &meta).unwrap();
    }

    async fn get_response(app: axum::Router, uri: &str) -> (StatusCode, String) {
        let resp = app
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, String::from_utf8(body.to_vec()).unwrap())
    }

    // --- Unit tests ---

    #[test]
    fn test_xml_escape() {
        assert_eq!(xml_escape("A&B"), "A&amp;B");
        assert_eq!(xml_escape("a<b>c"), "a&lt;b&gt;c");
        assert_eq!(xml_escape(r#"say "hi""#), "say &quot;hi&quot;");
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

    // --- Integration tests ---

    #[tokio::test]
    async fn test_ping_ok() {
        let (state, _dir) = test_state();
        let app = build_test_router(state);
        let (status, body) = get_response(app, &format!("/rest/ping?{}", auth_query(""))).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("status=\"ok\""));
    }

    #[tokio::test]
    async fn test_ping_json() {
        let (state, _dir) = test_state();
        let app = build_test_router(state);
        let (status, body) =
            get_response(app, &format!("/rest/ping?{}", auth_query("f=json"))).await;
        assert_eq!(status, StatusCode::OK);
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["subsonic-response"]["status"], "ok");
    }

    #[tokio::test]
    async fn test_ping_wrong_password() {
        let (state, _dir) = test_state();
        let app = build_test_router(state);
        let (status, body) = get_response(
            app,
            "/rest/ping?u=testuser&t=wrongtoken&s=abc&v=1.16.1&c=test",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("status=\"failed\""));
        assert!(body.contains("code=\"40\""));
    }

    #[tokio::test]
    async fn test_ping_wrong_username() {
        let (state, _dir) = test_state();
        let app = build_test_router(state);
        let salt = "abc123";
        let token = format!("{:x}", md5::compute(format!("testpass{}", salt)));
        let (_, body) = get_response(
            app,
            &format!(
                "/rest/ping?u=wronguser&t={}&s={}&v=1.16.1&c=test",
                token, salt
            ),
        )
        .await;
        assert!(body.contains("status=\"failed\""));
        assert!(body.contains("code=\"40\""));
    }

    #[tokio::test]
    async fn test_legacy_password_auth() {
        let (state, _dir) = test_state();
        let app = build_test_router(state);
        let (_, body) = get_response(app, "/rest/ping?u=testuser&p=testpass&v=1.16.1&c=test").await;
        assert!(body.contains("status=\"ok\""));
    }

    #[tokio::test]
    async fn test_get_license() {
        let (state, _dir) = test_state();
        let app = build_test_router(state);
        let (_, body) = get_response(app, &format!("/rest/getLicense?{}", auth_query(""))).await;
        assert!(body.contains("license"));
        assert!(body.contains("valid=\"true\""));
    }

    #[tokio::test]
    async fn test_get_artists_empty() {
        let (state, _dir) = test_state();
        let app = build_test_router(state);
        let (_, body) = get_response(app, &format!("/rest/getArtists?{}", auth_query(""))).await;
        assert!(body.contains("status=\"ok\""));
        assert!(body.contains("<artists"));
    }

    #[tokio::test]
    async fn test_get_artists_with_data() {
        let (state, _dir) = test_state();
        seed_data(&state);
        let app = build_test_router(state);
        let (_, body) = get_response(app, &format!("/rest/getArtists?{}", auth_query(""))).await;
        assert!(body.contains("Test Artist"));
    }

    #[tokio::test]
    async fn test_get_artist_by_id() {
        let (state, _dir) = test_state();
        seed_data(&state);

        let db = Database::open(&state.db_path).unwrap();
        let artists = queries::all_artists(&db.conn).unwrap();
        let artist = &artists[0];

        let app = build_test_router(state);
        let (_, body) = get_response(
            app,
            &format!("/rest/getArtist?{}&id={}", auth_query(""), artist.id),
        )
        .await;
        assert!(body.contains("Test Artist"));
        assert!(body.contains("Test Album"));
    }

    #[tokio::test]
    async fn test_get_album_by_id() {
        let (state, _dir) = test_state();
        seed_data(&state);

        let db = Database::open(&state.db_path).unwrap();
        let albums = queries::all_albums(&db.conn).unwrap();
        let album = &albums[0];

        let app = build_test_router(state);
        let (_, body) = get_response(
            app,
            &format!("/rest/getAlbum?{}&id={}", auth_query(""), album.id),
        )
        .await;
        assert!(body.contains("Test Album"));
        assert!(body.contains("Test Song"));
    }

    #[tokio::test]
    async fn test_get_song_by_id() {
        let (state, _dir) = test_state();
        seed_data(&state);

        let db = Database::open(&state.db_path).unwrap();
        let albums = queries::all_albums(&db.conn).unwrap();
        let tracks = queries::tracks_for_album(&db.conn, albums[0].id).unwrap();
        let track = &tracks[0];

        let app = build_test_router(state);
        let (_, body) = get_response(
            app,
            &format!("/rest/getSong?{}&id={}", auth_query(""), track.id),
        )
        .await;
        assert!(body.contains("Test Song"));
        assert!(body.contains("Test Artist"));
    }

    #[tokio::test]
    async fn test_get_album_list2() {
        let (state, _dir) = test_state();
        seed_data(&state);

        let app = build_test_router(state);
        let (_, body) = get_response(
            app,
            &format!(
                "/rest/getAlbumList2?{}&type=alphabeticalByName&size=10",
                auth_query("")
            ),
        )
        .await;
        assert!(body.contains("Test Album"));
    }

    #[tokio::test]
    async fn test_get_song_not_found() {
        let (state, _dir) = test_state();
        let app = build_test_router(state);
        let (_, body) =
            get_response(app, &format!("/rest/getSong?{}&id=99999", auth_query(""))).await;
        assert!(body.contains("status=\"failed\""));
        assert!(body.contains("code=\"70\""));
    }

    #[tokio::test]
    async fn test_json_response_format() {
        let (state, _dir) = test_state();
        seed_data(&state);

        let db = Database::open(&state.db_path).unwrap();
        let albums = queries::all_albums(&db.conn).unwrap();

        let app = build_test_router(state);
        let (_, body) = get_response(
            app,
            &format!(
                "/rest/getAlbum?{}&id={}",
                auth_query("f=json"),
                albums[0].id
            ),
        )
        .await;

        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["subsonic-response"]["status"], "ok");
        assert!(parsed["subsonic-response"]["album"].is_object());
    }

    #[tokio::test]
    async fn test_view_suffix_routes() {
        let (state, _dir) = test_state();
        let app = build_test_router(state);
        let (status, body) =
            get_response(app, &format!("/rest/ping.view?{}", auth_query(""))).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("status=\"ok\""));
    }
}
