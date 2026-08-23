//! Subsonic-compatible REST API layer.
//!
//! Implements a subset of the Subsonic/OpenSubsonic REST API backed by the
//! local koan database.  Supports both XML (default) and JSON (`f=json`)
//! responses.  Auth is `t=md5(password + s)` only, against a dedicated
//! `[subsonic]` secret — see `validate_auth`.

use std::collections::BTreeMap;
use std::io::Cursor;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Path as UrlPath, Query, RawQuery, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use koan_core::config::Config;
use koan_core::db::connection::Database;
use koan_core::db::queries;
use koan_core::index::metadata::extract_cover_art;
use koan_core::remote::client::SubsonicAuth;
use lru::LruCache;
use serde::Deserialize;
use tokio::io::AsyncReadExt as _;

const SUBSONIC_API_VERSION: &str = "1.16.1";
const SUBSONIC_XMLNS: &str = "http://subsonic.org/restapi";
const MIN_COVER_SIZE: u32 = 16;
const MAX_COVER_SIZE: u32 = 2048;

/// Rendered cover images held in memory. Each entry is one encoded JPEG/PNG at
/// one requested size; a client painting an album grid asks for a few hundred
/// in a burst, and re-decoding the source media file for each one dominated the
/// request.
const COVER_CACHE_ENTRIES: usize = 256;

/// Articles clients strip when sorting the artist index. Every real server
/// sends this; DSub sorts wrongly without it.
const IGNORED_ARTICLES: &str = "The El La Los Las Le Les";

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

struct AppState {
    db_path: PathBuf,
    username: String,
    password: String,
    /// Upstream Navidrome/Subsonic, used to build signed stream URLs for tracks
    /// with no local file. Resolved once at startup — resolving it per request
    /// re-read two TOML files and, on macOS, hit the keychain. Credentials
    /// rather than a `SubsonicClient`: that builds blocking `reqwest` clients,
    /// which panics when constructed inside the tokio runtime.
    upstream: Option<SubsonicAuth>,
    /// Async client for proxying those streams. `reqwest::Client` owns a
    /// connection pool, so it is built once and cloned.
    http: reqwest::Client,
    cover_cache: Mutex<LruCache<CoverKey, Arc<CachedCover>>>,
}

/// A cover image keyed by the entity it belongs to and the size asked for.
type CoverKey = (String, Option<u32>);

struct CachedCover {
    content_type: &'static str,
    bytes: Vec<u8>,
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

    fn bad_param(name: &str) -> Self {
        Self {
            code: SubsonicErrorCode::MissingParameter,
            message: format!("Invalid value for parameter '{}'", name),
        }
    }

    fn not_found(what: &str) -> Self {
        Self {
            code: SubsonicErrorCode::NotFound,
            message: format!("{} not found", what),
        }
    }

    fn unsupported(endpoint: &str) -> Self {
        Self {
            code: SubsonicErrorCode::NotFound,
            message: format!("Endpoint '{}' is not supported by this server", endpoint),
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

#[derive(Debug, Default, Deserialize)]
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

/// Query parameters kept as an ordered list of pairs.
///
/// `serde_urlencoded`, which axum's `Query` uses, cannot deserialise a repeated
/// key into a `Vec`. `createPlaylist` repeats `songId` once per track, so under
/// `Query` the extractor rejected the request with a bare HTTP 400 and the
/// handler never ran.
struct RawParams(Vec<(String, String)>);

impl RawParams {
    fn parse(query: Option<&str>) -> Self {
        Self(
            form_urlencoded::parse(query.unwrap_or_default().as_bytes())
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect(),
        )
    }

    fn get(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// Every value for `key`. Clients spell repeated parameters either
    /// `songId=1&songId=2` or `songId[]=1&songId[]=2`; both are accepted.
    fn all<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a str> {
        let bracketed = format!("{}[]", key);
        self.0
            .iter()
            .filter(move |(k, _)| k == key || *k == bracketed)
            .map(|(_, v)| v.as_str())
    }

    fn auth(&self) -> SubsonicParams {
        SubsonicParams {
            u: self.get("u").map(String::from),
            t: self.get("t").map(String::from),
            s: self.get("s").map(String::from),
            p: self.get("p").map(String::from),
            v: self.get("v").map(String::from),
            c: self.get("c").map(String::from),
            f: self.get("f").map(String::from),
        }
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

/// An attribute value with its wire type preserved.
///
/// XML has only text, so every variant renders identically there. JSON is
/// typed, and the OpenSubsonic schema says `duration`/`track`/`bitRate`/`year`
/// are ints, `size` is a long and `isDir` is a boolean. Clients with generated
/// deserialisers abort a library sync on the first song when those arrive
/// quoted.
#[derive(Clone)]
enum AttrValue {
    Str(String),
    Int(i64),
    Bool(bool),
}

impl AttrValue {
    fn to_xml_text(&self) -> String {
        match self {
            AttrValue::Str(s) => s.clone(),
            AttrValue::Int(n) => n.to_string(),
            AttrValue::Bool(b) => b.to_string(),
        }
    }

    fn to_json(&self) -> serde_json::Value {
        match self {
            AttrValue::Str(s) => serde_json::Value::String(s.clone()),
            AttrValue::Int(n) => serde_json::Value::Number((*n).into()),
            AttrValue::Bool(b) => serde_json::Value::Bool(*b),
        }
    }
}

#[derive(Clone)]
struct XmlNode {
    tag: String,
    attrs: Vec<(String, AttrValue)>,
    /// Element text. The XSD carries a few values this way
    /// (`<genre songCount="6">Noise</genre>`); the JSON mapping spells the same
    /// thing as a `value` member.
    text: Option<String>,
    children: Vec<XmlNode>,
    is_array: bool,
    array_child_tag: Option<String>,
}

impl XmlNode {
    fn new(tag: &str) -> Self {
        Self {
            tag: tag.into(),
            attrs: Vec::new(),
            text: None,
            children: Vec::new(),
            is_array: false,
            array_child_tag: None,
        }
    }

    fn attr(mut self, key: &str, value: &str) -> Self {
        self.attrs.push((key.into(), AttrValue::Str(value.into())));
        self
    }

    fn attr_int(mut self, key: &str, value: i64) -> Self {
        self.attrs.push((key.into(), AttrValue::Int(value)));
        self
    }

    fn attr_bool(mut self, key: &str, value: bool) -> Self {
        self.attrs.push((key.into(), AttrValue::Bool(value)));
        self
    }

    fn attr_opt(self, key: &str, value: Option<&str>) -> Self {
        match value {
            Some(v) => self.attr(key, v),
            None => self,
        }
    }

    fn attr_opt_int(self, key: &str, value: Option<i64>) -> Self {
        match value {
            Some(v) => self.attr_int(key, v),
            None => self,
        }
    }

    fn text(mut self, value: &str) -> Self {
        self.text = Some(value.into());
        self
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
            s.push_str(&format!(" {}=\"{}\"", k, xml_escape(&v.to_xml_text())));
        }
        if self.children.is_empty() {
            return match &self.text {
                Some(t) => format!("{}{}>{}</{}>", pad, s, xml_escape(t), self.tag),
                None => format!("{}{}/>", pad, s),
            };
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
            obj.insert(k.clone(), v.to_json());
        }
        if let Some(text) = &self.text {
            obj.insert("value".into(), serde_json::Value::String(text.clone()));
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
// Auth
// ---------------------------------------------------------------------------

/// Validate `u` + `t` + `s` against the configured `[subsonic]` credentials.
///
/// Token auth only. `p=` (plaintext, and its `enc:` hex dressing) is refused:
/// the protocol offers no transport guarantee, so accepting it means handing the
/// secret to anyone watching the wire. `t=md5(password + attacker-chosen salt)`
/// leaks an offline-crackable digest, which is why the secret is a generated
/// 256-bit value rather than something a human picked.
fn validate_auth(params: &SubsonicParams, state: &AppState) -> Result<(), SubsonicError> {
    use subtle::ConstantTimeEq;

    let username = params
        .u
        .as_deref()
        .ok_or_else(|| SubsonicError::missing_param("u"))?;

    let user_ok: bool = username
        .as_bytes()
        .ct_eq(state.username.as_bytes())
        .unwrap_u8()
        == 1;

    let (Some(token), Some(salt)) = (params.t.as_deref(), params.s.as_deref()) else {
        if params.p.is_some() {
            return Err(SubsonicError::wrong_auth());
        }
        return Err(SubsonicError::missing_param("t and s"));
    };

    let expected = format!("{:x}", md5::compute(format!("{}{}", state.password, salt)));
    let token_ok: bool = token.as_bytes().ct_eq(expected.as_bytes()).unwrap_u8() == 1;

    if user_ok && token_ok {
        Ok(())
    } else {
        Err(SubsonicError::wrong_auth())
    }
}

// ---------------------------------------------------------------------------
// Request prologue
// ---------------------------------------------------------------------------

/// Authenticate, open the database, and render — the prologue every browsing
/// endpoint shares. Errors come back in the format the request asked for.
fn respond_db(
    state: &AppState,
    auth: &SubsonicParams,
    f: impl FnOnce(&Database, XmlBuilder) -> Result<XmlBuilder, SubsonicError>,
) -> Response {
    let json = auth.wants_json();
    let result = validate_auth(auth, state)
        .and_then(|()| state.open_db())
        .and_then(|db| f(&db, SubsonicResponse::ok(json)));
    match result {
        Ok(builder) => builder.build(),
        Err(e) => SubsonicResponse::error(json, &e),
    }
}

/// As `respond_db`, for endpoints that never touch the database.
fn respond(
    state: &AppState,
    auth: &SubsonicParams,
    f: impl FnOnce(XmlBuilder) -> Result<XmlBuilder, SubsonicError>,
) -> Response {
    let json = auth.wants_json();
    match validate_auth(auth, state).and_then(|()| f(SubsonicResponse::ok(json))) {
        Ok(builder) => builder.build(),
        Err(e) => SubsonicResponse::error(json, &e),
    }
}

/// Prologue for the two endpoints that answer with bytes rather than a
/// document, and so cannot go through `respond_db`.
fn authed_db(state: &AppState, auth: &SubsonicParams) -> Result<Database, SubsonicError> {
    validate_auth(auth, state)?;
    state.open_db()
}

// ---------------------------------------------------------------------------
// Entity ids
// ---------------------------------------------------------------------------

/// Artists, albums and songs all draw their ids from the same `i64` space, so
/// any id a client can hand back to a *different* endpoint carries a type
/// prefix — without one, `getCoverArt?id=5` meaning "album 5" silently served
/// track 5's art. Navidrome spells these the same way.
const ARTIST_PREFIX: &str = "ar-";
const ALBUM_PREFIX: &str = "al-";
const SONG_PREFIX: &str = "mf-";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntityKind {
    Artist,
    Album,
    Song,
}

/// Parse `ar-3`, `al-3`, `mf-3`, or a bare `3`.
///
/// The ID3 endpoints (`getArtists`, `getAlbum`, `getSong`) still publish bare
/// ids, so both spellings arrive and both have to resolve.
fn parse_entity_id(raw: &str) -> Option<(Option<EntityKind>, i64)> {
    for (prefix, kind) in [
        (ARTIST_PREFIX, EntityKind::Artist),
        (ALBUM_PREFIX, EntityKind::Album),
        (SONG_PREFIX, EntityKind::Song),
    ] {
        if let Some(rest) = raw.strip_prefix(prefix) {
            return rest.parse().ok().map(|id| (Some(kind), id));
        }
    }
    raw.parse().ok().map(|id| (None, id))
}

/// The `id` parameter as a plain row id, ignoring any type prefix.
fn require_id(raw: Option<&str>) -> Result<i64, SubsonicError> {
    let raw = raw.ok_or_else(|| SubsonicError::missing_param("id"))?;
    parse_entity_id(raw)
        .map(|(_, id)| id)
        .ok_or_else(|| SubsonicError::bad_param("id"))
}

/// The `id` parameter with its type prefix, for endpoints that serve more than
/// one kind of entity.
fn require_entity(raw: Option<&str>) -> Result<(Option<EntityKind>, i64), SubsonicError> {
    let raw = raw.ok_or_else(|| SubsonicError::missing_param("id"))?;
    parse_entity_id(raw).ok_or_else(|| SubsonicError::bad_param("id"))
}

// ---------------------------------------------------------------------------
// Helpers: track/album/artist → XmlNode
// ---------------------------------------------------------------------------

/// A track as a `Child` element.
///
/// The tag varies by context — `song` in most responses, `entry` inside a
/// playlist, `child` inside a music directory — while the attributes do not.
fn track_node(track: &queries::TrackRow, tag: &str) -> XmlNode {
    let duration_secs = track.duration_ms.map(|ms| ms / 1000);
    let (suffix, content_type) = track
        .codec
        .as_deref()
        .map(codec_to_mime)
        .unwrap_or(("bin", "application/octet-stream"));
    XmlNode::new(tag)
        .attr("id", &track.id.to_string())
        .attr("title", &track.title)
        .attr("album", &track.album_title)
        .attr("artist", &track.artist_name)
        .attr_opt_int("track", track.track_number.map(i64::from))
        .attr_opt_int("discNumber", track.disc.map(i64::from))
        .attr_opt_int("duration", duration_secs)
        .attr_opt_int("bitRate", track.bitrate.map(i64::from))
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
        .attr_opt(
            "parent",
            track
                .album_id
                .map(|id| format!("{}{}", ALBUM_PREFIX, id))
                .as_deref(),
        )
        .attr("coverArt", &format!("{}{}", SONG_PREFIX, track.id))
        .attr("type", "music")
        .attr_bool("isDir", false)
}

fn track_to_xml_node(track: &queries::TrackRow) -> XmlNode {
    track_node(track, "song")
}

fn year_from_date(date: Option<&str>) -> Option<i64> {
    date.and_then(|d| d.get(..4)).and_then(|y| y.parse().ok())
}

/// An album as an `AlbumID3` element. `title` rides alongside `name` because
/// the file-browse half of the protocol spells it that way and clients mix the
/// two freely.
fn album_to_xml_node(album: &queries::AlbumRow, track_count: Option<i32>) -> XmlNode {
    XmlNode::new("album")
        .attr("id", &album.id.to_string())
        .attr("name", &album.title)
        .attr("title", &album.title)
        .attr("artist", &album.artist_name)
        .attr("artistId", &album.artist_id.to_string())
        .attr("parent", &format!("{}{}", ARTIST_PREFIX, album.artist_id))
        .attr("coverArt", &format!("{}{}", ALBUM_PREFIX, album.id))
        .attr_int("songCount", i64::from(track_count.unwrap_or(0)))
        .attr_opt_int("year", year_from_date(album.date.as_deref()))
        .attr_bool("isDir", true)
}

/// An album as a directory `child`, for the file-browse endpoints. The id is
/// prefixed here because it comes straight back as `getMusicDirectory?id=`.
fn album_child_node(album: &queries::AlbumRow) -> XmlNode {
    XmlNode::new("child")
        .attr("id", &format!("{}{}", ALBUM_PREFIX, album.id))
        .attr("parent", &format!("{}{}", ARTIST_PREFIX, album.artist_id))
        .attr("title", &album.title)
        .attr("album", &album.title)
        .attr("artist", &album.artist_name)
        .attr("coverArt", &format!("{}{}", ALBUM_PREFIX, album.id))
        .attr_opt_int("year", year_from_date(album.date.as_deref()))
        .attr_bool("isDir", true)
}

fn album_counts_by_artist(db: &Database) -> Result<BTreeMap<i64, i64>, SubsonicError> {
    let albums =
        queries::all_albums(&db.conn).map_err(|e| SubsonicError::internal(e.to_string()))?;
    let mut map: BTreeMap<i64, i64> = BTreeMap::new();
    for album in albums {
        *map.entry(album.artist_id).or_insert(0) += 1;
    }
    Ok(map)
}

/// Artists bucketed by first letter, plus each artist's album count — the shape
/// `getArtists` (ID3) and `getIndexes` (file-browse) both hang off.
type ArtistIndex = (
    BTreeMap<String, Vec<queries::ArtistRow>>,
    BTreeMap<i64, i64>,
);

fn artist_index(db: &Database) -> Result<ArtistIndex, SubsonicError> {
    let artists =
        queries::all_artists(&db.conn).map_err(|e| SubsonicError::internal(e.to_string()))?;

    let mut index_map: BTreeMap<String, Vec<queries::ArtistRow>> = BTreeMap::new();
    for artist in artists {
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

    Ok((index_map, album_counts_by_artist(db)?))
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
struct CoverArtParams {
    #[serde(flatten)]
    auth: SubsonicParams,
    id: Option<String>,
    size: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScrobbleParams {
    #[serde(flatten)]
    auth: SubsonicParams,
    id: Option<String>,
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
    id: Option<String>,
    count: Option<usize>,
}

// ===========================================================================
// Endpoints — browsing
// ===========================================================================

async fn ping(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    respond(&state, &params, Ok)
}

async fn get_license(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    respond(&state, &params, |b| {
        Ok(b.child(
            XmlNode::new("license")
                .attr_bool("valid", true)
                .attr("email", "koan@localhost"),
        ))
    })
}

async fn get_artists(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    respond_db(&state, &params, |db, b| {
        let (index_map, album_counts) = artist_index(db)?;

        let mut artists_node = XmlNode::new("artists")
            .attr("ignoredArticles", IGNORED_ARTICLES)
            .array_of("index");
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
                        .attr("coverArt", &format!("{}{}", ARTIST_PREFIX, artist.id))
                        .attr_int("albumCount", count),
                );
            }
            artists_node = artists_node.child(index_node);
        }

        Ok(b.child(artists_node))
    })
}

/// The file-browse counterpart of `getArtists`. DSub and every folder-oriented
/// client enumerate the library through this and `getMusicDirectory`, so
/// without them they see an empty server.
async fn get_indexes(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    respond_db(&state, &params, |db, b| {
        let (index_map, _) = artist_index(db)?;
        let last_modified: i64 = db
            .conn
            .query_row(
                "SELECT COALESCE(MAX(mtime), 0) * 1000 FROM tracks",
                [],
                |r| r.get(0),
            )
            .map_err(|e| SubsonicError::internal(e.to_string()))?;

        let mut indexes_node = XmlNode::new("indexes")
            .attr_int("lastModified", last_modified)
            .attr("ignoredArticles", IGNORED_ARTICLES)
            .array_of("index");
        for (letter, group) in &index_map {
            let mut index_node = XmlNode::new("index")
                .attr("name", letter)
                .array_of("artist");
            for artist in group {
                index_node = index_node.child(
                    XmlNode::new("artist")
                        .attr("id", &format!("{}{}", ARTIST_PREFIX, artist.id))
                        .attr("name", &artist.name),
                );
            }
            indexes_node = indexes_node.child(index_node);
        }

        Ok(b.child(indexes_node))
    })
}

/// One level of the browse tree: an artist directory lists its albums, an album
/// directory lists its songs.
async fn get_music_directory(
    State(state): State<Arc<AppState>>,
    Query(params): Query<IdParam>,
) -> Response {
    respond_db(&state, &params.auth, |db, b| {
        let (kind, id) = require_entity(params.id.as_deref())?;

        // A bare id is ambiguous — artists and albums share the number space —
        // so try the artist table first and fall through. Clients that arrived
        // via `getIndexes` always send a prefix and never hit this.
        if kind != Some(EntityKind::Album) {
            let artists = queries::all_artists(&db.conn)
                .map_err(|e| SubsonicError::internal(e.to_string()))?;
            if let Some(artist) = artists.into_iter().find(|a| a.id == id) {
                let albums = queries::albums_for_artist(&db.conn, id)
                    .map_err(|e| SubsonicError::internal(e.to_string()))?;
                let mut dir = XmlNode::new("directory")
                    .attr("id", &format!("{}{}", ARTIST_PREFIX, artist.id))
                    .attr("name", &artist.name)
                    .array_of("child");
                for album in &albums {
                    dir = dir.child(album_child_node(album));
                }
                return Ok(b.child(dir));
            }
        }

        let album = queries::get_album(&db.conn, id)
            .map_err(|e| SubsonicError::internal(e.to_string()))?
            .ok_or_else(|| SubsonicError::not_found("Directory"))?;
        let tracks = queries::tracks_for_album(&db.conn, id)
            .map_err(|e| SubsonicError::internal(e.to_string()))?;

        let mut dir = XmlNode::new("directory")
            .attr("id", &format!("{}{}", ALBUM_PREFIX, album.id))
            .attr("parent", &format!("{}{}", ARTIST_PREFIX, album.artist_id))
            .attr("name", &album.title)
            .array_of("child");
        for track in &tracks {
            dir = dir.child(track_node(track, "child"));
        }
        Ok(b.child(dir))
    })
}

async fn get_artist(State(state): State<Arc<AppState>>, Query(params): Query<IdParam>) -> Response {
    respond_db(&state, &params.auth, |db, b| {
        let artist_id = require_id(params.id.as_deref())?;

        let all =
            queries::all_artists(&db.conn).map_err(|e| SubsonicError::internal(e.to_string()))?;
        let artist = all
            .into_iter()
            .find(|a| a.id == artist_id)
            .ok_or_else(|| SubsonicError::not_found("Artist"))?;

        let albums = queries::albums_for_artist(&db.conn, artist_id)
            .map_err(|e| SubsonicError::internal(e.to_string()))?;

        let mut artist_node = XmlNode::new("artist")
            .attr("id", &artist.id.to_string())
            .attr("name", &artist.name)
            .attr("coverArt", &format!("{}{}", ARTIST_PREFIX, artist.id))
            .attr_int("albumCount", albums.len() as i64)
            .array_of("album");

        for album in &albums {
            artist_node = artist_node.child(album_to_xml_node(album, album.total_tracks));
        }

        Ok(b.child(artist_node))
    })
}

async fn get_album(State(state): State<Arc<AppState>>, Query(params): Query<IdParam>) -> Response {
    respond_db(&state, &params.auth, |db, b| {
        let album_id = require_id(params.id.as_deref())?;

        let album = queries::get_album(&db.conn, album_id)
            .map_err(|e| SubsonicError::internal(e.to_string()))?
            .ok_or_else(|| SubsonicError::not_found("Album"))?;

        let tracks = queries::tracks_for_album(&db.conn, album_id)
            .map_err(|e| SubsonicError::internal(e.to_string()))?;
        let mut album_node = album_to_xml_node(&album, Some(tracks.len() as i32)).array_of("song");

        for track in &tracks {
            album_node = album_node.child(track_to_xml_node(track));
        }

        Ok(b.child(album_node))
    })
}

/// Albums ordered by `type`, paged. Shared by `getAlbumList` and
/// `getAlbumList2`, which differ only in the element they hang the list off.
fn album_list(
    db: &Database,
    params: &AlbumListParams,
    tag: &str,
) -> Result<XmlNode, SubsonicError> {
    let list_type = params.list_type.as_deref().unwrap_or("alphabeticalByName");
    let size = params.size.unwrap_or(20).clamp(0, 500) as usize;
    let offset = params.offset.unwrap_or(0).max(0) as usize;

    let mut albums =
        queries::all_albums(&db.conn).map_err(|e| SubsonicError::internal(e.to_string()))?;

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

    let mut list_node = XmlNode::new(tag).array_of("album");
    for album in albums.into_iter().skip(offset).take(size) {
        list_node = list_node.child(album_to_xml_node(&album, album.total_tracks));
    }
    Ok(list_node)
}

async fn get_album_list(
    State(state): State<Arc<AppState>>,
    Query(params): Query<AlbumListParams>,
) -> Response {
    respond_db(&state, &params.auth, |db, b| {
        Ok(b.child(album_list(db, &params, "albumList")?))
    })
}

async fn get_album_list2(
    State(state): State<Arc<AppState>>,
    Query(params): Query<AlbumListParams>,
) -> Response {
    respond_db(&state, &params.auth, |db, b| {
        Ok(b.child(album_list(db, &params, "albumList2")?))
    })
}

async fn get_song(State(state): State<Arc<AppState>>, Query(params): Query<IdParam>) -> Response {
    respond_db(&state, &params.auth, |db, b| {
        let track_id = require_id(params.id.as_deref())?;
        let track = queries::get_track_row(&db.conn, track_id)
            .map_err(|e| SubsonicError::internal(e.to_string()))?
            .ok_or_else(|| SubsonicError::not_found("Song"))?;
        Ok(b.child(track_to_xml_node(&track)))
    })
}

// ===========================================================================
// Endpoints — search
// ===========================================================================

async fn search3(
    State(state): State<Arc<AppState>>,
    Query(params): Query<Search3Params>,
) -> Response {
    respond_db(&state, &params.auth, |db, b| {
        let query = params
            .query
            .as_deref()
            .ok_or_else(|| SubsonicError::missing_param("query"))?;

        let artist_count = params.artist_count.unwrap_or(20);
        let album_count = params.album_count.unwrap_or(20);
        let song_count = params.song_count.unwrap_or(20);

        let total_needed = (artist_count + album_count + song_count).max(100);
        let tracks = queries::search_tracks_paged(&db.conn, query, total_needed, 0)
            .map_err(|e| SubsonicError::internal(e.to_string()))?;

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
                        .attr("name", &t.artist_name)
                        .attr("coverArt", &format!("{}{}", ARTIST_PREFIX, aid)),
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
                        .attr("title", &t.album_title)
                        .attr("artist", &t.album_artist_name)
                        .attr("coverArt", &format!("{}{}", ALBUM_PREFIX, alid))
                        .attr_bool("isDir", true),
                );
                album_n += 1;
            }
        }

        // Songs.
        for t in tracks.iter().take(song_count as usize) {
            result_node = result_node.child(track_to_xml_node(t));
        }

        Ok(b.child(result_node))
    })
}

// ===========================================================================
// Endpoints — streaming
// ===========================================================================

#[derive(Debug, Deserialize)]
struct StreamParams {
    #[serde(flatten)]
    auth: SubsonicParams,
    /// A `String`, not an `i64`: axum's `Query` rejects a value it cannot
    /// deserialise with a plain-text HTTP 400 *before* the handler runs, which
    /// is neither a Subsonic envelope nor something a client can report.
    id: Option<String>,
}

async fn stream(
    State(state): State<Arc<AppState>>,
    Query(params): Query<StreamParams>,
    headers: HeaderMap,
) -> Response {
    let json = params.auth.wants_json();
    match stream_inner(&state, &params, &headers).await {
        Ok(resp) => resp,
        Err(e) => SubsonicResponse::error(json, &e),
    }
}

async fn stream_inner(
    state: &AppState,
    params: &StreamParams,
    headers: &HeaderMap,
) -> Result<Response, SubsonicError> {
    let db = authed_db(state, &params.auth)?;
    let track_id = require_id(params.id.as_deref())?;

    let track = queries::get_track_row(&db.conn, track_id)
        .map_err(|e| SubsonicError::internal(e.to_string()))?
        .ok_or_else(|| SubsonicError::not_found("Track"))?;

    // Try local/cached file first; fall back to proxying from upstream.
    let local_path = track_file_path(&track).map(PathBuf::from);
    let local_exists = if let Some(ref p) = local_path {
        tokio::fs::metadata(p).await.is_ok()
    } else {
        false
    };

    if !local_exists {
        // Proxy from upstream Navidrome/Subsonic server.
        if let Some(ref remote_id) = track.remote_id {
            return proxy_stream_from_upstream(state, remote_id, &track, headers).await;
        }
        return Err(SubsonicError::not_found(
            "Track has no local file and no remote source",
        ));
    }

    let path = local_path.unwrap();
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

        match parse_range(range_str, total_size) {
            RangeRequest::Satisfiable { start, end } => {
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
            RangeRequest::Unsatisfiable => {
                return Response::builder()
                    .status(StatusCode::RANGE_NOT_SATISFIABLE)
                    .header(header::CONTENT_RANGE, format!("bytes */{}", total_size))
                    .header(header::ACCEPT_RANGES, "bytes")
                    .body(axum::body::Body::empty())
                    .map_err(|e| SubsonicError::internal(e.to_string()));
            }
            // A header that does not parse is ignored and the whole body sent.
            RangeRequest::Malformed => {}
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

/// Proxy a stream from the upstream Navidrome/Subsonic server.
/// Forwards the audio bytes through to the client, passing along Range headers.
async fn proxy_stream_from_upstream(
    state: &AppState,
    remote_id: &str,
    track: &queries::TrackRow,
    client_headers: &HeaderMap,
) -> Result<Response, SubsonicError> {
    let upstream_url = state
        .upstream
        .as_ref()
        .ok_or_else(|| SubsonicError::not_found("Remote server not configured"))?
        .stream_url(remote_id)
        .map_err(|e| SubsonicError::internal(e.to_string()))?;

    let mut req = state.http.get(&upstream_url);

    // Forward Range header if present.
    if let Some(range) = client_headers.get(header::RANGE)
        && let Ok(range_str) = range.to_str()
    {
        req = req.header("Range", range_str);
    }

    let upstream_resp = req
        .send()
        .await
        .map_err(|e| SubsonicError::internal(format!("upstream error: {}", e)))?;

    let status = upstream_resp.status();
    let content_type = track
        .codec
        .as_deref()
        .map(|c| codec_to_mime(c).1)
        .unwrap_or("application/octet-stream");

    let mut builder = Response::builder().status(status.as_u16());
    builder = builder.header(header::CONTENT_TYPE, content_type);

    // Forward content-length and range headers from upstream.
    if let Some(cl) = upstream_resp.headers().get(header::CONTENT_LENGTH) {
        builder = builder.header(header::CONTENT_LENGTH, cl);
    }
    if let Some(cr) = upstream_resp.headers().get(header::CONTENT_RANGE) {
        builder = builder.header(header::CONTENT_RANGE, cr);
    }
    builder = builder.header(header::ACCEPT_RANGES, "bytes");

    let body = axum::body::Body::from_stream(upstream_resp.bytes_stream());
    builder
        .body(body)
        .map_err(|e| SubsonicError::internal(e.to_string()))
}

/// What a `Range` header asks for.
///
/// RFC 9110 draws a line a bare `Option` could not: a header that does not
/// parse is ignored and the whole body sent, while a well-formed range outside
/// the file is a 416 carrying `Content-Range: bytes */<total>`.
#[derive(Debug, PartialEq, Eq)]
enum RangeRequest {
    Satisfiable { start: u64, end: u64 },
    Unsatisfiable,
    Malformed,
}

fn parse_range(range: &str, total: u64) -> RangeRequest {
    let Some(spec) = range.strip_prefix("bytes=") else {
        return RangeRequest::Malformed;
    };
    let Some((start_str, end_str)) = spec.split_once('-') else {
        return RangeRequest::Malformed;
    };
    let (start_str, end_str) = (start_str.trim(), end_str.trim());

    if start_str.is_empty() {
        let Ok(suffix) = end_str.parse::<u64>() else {
            return RangeRequest::Malformed;
        };
        if total == 0 || suffix == 0 {
            return RangeRequest::Unsatisfiable;
        }
        return RangeRequest::Satisfiable {
            start: total.saturating_sub(suffix),
            end: total - 1,
        };
    }

    let Ok(start) = start_str.parse::<u64>() else {
        return RangeRequest::Malformed;
    };
    if total == 0 {
        return RangeRequest::Unsatisfiable;
    }
    let end = if end_str.is_empty() {
        total - 1
    } else {
        let Ok(end) = end_str.parse::<u64>() else {
            return RangeRequest::Malformed;
        };
        end.min(total - 1)
    };
    if start > end || start >= total {
        return RangeRequest::Unsatisfiable;
    }
    RangeRequest::Satisfiable { start, end }
}

// ===========================================================================
// Endpoints — cover art
// ===========================================================================

async fn get_cover_art(
    State(state): State<Arc<AppState>>,
    Query(params): Query<CoverArtParams>,
) -> Response {
    let json = params.auth.wants_json();
    match cover_art_inner(&state, &params) {
        Ok(resp) => resp,
        Err(e) => SubsonicResponse::error(json, &e),
    }
}

fn cover_art_inner(state: &AppState, params: &CoverArtParams) -> Result<Response, SubsonicError> {
    let db = authed_db(state, &params.auth)?;
    let (kind, id) = require_entity(params.id.as_deref())?;
    let size = params.size.map(|s| s.clamp(MIN_COVER_SIZE, MAX_COVER_SIZE));

    let key = (
        match kind {
            Some(EntityKind::Artist) => format!("{}{}", ARTIST_PREFIX, id),
            Some(EntityKind::Album) => format!("{}{}", ALBUM_PREFIX, id),
            Some(EntityKind::Song) | None => format!("{}{}", SONG_PREFIX, id),
        },
        size,
    );

    if let Some(hit) = state.cover_cache.lock().unwrap().get(&key).cloned() {
        return Ok(cover_response(&hit));
    }

    let path = cover_source_path(&db, kind, id)?;
    let art_bytes = extract_cover_art(&path)
        .ok_or_else(|| SubsonicError::not_found("No cover art embedded"))?;

    let (content_type, is_png) = if art_bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        ("image/png", true)
    } else {
        ("image/jpeg", false)
    };

    let bytes = match size {
        Some(size) => resize_image(&art_bytes, size, is_png)?,
        None => art_bytes,
    };

    let entry = Arc::new(CachedCover {
        content_type,
        bytes,
    });
    state
        .cover_cache
        .lock()
        .unwrap()
        .put(key, Arc::clone(&entry));
    Ok(cover_response(&entry))
}

fn cover_response(cover: &CachedCover) -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, cover.content_type),
            (header::CACHE_CONTROL, "max-age=86400"),
        ],
        cover.bytes.clone(),
    )
        .into_response()
}

/// The media file whose embedded art answers a `getCoverArt` id. Album and
/// artist ids resolve through their first track — koan stores no standalone
/// cover images.
fn cover_source_path(
    db: &Database,
    kind: Option<EntityKind>,
    id: i64,
) -> Result<PathBuf, SubsonicError> {
    let track = match kind {
        Some(EntityKind::Album) => queries::tracks_for_album(&db.conn, id)
            .map_err(|e| SubsonicError::internal(e.to_string()))?
            .into_iter()
            .next()
            .ok_or_else(|| SubsonicError::not_found("Album"))?,
        Some(EntityKind::Artist) => {
            let albums = queries::albums_for_artist(&db.conn, id)
                .map_err(|e| SubsonicError::internal(e.to_string()))?;
            albums
                .iter()
                .find_map(|album| {
                    queries::tracks_for_album(&db.conn, album.id)
                        .ok()
                        .and_then(|tracks| tracks.into_iter().next())
                })
                .ok_or_else(|| SubsonicError::not_found("Artist"))?
        }
        Some(EntityKind::Song) | None => queries::get_track_row(&db.conn, id)
            .map_err(|e| SubsonicError::internal(e.to_string()))?
            .ok_or_else(|| SubsonicError::not_found("Track"))?,
    };

    track_file_path(&track)
        .map(PathBuf::from)
        .ok_or_else(|| SubsonicError::not_found("Track has no local file"))
}

/// `image`'s `resize` upscales, and the allocation for the result is not fallible
/// — an oversized `size=` would abort the process rather than return an error.
/// Clamped, and never larger than the source.
fn resize_image(data: &[u8], size: u32, output_png: bool) -> Result<Vec<u8>, SubsonicError> {
    use image::GenericImageView as _;

    let img = image::load_from_memory(data)
        .map_err(|e| SubsonicError::internal(format!("image decode error: {}", e)))?;
    let (w, h) = img.dimensions();
    let size = size.clamp(MIN_COVER_SIZE, MAX_COVER_SIZE).min(w.max(h));
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

async fn star(State(state): State<Arc<AppState>>, Query(params): Query<IdParam>) -> Response {
    respond_db(&state, &params.auth, |db, b| {
        toggle_star(db, params.id.as_deref(), queries::add_favourite)?;
        Ok(b)
    })
}

async fn unstar(State(state): State<Arc<AppState>>, Query(params): Query<IdParam>) -> Response {
    respond_db(&state, &params.auth, |db, b| {
        toggle_star(db, params.id.as_deref(), queries::remove_favourite)?;
        Ok(b)
    })
}

fn toggle_star(
    db: &Database,
    id: Option<&str>,
    op: fn(&rusqlite::Connection, &std::path::Path) -> rusqlite::Result<()>,
) -> Result<(), SubsonicError> {
    let track_id = require_id(id)?;

    let track = queries::get_track_row(&db.conn, track_id)
        .map_err(|e| SubsonicError::internal(e.to_string()))?
        .ok_or_else(|| SubsonicError::not_found("Track"))?;

    let path_str = track_file_path(&track).unwrap_or("");
    op(&db.conn, std::path::Path::new(path_str)).map_err(|e| SubsonicError::internal(e.to_string()))
}

async fn get_starred2(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    respond_db(&state, &params, |db, b| {
        let favourites = queries::load_favourites(&db.conn)
            .map_err(|e| SubsonicError::internal(e.to_string()))?;

        let mut starred_node = XmlNode::new("starred2").array_of("song");
        for fav_path in &favourites {
            let path_str = fav_path.to_string_lossy();
            if let Ok(Some(track_id)) = queries::track_id_by_path(&db.conn, &path_str)
                && let Ok(Some(track)) = queries::get_track_row(&db.conn, track_id)
            {
                starred_node = starred_node.child(track_to_xml_node(&track));
            }
        }

        Ok(b.child(starred_node))
    })
}

async fn scrobble(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ScrobbleParams>,
) -> Response {
    respond_db(&state, &params.auth, |db, b| {
        let track_id = require_id(params.id.as_deref())?;

        queries::get_track_row(&db.conn, track_id)
            .map_err(|e| SubsonicError::internal(e.to_string()))?
            .ok_or_else(|| SubsonicError::not_found("Track"))?;

        // `time` is when the client played it, which can be well in the past
        // after an offline session.
        let played_at = params.time.map_or_else(
            || {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64
            },
            |time_ms| time_ms / 1000,
        );

        queries::record_play_at(
            &db.conn,
            track_id,
            played_at,
            None,
            queries::SOURCE_SUBSONIC,
        )
        .map_err(|e| SubsonicError::from(format!("Database error: {}", e)))?;
        Ok(b)
    })
}

async fn get_random_songs(
    State(state): State<Arc<AppState>>,
    Query(params): Query<RandomSongsParams>,
) -> Response {
    respond_db(&state, &params.auth, |db, b| {
        let size = params.size.unwrap_or(10);
        let genre = params.genre.as_deref();
        let fetch_count = if genre.is_some() { size * 5 } else { size };

        let tracks = queries::random_tracks(&db.conn, fetch_count, None)
            .map_err(|e| SubsonicError::internal(e.to_string()))?;

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

        Ok(b.child(node))
    })
}

async fn get_similar_songs2(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SimilarSongs2Params>,
) -> Response {
    respond_db(&state, &params.auth, |db, b| {
        let track_id = require_id(params.id.as_deref())?;
        let count = params.count.unwrap_or(50);

        let track = queries::get_track_row(&db.conn, track_id)
            .map_err(|e| SubsonicError::internal(e.to_string()))?
            .ok_or_else(|| SubsonicError::not_found("Track"))?;

        let Some(artist_id) = track.artist_id else {
            return Ok(b.child(XmlNode::new("similarSongs2")));
        };

        let similar = queries::get_similar_artists(&db.conn, artist_id)
            .map_err(|e| SubsonicError::internal(e.to_string()))?;

        let mut node = XmlNode::new("similarSongs2").array_of("song");
        let mut total = 0usize;
        for (artist_row, _score) in &similar {
            if total >= count {
                break;
            }
            let Ok(artist_tracks) = queries::tracks_for_artist(&db.conn, artist_row.id) else {
                continue;
            };
            for t in &artist_tracks {
                if total >= count {
                    break;
                }
                node = node.child(track_to_xml_node(t));
                total += 1;
            }
        }

        Ok(b.child(node))
    })
}

// ---------------------------------------------------------------------------
// Endpoints — server/user metadata
// ---------------------------------------------------------------------------

async fn get_music_folders(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    respond(&state, &params, |b| {
        Ok(b.child(
            XmlNode::new("musicFolders").child(
                XmlNode::new("musicFolder")
                    .attr("id", "1")
                    .attr("name", "Music"),
            ),
        ))
    })
}

/// Clients call this during setup to decide which features to offer. koan has
/// exactly one user — the `[subsonic]` credentials — and no admin surface.
async fn get_user(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    respond(&state, &params, |b| {
        Ok(b.child(
            XmlNode::new("user")
                .attr("username", &state.username)
                .attr_bool("scrobblingEnabled", true)
                .attr_bool("adminRole", false)
                .attr_bool("settingsRole", false)
                .attr_bool("downloadRole", true)
                .attr_bool("uploadRole", false)
                .attr_bool("playlistRole", true)
                .attr_bool("coverArtRole", true)
                .attr_bool("commentRole", false)
                .attr_bool("podcastRole", false)
                .attr_bool("streamRole", true)
                .attr_bool("jukeboxRole", false)
                .attr_bool("shareRole", false)
                .attr_bool("videoConversionRole", false),
        ))
    })
}

/// Scans are driven by `koan scan`, never by a client, so this only ever
/// reports the library size. Clients poll it after a connection test.
async fn get_scan_status(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    respond_db(&state, &params, |db, b| {
        let stats =
            queries::library_stats(&db.conn).map_err(|e| SubsonicError::internal(e.to_string()))?;
        Ok(b.child(
            XmlNode::new("scanStatus")
                .attr_bool("scanning", false)
                .attr_int("count", stats.total_tracks),
        ))
    })
}

async fn get_genres(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    respond_db(&state, &params, |db, b| {
        let mut stmt = db
            .conn
            .prepare(
                "SELECT genre, COUNT(*), COUNT(DISTINCT album_id)
                 FROM tracks WHERE genre IS NOT NULL AND genre != ''
                 GROUP BY genre ORDER BY genre",
            )
            .map_err(|e| SubsonicError::internal(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .map_err(|e| SubsonicError::internal(e.to_string()))?;

        let mut genres_node = XmlNode::new("genres").array_of("genre");
        for row in rows {
            let (name, song_count, album_count) =
                row.map_err(|e| SubsonicError::internal(e.to_string()))?;
            genres_node = genres_node.child(
                XmlNode::new("genre")
                    .attr_int("songCount", song_count)
                    .attr_int("albumCount", album_count)
                    // The XSD carries the name as element text; `value` is the
                    // JSON spelling of the same thing.
                    .text(&name),
            );
        }

        Ok(b.child(genres_node))
    })
}

// ---------------------------------------------------------------------------
// Playlist endpoints (mapped to koan snapshots)
// ---------------------------------------------------------------------------

async fn get_playlists(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    respond_db(&state, &params, |db, b| {
        let snaps = queries::list_snapshots(&db.conn)
            .map_err(|e| SubsonicError::internal(e.to_string()))?;

        let mut playlists_node = XmlNode::new("playlists").array_of("playlist");
        for snap in &snaps {
            playlists_node = playlists_node.child(
                XmlNode::new("playlist")
                    .attr("id", &snap.name)
                    .attr("name", &snap.name)
                    .attr_int("songCount", snap.track_count as i64)
                    .attr("owner", &state.username)
                    .attr_bool("public", false)
                    .attr("created", &snap.created_at),
            );
        }

        Ok(b.child(playlists_node))
    })
}

/// A saved snapshot as a `<playlist>` with its members resolved.
fn playlist_node(db: &Database, name: &str, owner: &str) -> Result<XmlNode, SubsonicError> {
    let snap = queries::load_snapshot(&db.conn, name)
        .map_err(|e| SubsonicError::internal(e.to_string()))?
        .ok_or_else(|| SubsonicError::not_found("Playlist"))?;

    let mut node = XmlNode::new("playlist")
        .attr("id", &snap.name)
        .attr("name", &snap.name)
        .attr_int("songCount", snap.items.len() as i64)
        .attr("owner", owner)
        .attr_bool("public", false)
        .attr("created", &snap.created_at)
        .array_of("entry");

    // Playlist members are `<entry>`, not `<song>` — an XML client shown
    // `<song>` sees an empty playlist.
    for item in &snap.items {
        if let Ok(Some(tid)) = queries::track_id_by_path(&db.conn, &item.path)
            && let Ok(Some(track)) = queries::get_track_row(&db.conn, tid)
        {
            node = node.child(track_node(&track, "entry"));
        }
    }

    Ok(node)
}

async fn get_playlist(
    State(state): State<Arc<AppState>>,
    Query(params): Query<IdParam>,
) -> Response {
    respond_db(&state, &params.auth, |db, b| {
        let name = params
            .id
            .as_deref()
            .ok_or_else(|| SubsonicError::missing_param("id"))?;
        Ok(b.child(playlist_node(db, name, &state.username)?))
    })
}

async fn create_playlist(State(state): State<Arc<AppState>>, RawQuery(raw): RawQuery) -> Response {
    let params = RawParams::parse(raw.as_deref());
    let auth = params.auth();

    respond_db(&state, &auth, |db, b| {
        let name = params
            .get("name")
            .or_else(|| params.get("playlistId"))
            .ok_or_else(|| SubsonicError::missing_param("name"))?;

        let mut items = Vec::new();
        for id_str in params.all("songId") {
            let Ok(tid) = id_str.parse::<i64>() else {
                continue;
            };
            let Some(track) = queries::get_track_row(&db.conn, tid)
                .map_err(|e| SubsonicError::internal(e.to_string()))?
            else {
                continue;
            };
            let path = track
                .path
                .as_deref()
                .or(track.cached_path.as_deref())
                .unwrap_or("");
            items.push(koan_core::db::queries::playback_state::PersistedQueueItem {
                path: path.to_string(),
                title: track.title,
                artist: track.artist_name,
                album_artist: track.album_artist_name,
                album: track.album_title,
                year: None,
                codec: track.codec,
                track_number: track.track_number.map(|n| n as i64),
                disc: track.disc.map(|n| n as i64),
                duration_ms: track.duration_ms.map(|d| d as u64),
                db_id: Some(tid),
            });
        }

        queries::save_snapshot(&db.conn, name, &items, None, 0)
            .map_err(|e| SubsonicError::internal(e.to_string()))?;

        // Since 1.14.0 the response carries the playlist that was created;
        // clients read the id back off it rather than guessing.
        Ok(b.child(playlist_node(db, name, &state.username)?))
    })
}

async fn delete_playlist(
    State(state): State<Arc<AppState>>,
    Query(params): Query<IdParam>,
) -> Response {
    respond_db(&state, &params.auth, |db, b| {
        let name = params
            .id
            .as_deref()
            .ok_or_else(|| SubsonicError::missing_param("id"))?;

        match queries::delete_snapshot(&db.conn, name) {
            Ok(true) => Ok(b),
            Ok(false) => Err(SubsonicError::not_found("Playlist")),
            Err(e) => Err(SubsonicError::internal(e.to_string())),
        }
    })
}

// ---------------------------------------------------------------------------
// Unimplemented endpoints
// ---------------------------------------------------------------------------

/// Anything under `/rest/` with no handler.
///
/// Registered as a wildcard route rather than a router fallback: this router is
/// merged into the GraphQL app, and a fallback there would swallow every 404 in
/// the process. A body-less 404 reads to a client as a broken server, and
/// several abort the connection test on one.
async fn unsupported_endpoint(UrlPath(path): UrlPath<String>, RawQuery(raw): RawQuery) -> Response {
    let params = RawParams::parse(raw.as_deref());
    let endpoint = path.trim_end_matches(".view");
    SubsonicResponse::error(
        params.auth().wants_json(),
        &SubsonicError::unsupported(endpoint),
    )
}

// ===========================================================================
// Public router
// ===========================================================================

/// Register all Subsonic REST routes on the given router.
fn register_subsonic_routes(router: axum::Router<Arc<AppState>>) -> axum::Router<Arc<AppState>> {
    router
        // Browsing (ID3)
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
        .route("/rest/getAlbumList", get(get_album_list))
        .route("/rest/getAlbumList.view", get(get_album_list))
        .route("/rest/getAlbumList2", get(get_album_list2))
        .route("/rest/getAlbumList2.view", get(get_album_list2))
        .route("/rest/getSong", get(get_song))
        .route("/rest/getSong.view", get(get_song))
        // Browsing (file tree)
        .route("/rest/getIndexes", get(get_indexes))
        .route("/rest/getIndexes.view", get(get_indexes))
        .route("/rest/getMusicDirectory", get(get_music_directory))
        .route("/rest/getMusicDirectory.view", get(get_music_directory))
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
        // Server + user metadata
        .route("/rest/getMusicFolders", get(get_music_folders))
        .route("/rest/getMusicFolders.view", get(get_music_folders))
        .route("/rest/getGenres", get(get_genres))
        .route("/rest/getGenres.view", get(get_genres))
        .route("/rest/getUser", get(get_user))
        .route("/rest/getUser.view", get(get_user))
        .route("/rest/getScanStatus", get(get_scan_status))
        .route("/rest/getScanStatus.view", get(get_scan_status))
        // Playlists (mapped to koan snapshots)
        .route("/rest/getPlaylists", get(get_playlists))
        .route("/rest/getPlaylists.view", get(get_playlists))
        .route("/rest/getPlaylist", get(get_playlist))
        .route("/rest/getPlaylist.view", get(get_playlist))
        .route("/rest/createPlaylist", get(create_playlist))
        .route("/rest/createPlaylist.view", get(create_playlist))
        .route("/rest/deletePlaylist", get(delete_playlist))
        .route("/rest/deletePlaylist.view", get(delete_playlist))
        // Everything else under /rest/
        .route("/rest/{*endpoint}", get(unsupported_endpoint))
}

/// Build a Subsonic-compatible REST API router.
///
/// Returns `None` unless `[subsonic]` is enabled and has its own credentials.
/// `/rest/*` carries no JWT layer, so these credentials alone guard every byte
/// of the library — they must never be the upstream `[remote]` password.
pub fn subsonic_router(db_path: PathBuf) -> Option<axum::Router> {
    let cfg = Config::load().unwrap_or_default();

    if !cfg.subsonic.enabled {
        return None;
    }

    if cfg.subsonic.username.is_empty() {
        log::warn!("Subsonic API disabled: subsonic.username is empty.");
        return None;
    }

    let Some(password) = koan_core::helpers::get_subsonic_password(&cfg) else {
        log::warn!(
            "Subsonic API disabled: no secret configured. Run `koan subsonic setup` to generate one."
        );
        return None;
    };

    let state = Arc::new(AppState {
        db_path,
        username: cfg.subsonic.username.clone(),
        password,
        upstream: koan_core::helpers::subsonic_auth(&cfg),
        http: reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default(),
        cover_cache: Mutex::new(LruCache::new(
            NonZeroUsize::new(COVER_CACHE_ENTRIES).unwrap(),
        )),
    });

    Some(register_subsonic_routes(axum::Router::new()).with_state(state))
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
            upstream: None,
            http: reqwest::Client::new(),
            cover_cache: Mutex::new(LruCache::new(
                NonZeroUsize::new(COVER_CACHE_ENTRIES).unwrap(),
            )),
        });
        (state, dir)
    }

    fn build_test_router(state: Arc<AppState>) -> axum::Router {
        register_subsonic_routes(axum::Router::new()).with_state(state)
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

    fn track_meta(path: &str, title: &str, album: &str, track_number: i32) -> TrackMeta {
        TrackMeta {
            title: title.into(),
            artist: "Test Artist".into(),
            album: album.into(),
            album_artist: Some("Test Artist".into()),
            track_number: Some(track_number),
            disc: Some(1),
            duration_ms: Some(240_000),
            codec: Some("FLAC".into()),
            sample_rate: Some(44100),
            bit_depth: Some(16),
            channels: Some(2),
            bitrate: Some(1411),
            genre: Some("Rock".into()),
            path: Some(path.into()),
            date: Some("2020".into()),
            label: None,
            size_bytes: None,
            mtime: None,
            source: "local".into(),
            remote_id: None,
            remote_url: None,
            album_added_at: None,
        }
    }

    fn seed_data(state: &AppState) {
        let db = Database::open(&state.db_path).unwrap();
        queries::upsert_track(
            &db.conn,
            &track_meta("/music/test.flac", "Test Song", "Test Album", 1),
        )
        .unwrap();
    }

    /// Seed a track backed by a file that actually exists, for the paths that
    /// read bytes off disk.
    fn seed_local_file(state: &AppState, dir: &std::path::Path, bytes: &[u8]) -> i64 {
        let path = dir.join("real.flac");
        std::fs::write(&path, bytes).unwrap();
        let db = Database::open(&state.db_path).unwrap();
        queries::upsert_track(
            &db.conn,
            &track_meta(path.to_str().unwrap(), "Test Song", "Test Album", 1),
        )
        .unwrap();
        queries::track_id_by_path(&db.conn, path.to_str().unwrap())
            .unwrap()
            .unwrap()
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
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    async fn get_with_range(
        app: axum::Router,
        uri: &str,
        range: &str,
    ) -> (StatusCode, HeaderMap, Vec<u8>) {
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header(header::RANGE, range)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let headers = resp.headers().clone();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, headers, body.to_vec())
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
    fn test_parse_range_full() {
        assert_eq!(
            parse_range("bytes=0-999", 5000),
            RangeRequest::Satisfiable { start: 0, end: 999 }
        );
    }

    #[test]
    fn test_parse_range_open_end() {
        assert_eq!(
            parse_range("bytes=1000-", 5000),
            RangeRequest::Satisfiable {
                start: 1000,
                end: 4999
            }
        );
    }

    #[test]
    fn test_parse_range_suffix() {
        assert_eq!(
            parse_range("bytes=-500", 5000),
            RangeRequest::Satisfiable {
                start: 4500,
                end: 4999
            }
        );
    }

    #[test]
    fn test_parse_range_out_of_bounds() {
        assert_eq!(
            parse_range("bytes=5000-6000", 5000),
            RangeRequest::Unsatisfiable
        );
    }

    #[test]
    fn test_parse_range_clamps_end() {
        assert_eq!(
            parse_range("bytes=4000-9999", 5000),
            RangeRequest::Satisfiable {
                start: 4000,
                end: 4999
            }
        );
    }

    #[test]
    fn test_parse_range_on_empty_file() {
        assert_eq!(parse_range("bytes=0-", 0), RangeRequest::Unsatisfiable);
        assert_eq!(parse_range("bytes=-100", 0), RangeRequest::Unsatisfiable);
    }

    #[test]
    fn test_parse_range_malformed_is_ignored() {
        assert_eq!(parse_range("seconds=0-10", 5000), RangeRequest::Malformed);
        assert_eq!(parse_range("bytes=abc-def", 5000), RangeRequest::Malformed);
        // Multipart ranges are unsupported; serving the whole body is legal.
        assert_eq!(parse_range("bytes=0-1,5-6", 5000), RangeRequest::Malformed);
    }

    #[test]
    fn test_parse_entity_id() {
        assert_eq!(parse_entity_id("5"), Some((None, 5)));
        assert_eq!(parse_entity_id("mf-5"), Some((Some(EntityKind::Song), 5)));
        assert_eq!(parse_entity_id("al-5"), Some((Some(EntityKind::Album), 5)));
        assert_eq!(parse_entity_id("ar-5"), Some((Some(EntityKind::Artist), 5)));
        assert_eq!(parse_entity_id("not-an-id"), None);
    }

    #[test]
    fn test_raw_params_repeated_keys() {
        let p = RawParams::parse(Some("name=mix&songId=1&songId=2&songId%5B%5D=3"));
        assert_eq!(p.get("name"), Some("mix"));
        assert_eq!(p.all("songId").collect::<Vec<_>>(), vec!["1", "2", "3"]);
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
    async fn test_legacy_password_auth_rejected() {
        let (state, _dir) = test_state();
        let app = build_test_router(state);
        let (_, body) = get_response(app, "/rest/ping?u=testuser&p=testpass&v=1.16.1&c=test").await;
        assert!(body.contains("status=\"failed\""));
        assert!(body.contains("code=\"40\""));
    }

    #[tokio::test]
    async fn test_enc_hex_password_auth_rejected() {
        let (state, _dir) = test_state();
        let app = build_test_router(state);
        let (_, body) = get_response(
            app,
            "/rest/ping?u=testuser&p=enc:7465737470617373&v=1.16.1&c=test",
        )
        .await;
        assert!(body.contains("status=\"failed\""));
    }

    #[tokio::test]
    async fn test_cover_art_size_is_clamped() {
        // A 1x1 PNG upscaled to 65535x65535 would ask for ~17GB and abort the
        // process; the clamp keeps the request bounded.
        let png = image::RgbImage::from_pixel(1, 1, image::Rgb([1, 2, 3]));
        let mut src = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(png)
            .write_to(&mut src, image::ImageFormat::Png)
            .unwrap();

        let out = resize_image(&src.into_inner(), u32::MAX, true).unwrap();
        let decoded = image::load_from_memory(&out).unwrap();
        use image::GenericImageView as _;
        assert_eq!(decoded.dimensions(), (1, 1));
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

    /// The typed-attribute rewrite must not change the XML wire format: every
    /// attribute is still a quoted string, spelled exactly as before.
    #[tokio::test]
    async fn test_song_xml_is_unchanged_by_typed_attributes() {
        let (state, _dir) = test_state();
        seed_data(&state);

        let db = Database::open(&state.db_path).unwrap();
        let track_id = queries::all_tracks(&db.conn).unwrap()[0].id;

        let app = build_test_router(state);
        let (_, body) = get_response(
            app,
            &format!("/rest/getSong?{}&id={}", auth_query(""), track_id),
        )
        .await;

        let song = body
            .lines()
            .find(|l| l.trim_start().starts_with("<song "))
            .expect("no <song> element")
            .trim()
            .to_string();
        assert_eq!(
            song,
            format!(
                concat!(
                    r#"<song id="{id}" title="Test Song" album="Test Album" artist="Test Artist" "#,
                    r#"track="1" discNumber="1" duration="240" bitRate="1411" suffix="flac" "#,
                    r#"contentType="audio/flac" genre="Rock" albumId="1" artistId="1" "#,
                    r#"parent="al-1" coverArt="mf-{id}" type="music" isDir="false"/>"#
                ),
                id = track_id
            )
        );
    }

    /// The failure that stopped Symfonium, Substreamer and Feishin dead: a
    /// strictly-typed deserialiser rejects `"duration": "240"`.
    #[tokio::test]
    async fn test_song_json_field_types() {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct StrictSong {
            id: String,
            title: String,
            duration: i64,
            track: i64,
            bit_rate: i64,
            disc_number: i64,
            is_dir: bool,
            #[serde(rename = "type")]
            kind: String,
            cover_art: String,
        }

        let (state, _dir) = test_state();
        seed_data(&state);

        let db = Database::open(&state.db_path).unwrap();
        let track_id = queries::all_tracks(&db.conn).unwrap()[0].id;

        let app = build_test_router(state);
        let (_, body) = get_response(
            app,
            &format!("/rest/getSong?{}&id={}", auth_query("f=json"), track_id),
        )
        .await;

        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        let song: StrictSong =
            serde_json::from_value(parsed["subsonic-response"]["song"].clone()).unwrap();

        assert_eq!(song.id, track_id.to_string());
        assert_eq!(song.title, "Test Song");
        assert_eq!(song.duration, 240);
        assert_eq!(song.track, 1);
        assert_eq!(song.bit_rate, 1411);
        assert_eq!(song.disc_number, 1);
        assert!(!song.is_dir);
        assert_eq!(song.kind, "music");
        assert_eq!(song.cover_art, format!("mf-{}", track_id));
    }

    #[tokio::test]
    async fn test_album_json_field_types() {
        let (state, _dir) = test_state();
        seed_data(&state);

        let db = Database::open(&state.db_path).unwrap();
        let album_id = queries::all_albums(&db.conn).unwrap()[0].id;

        let app = build_test_router(state);
        let (_, body) = get_response(
            app,
            &format!("/rest/getAlbum?{}&id={}", auth_query("f=json"), album_id),
        )
        .await;

        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        let album = &parsed["subsonic-response"]["album"];
        assert_eq!(album["songCount"], serde_json::json!(1));
        assert_eq!(album["year"], serde_json::json!(2020));
        assert_eq!(album["isDir"], serde_json::json!(true));
        assert_eq!(album["coverArt"], format!("al-{}", album_id));
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
    async fn test_get_album_list_v1() {
        let (state, _dir) = test_state();
        seed_data(&state);

        let app = build_test_router(state);
        let (_, body) = get_response(
            app,
            &format!("/rest/getAlbumList?{}&type=newest", auth_query("")),
        )
        .await;
        assert!(body.contains("<albumList"));
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

    /// A malformed id used to reach axum's `Query` extractor and come back as a
    /// bare HTTP 400 with a plain-text body — unparseable by any client.
    #[tokio::test]
    async fn test_bad_id_is_a_subsonic_error() {
        let (state, _dir) = test_state();
        let app = build_test_router(state);
        let (status, body) = get_response(
            app,
            &format!("/rest/stream?{}&id=not-a-number", auth_query("")),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("status=\"failed\""));
        assert!(body.contains("code=\"10\""));
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

    #[tokio::test]
    async fn test_unknown_endpoint_is_subsonic_error_70() {
        let (state, _dir) = test_state();
        let app = build_test_router(state.clone());
        let (status, body) =
            get_response(app, &format!("/rest/getPodcasts?{}", auth_query(""))).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("status=\"failed\""));
        assert!(body.contains("code=\"70\""));
        assert!(body.contains("getPodcasts"));

        let app = build_test_router(state);
        let (_, body) = get_response(
            app,
            &format!("/rest/getBookmarks.view?{}", auth_query("f=json")),
        )
        .await;
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["subsonic-response"]["error"]["code"], 70);
    }

    // --- New endpoint tests ---

    #[tokio::test]
    async fn test_get_music_folders() {
        let (state, _dir) = test_state();
        let app = build_test_router(state);
        let (_, body) =
            get_response(app, &format!("/rest/getMusicFolders?{}", auth_query(""))).await;
        assert!(body.contains("musicFolder"));
        assert!(body.contains("Music"));
    }

    #[tokio::test]
    async fn test_get_user_and_scan_status() {
        let (state, _dir) = test_state();
        seed_data(&state);

        let app = build_test_router(state.clone());
        let (_, body) = get_response(app, &format!("/rest/getUser?{}", auth_query(""))).await;
        assert!(body.contains("username=\"testuser\""));
        assert!(body.contains("streamRole=\"true\""));

        let app = build_test_router(state);
        let (_, body) = get_response(app, &format!("/rest/getScanStatus?{}", auth_query(""))).await;
        assert!(body.contains("scanning=\"false\""));
        assert!(body.contains("count=\"1\""));
    }

    #[tokio::test]
    async fn test_get_indexes_and_music_directory() {
        let (state, _dir) = test_state();
        seed_data(&state);

        let db = Database::open(&state.db_path).unwrap();
        let artist_id = queries::all_artists(&db.conn).unwrap()[0].id;
        let album_id = queries::all_albums(&db.conn).unwrap()[0].id;

        let app = build_test_router(state.clone());
        let (_, body) = get_response(app, &format!("/rest/getIndexes?{}", auth_query(""))).await;
        assert!(body.contains("<indexes"));
        assert!(body.contains(&format!("id=\"ar-{}\"", artist_id)));

        // Artist directory lists albums.
        let app = build_test_router(state.clone());
        let (_, body) = get_response(
            app,
            &format!(
                "/rest/getMusicDirectory?{}&id=ar-{}",
                auth_query(""),
                artist_id
            ),
        )
        .await;
        assert!(body.contains(&format!("id=\"al-{}\"", album_id)));
        assert!(body.contains("isDir=\"true\""));

        // Album directory lists songs.
        let app = build_test_router(state);
        let (_, body) = get_response(
            app,
            &format!(
                "/rest/getMusicDirectory?{}&id=al-{}",
                auth_query(""),
                album_id
            ),
        )
        .await;
        assert!(body.contains("Test Song"));
        assert!(body.contains("<child"));
    }

    #[tokio::test]
    async fn test_get_genres_xml_carries_name_as_text() {
        let (state, _dir) = test_state();
        seed_data(&state);

        let app = build_test_router(state.clone());
        let (_, body) = get_response(app, &format!("/rest/getGenres?{}", auth_query(""))).await;
        assert!(
            body.contains(">Rock</genre>"),
            "genre name must be element text: {}",
            body
        );

        // JSON spells the same value as a `value` member.
        let app = build_test_router(state);
        let (_, body) =
            get_response(app, &format!("/rest/getGenres?{}", auth_query("f=json"))).await;
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        let genre = &parsed["subsonic-response"]["genres"]["genre"][0];
        assert_eq!(genre["value"], "Rock");
        assert_eq!(genre["songCount"], serde_json::json!(1));
    }

    #[tokio::test]
    async fn test_search3() {
        let (state, _dir) = test_state();
        seed_data(&state);
        let app = build_test_router(state);
        let (_, body) =
            get_response(app, &format!("/rest/search3?{}&query=Test", auth_query(""))).await;
        assert!(body.contains("Test Song"));
        assert!(body.contains("Test Artist"));
    }

    #[tokio::test]
    async fn test_star_and_get_starred() {
        let (state, _dir) = test_state();
        seed_data(&state);

        let db = Database::open(&state.db_path).unwrap();
        let tracks = queries::all_tracks(&db.conn).unwrap();
        let track_id = tracks[0].id;

        let app = build_test_router(state.clone());
        let (_, body) = get_response(
            app,
            &format!("/rest/star?{}&id={}", auth_query(""), track_id),
        )
        .await;
        assert!(body.contains("status=\"ok\""));

        let app = build_test_router(state);
        let (_, body) = get_response(app, &format!("/rest/getStarred2?{}", auth_query(""))).await;
        assert!(body.contains("Test Song"));
    }

    #[tokio::test]
    async fn test_create_playlist_with_songs() {
        let (state, _dir) = test_state();
        seed_data(&state);

        let db = Database::open(&state.db_path).unwrap();
        let track_id = queries::all_tracks(&db.conn).unwrap()[0].id;

        // Two `songId` values — the shape every client sends, and the one
        // `serde_urlencoded` turned into a bare HTTP 400.
        let app = build_test_router(state.clone());
        let (status, body) = get_response(
            app,
            &format!(
                "/rest/createPlaylist?{}&name=testmix&songId={}&songId={}",
                auth_query(""),
                track_id,
                track_id
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("status=\"ok\""), "createPlaylist: {}", body);
        // The response carries the playlist itself, per 1.14.0.
        assert!(body.contains("<playlist id=\"testmix\""), "{}", body);

        let app = build_test_router(state);
        let (_, body) = get_response(
            app,
            &format!("/rest/getPlaylist?{}&id=testmix", auth_query("")),
        )
        .await;
        assert!(body.contains("songCount=\"2\""), "{}", body);
        // Playlist members are `<entry>`, never `<song>`.
        assert_eq!(body.matches("<entry ").count(), 2, "{}", body);
        assert!(!body.contains("<song "));
    }

    #[tokio::test]
    async fn test_playlists_crud() {
        let (state, _dir) = test_state();
        seed_data(&state);

        let app = build_test_router(state.clone());
        let (_, body) = get_response(
            app,
            &format!("/rest/createPlaylist?{}&name=testmix", auth_query("")),
        )
        .await;
        assert!(
            body.contains("status=\"ok\""),
            "createPlaylist failed: {}",
            body
        );

        // List
        let app = build_test_router(state.clone());
        let (_, body) = get_response(app, &format!("/rest/getPlaylists?{}", auth_query(""))).await;
        assert!(body.contains("testmix"));

        // Get
        let app = build_test_router(state.clone());
        let (_, body) = get_response(
            app,
            &format!("/rest/getPlaylist?{}&id=testmix", auth_query("")),
        )
        .await;
        assert!(body.contains("testmix"));

        // Delete
        let app = build_test_router(state.clone());
        let (_, body) = get_response(
            app,
            &format!("/rest/deletePlaylist?{}&id=testmix", auth_query("")),
        )
        .await;
        assert!(body.contains("status=\"ok\""));

        // Verify deleted
        let app = build_test_router(state);
        let (_, body) = get_response(
            app,
            &format!("/rest/getPlaylist?{}&id=testmix", auth_query("")),
        )
        .await;
        assert!(body.contains("status=\"failed\""));
    }

    #[tokio::test]
    async fn test_scrobble() {
        let (state, _dir) = test_state();
        seed_data(&state);

        let db = Database::open(&state.db_path).unwrap();
        let tracks = queries::all_tracks(&db.conn).unwrap();

        let app = build_test_router(state);
        let (_, body) = get_response(
            app,
            &format!("/rest/scrobble?{}&id={}", auth_query(""), tracks[0].id),
        )
        .await;
        assert!(body.contains("status=\"ok\""));
    }

    #[tokio::test]
    async fn test_get_random_songs() {
        let (state, _dir) = test_state();
        seed_data(&state);
        let app = build_test_router(state);
        let (_, body) = get_response(
            app,
            &format!("/rest/getRandomSongs?{}&size=5", auth_query("")),
        )
        .await;
        assert!(body.contains("randomSongs"));
    }

    // --- Streaming ---

    #[tokio::test]
    async fn test_stream_partial_content() {
        let (state, dir) = test_state();
        let track_id = seed_local_file(&state, dir.path(), &[0u8; 1000]);

        let app = build_test_router(state);
        let (status, headers, body) = get_with_range(
            app,
            &format!("/rest/stream?{}&id={}", auth_query(""), track_id),
            "bytes=100-199",
        )
        .await;

        assert_eq!(status, StatusCode::PARTIAL_CONTENT);
        assert_eq!(headers[header::CONTENT_RANGE], "bytes 100-199/1000");
        assert_eq!(body.len(), 100);
    }

    #[tokio::test]
    async fn test_stream_unsatisfiable_range_is_416() {
        let (state, dir) = test_state();
        let track_id = seed_local_file(&state, dir.path(), &[0u8; 1000]);

        let app = build_test_router(state);
        let (status, headers, body) = get_with_range(
            app,
            &format!("/rest/stream?{}&id={}", auth_query(""), track_id),
            "bytes=5000-6000",
        )
        .await;

        assert_eq!(status, StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(headers[header::CONTENT_RANGE], "bytes */1000");
        assert!(body.is_empty());
    }

    #[tokio::test]
    async fn test_stream_malformed_range_serves_whole_body() {
        let (state, dir) = test_state();
        let track_id = seed_local_file(&state, dir.path(), &[0u8; 1000]);

        let app = build_test_router(state);
        let (status, _, body) = get_with_range(
            app,
            &format!("/rest/stream?{}&id={}", auth_query(""), track_id),
            "seconds=0-10",
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.len(), 1000);
    }

    // --- Cover art ---

    /// `getCoverArt` resolved every id as a track id, so `id=5` meaning
    /// "album 5" served track 5's art. Seeded so the two number spaces cross:
    /// album 2 holds track 3, so `al-2` and `mf-2` must land on different files.
    #[test]
    fn test_cover_art_id_namespacing() {
        let (state, dir) = test_state();
        let db = Database::open(&state.db_path).unwrap();
        for (n, album) in [(1, "Album One"), (2, "Album One"), (3, "Album Two")] {
            let path = dir.path().join(format!("t{}.flac", n));
            queries::upsert_track(
                &db.conn,
                &track_meta(path.to_str().unwrap(), &format!("Song {}", n), album, n),
            )
            .unwrap();
        }

        let track3 =
            queries::track_id_by_path(&db.conn, dir.path().join("t3.flac").to_str().unwrap())
                .unwrap()
                .unwrap();
        let track2 =
            queries::track_id_by_path(&db.conn, dir.path().join("t2.flac").to_str().unwrap())
                .unwrap()
                .unwrap();
        let album_two = queries::get_track_row(&db.conn, track3)
            .unwrap()
            .unwrap()
            .album_id
            .unwrap();
        assert_eq!(album_two, track2, "test needs the id spaces to overlap");

        // `al-2` is Album Two — track 3's file, not track 2's.
        assert_eq!(
            cover_source_path(&db, Some(EntityKind::Album), album_two).unwrap(),
            dir.path().join("t3.flac")
        );
        // `mf-2` and a bare `2` are both track 2.
        assert_eq!(
            cover_source_path(&db, Some(EntityKind::Song), track2).unwrap(),
            dir.path().join("t2.flac")
        );
        assert_eq!(
            cover_source_path(&db, None, track2).unwrap(),
            dir.path().join("t2.flac")
        );
        // An artist resolves through their first album's first track.
        let artist_id = queries::all_artists(&db.conn).unwrap()[0].id;
        assert!(
            cover_source_path(&db, Some(EntityKind::Artist), artist_id)
                .unwrap()
                .starts_with(dir.path())
        );
    }

    #[tokio::test]
    async fn test_cover_art_missing_album_is_error_70() {
        let (state, _dir) = test_state();
        let app = build_test_router(state);
        let (status, body) = get_response(
            app,
            &format!("/rest/getCoverArt?{}&id=al-9999", auth_query("")),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("code=\"70\""));
    }
}
