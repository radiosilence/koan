//! Subsonic-compatible REST API layer.
//!
//! Implements a subset of the Subsonic/OpenSubsonic REST API backed by the
//! local koan database.  Supports both XML (default) and JSON (`f=json`)
//! responses.  Clients sign in with a koan account, or with the dedicated
//! `[subsonic]` secret — see `validate_auth`.

use std::collections::{BTreeMap, HashMap};
use std::io::Cursor;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Path as UrlPath, Query, RawQuery, Request, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use koan_core::auth::Role;
use koan_core::config::Config;
use koan_core::db::connection::Database;
use koan_core::db::pool::{Handle, Pool};
use koan_core::db::queries;
use koan_core::index::metadata::extract_cover_art;
use koan_core::remote::client::SubsonicAuth;
use lru::LruCache;
use serde::Deserialize;
use tokio::io::AsyncReadExt as _;

const SUBSONIC_API_VERSION: &str = "1.16.1";
const SUBSONIC_XMLNS: &str = "http://subsonic.org/restapi";
/// The OpenSubsonic `type`: which server this is, as opposed to which protocol.
const SERVER_TYPE: &str = "koan";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Where an authentication error sends a client's user: the page explaining
/// accounts, passwords and API keys.
const AUTH_HELP_URL: &str =
    "https://github.com/radiosilence/koan/blob/main/docs/guide/authentication.md#subsonic-api";

/// Largest form body a POST may carry. `createPlaylist` repeats `songId` once
/// per track, so this is sized for playlists thousands long.
const MAX_FORM_BODY: usize = 1 << 20;

/// The OpenSubsonic extensions this server implements, and their versions.
const EXTENSIONS: &[(&str, &[i64])] = &[
    ("apiKeyAuthentication", &[1]),
    ("formPost", &[1]),
    ("songLyrics", &[1]),
];
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
    pool: Arc<Pool>,
    username: String,
    /// The `[subsonic]` shared secret; without one, only accounts sign in.
    password: Option<String>,
    users: crate::auth::password::PasswordVerifier,
    /// Upstream Navidrome/Subsonic, used to build signed stream URLs for tracks
    /// with no local file. Resolved once at startup — resolving it per request
    /// re-read two TOML files. Credentials
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
    fn open_db(&self) -> Result<Handle<'_>, SubsonicError> {
        self.pool
            .get()
            .map_err(|e| SubsonicError::from(e.to_string()))
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
    TokenAuthUnsupported = 41,
    ConflictingAuth = 43,
    InvalidApiKey = 44,
    NotAuthorized = 50,
    NotFound = 70,
}

#[derive(Debug)]
struct SubsonicError {
    code: SubsonicErrorCode,
    message: String,
    help_url: Option<&'static str>,
}

impl SubsonicError {
    fn new(code: SubsonicErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            help_url: None,
        }
    }

    fn auth(code: SubsonicErrorCode, message: &str) -> Self {
        Self {
            help_url: Some(AUTH_HELP_URL),
            ..Self::new(code, message)
        }
    }

    fn wrong_auth() -> Self {
        Self::new(SubsonicErrorCode::WrongAuth, "Wrong username or password")
    }

    fn token_auth_unsupported() -> Self {
        Self::auth(
            SubsonicErrorCode::TokenAuthUnsupported,
            "Token authentication needs this account to have signed in by password once (the web UI, or p=); until then use a password or an API key",
        )
    }

    fn conflicting_auth() -> Self {
        Self::auth(
            SubsonicErrorCode::ConflictingAuth,
            "Multiple conflicting authentication mechanisms provided",
        )
    }

    fn invalid_api_key() -> Self {
        Self::auth(SubsonicErrorCode::InvalidApiKey, "Invalid API key")
    }

    fn not_authorized() -> Self {
        Self::new(
            SubsonicErrorCode::NotAuthorized,
            "User is not authorized for the given operation",
        )
    }

    fn missing_param(name: &str) -> Self {
        Self::new(
            SubsonicErrorCode::MissingParameter,
            format!("Required parameter '{}' is missing", name),
        )
    }

    fn bad_param(name: &str) -> Self {
        Self::new(
            SubsonicErrorCode::MissingParameter,
            format!("Invalid value for parameter '{}'", name),
        )
    }

    fn not_found(what: &str) -> Self {
        Self::new(SubsonicErrorCode::NotFound, format!("{} not found", what))
    }

    fn unsupported(endpoint: &str) -> Self {
        Self::new(
            SubsonicErrorCode::NotFound,
            format!("Endpoint '{}' is not supported by this server", endpoint),
        )
    }

    fn internal(msg: impl Into<String>) -> Self {
        Self::new(SubsonicErrorCode::Generic, msg)
    }
}

impl From<String> for SubsonicError {
    fn from(s: String) -> Self {
        Self::new(SubsonicErrorCode::Generic, s)
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
    #[serde(rename = "apiKey")]
    api_key: Option<String>,
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
            api_key: self.get("apiKey").map(String::from),
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
            status: "ok",
            root: XmlNode::new("subsonic-response"),
        }
    }

    fn error(json: bool, err: &SubsonicError) -> Response {
        let error = XmlNode::new("error")
            .attr_int("code", err.code as i64)
            .attr("message", &err.message)
            .attr_opt("helpUrl", err.help_url);
        XmlBuilder {
            json,
            status: "failed",
            root: XmlNode::new("subsonic-response").child(error),
        }
        .build()
    }
}

// ---------------------------------------------------------------------------
// Lightweight XML/JSON builder
// ---------------------------------------------------------------------------

/// A response document: the `subsonic-response` envelope and what hangs off it.
struct XmlBuilder {
    json: bool,
    status: &'static str,
    root: XmlNode,
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
    /// A bare value, for arrays of primitives: `<versions>1</versions>` in XML,
    /// the element of `"versions": [1]` in JSON.
    scalar: Option<AttrValue>,
    children: Vec<XmlNode>,
    /// Child tags that are arrays in JSON whatever their count, empty
    /// included. OpenSubsonic requires a supported list field to be present
    /// as `[]` rather than absent, and a lone member would otherwise collapse
    /// into an object.
    array_tags: Vec<String>,
}

impl XmlNode {
    fn new(tag: &str) -> Self {
        Self {
            tag: tag.into(),
            attrs: Vec::new(),
            text: None,
            scalar: None,
            children: Vec::new(),
            array_tags: Vec::new(),
        }
    }

    fn scalar(tag: &str, value: AttrValue) -> Self {
        Self {
            scalar: Some(value),
            ..Self::new(tag)
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
        self.array_tags.push(child_tag.into());
        self
    }

    /// Children under `tag`, always an array in JSON.
    fn list(self, tag: &str, nodes: impl IntoIterator<Item = XmlNode>) -> Self {
        nodes
            .into_iter()
            .fold(self.array_of(tag), |node, child| node.child(child))
    }

    fn to_xml(&self, indent: usize) -> String {
        let pad = "  ".repeat(indent);
        let mut s = format!("<{}", self.tag);
        for (k, v) in &self.attrs {
            s.push_str(&format!(" {}=\"{}\"", k, xml_escape(&v.to_xml_text())));
        }
        if self.children.is_empty() {
            let text = self
                .text
                .clone()
                .or_else(|| self.scalar.as_ref().map(AttrValue::to_xml_text));
            return match text {
                Some(t) => format!("{}{}>{}</{}>", pad, s, xml_escape(&t), self.tag),
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
        if let Some(value) = &self.scalar {
            return value.to_json();
        }
        let mut obj = serde_json::Map::new();
        for (k, v) in &self.attrs {
            obj.insert(k.clone(), v.to_json());
        }
        if let Some(text) = &self.text {
            obj.insert("value".into(), serde_json::Value::String(text.clone()));
        }
        let mut groups: BTreeMap<&str, Vec<serde_json::Value>> = self
            .array_tags
            .iter()
            .map(|tag| (tag.as_str(), Vec::new()))
            .collect();
        for child in &self.children {
            groups
                .entry(&child.tag)
                .or_default()
                .push(child.to_json_value());
        }
        for (tag, mut values) in groups {
            let value = if values.len() == 1 && !self.array_tags.iter().any(|t| t == tag) {
                values.pop().unwrap()
            } else {
                serde_json::Value::Array(values)
            };
            obj.insert(tag.into(), value);
        }
        serde_json::Value::Object(obj)
    }
}

impl XmlBuilder {
    fn child(mut self, node: XmlNode) -> Self {
        self.root = self.root.child(node);
        self
    }

    fn list(mut self, tag: &str, nodes: impl IntoIterator<Item = XmlNode>) -> Self {
        self.root = self.root.list(tag, nodes);
        self
    }

    /// Render with the envelope every response carries, success or failure:
    /// the Subsonic `status` and `version`, and OpenSubsonic's `type`,
    /// `serverVersion` and `openSubsonic`.
    fn build(self) -> Response {
        let mut root = self.root;
        root.attrs.splice(
            0..0,
            [
                ("status".into(), AttrValue::Str(self.status.into())),
                (
                    "version".into(),
                    AttrValue::Str(SUBSONIC_API_VERSION.into()),
                ),
                ("type".into(), AttrValue::Str(SERVER_TYPE.into())),
                (
                    "serverVersion".into(),
                    AttrValue::Str(SERVER_VERSION.into()),
                ),
                ("openSubsonic".into(), AttrValue::Bool(true)),
            ],
        );
        if self.json {
            let wrapper = serde_json::json!({ "subsonic-response": root.to_json_value() });
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
                serde_json::to_string(&wrapper).unwrap(),
            )
                .into_response()
        } else {
            root.attrs
                .insert(0, ("xmlns".into(), AttrValue::Str(SUBSONIC_XMLNS.into())));
            let xml = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n{}",
                root.to_xml(0)
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

/// Who a request acts as.
struct Caller {
    username: String,
    role: Role,
}

/// Authenticate a request.
///
/// Three ways in:
/// - `apiKey=`, a key made for a koan account (`koan auth api-key create`, or
///   the web UI's keys page). It names its user, so `u` alongside it is a
///   conflict (43), as is any other credential.
/// - `p=` (plain or `enc:` hex) with a koan account's password, checked
///   against its argon2 hash. The protocol sends it with every request, so it
///   is only as private as the transport; koan.blit.cc is HTTPS-only, and this
///   is what the web login form sends too.
/// - `t=md5(secret + s)` with the `[subsonic]` shared secret, for clients that
///   only speak token auth. Token auth cannot work against a hash, so for any
///   other username it is refused with 41, the code that tells a client to
///   fall back to a password or a key. The secret acts as `User`, and is also
///   accepted as `p=`.
fn validate_auth(params: &SubsonicParams, state: &AppState) -> Result<Caller, SubsonicError> {
    use subtle::ConstantTimeEq;

    if let Some(key) = params.api_key.as_deref() {
        if params.u.is_some() || params.p.is_some() || params.t.is_some() || params.s.is_some() {
            return Err(SubsonicError::conflicting_auth());
        }
        let db = state.open_db()?;
        let user = queries::api_keys::authenticate_api_key(&db.conn, key)
            .map_err(|e| SubsonicError::internal(e.to_string()))?
            .ok_or_else(SubsonicError::invalid_api_key)?;
        return Ok(Caller {
            username: user.username,
            role: user.role,
        });
    }

    let username = params
        .u
        .as_deref()
        .ok_or_else(|| SubsonicError::missing_param("u"))?;
    let caller = |role| {
        Ok(Caller {
            username: username.to_owned(),
            role,
        })
    };
    let shared = |given: &str| {
        let user_ok = username.as_bytes().ct_eq(state.username.as_bytes());
        state
            .password
            .as_deref()
            .is_some_and(|p| bool::from(user_ok & given.as_bytes().ct_eq(p.as_bytes())))
    };

    if let (Some(token), Some(salt)) = (params.t.as_deref(), params.s.as_deref()) {
        if let Some(secret) = state
            .password
            .as_deref()
            .filter(|_| username == state.username)
        {
            let expected = format!("{:x}", md5::compute(format!("{secret}{salt}")));
            return if bool::from(token.as_bytes().ct_eq(expected.as_bytes())) {
                caller(Role::User)
            } else {
                Err(SubsonicError::wrong_auth())
            };
        }
        // An account: checked against its sealed password. Without one (never
        // signed in by password since this existed) token auth can't be
        // checked, which 41 tells the client so it can fall back.
        return match state.users.verify_token(username, token, salt) {
            Some(role) => caller(role),
            None if state.users.has_sealed(username) => Err(SubsonicError::wrong_auth()),
            None => Err(SubsonicError::token_auth_unsupported()),
        };
    }

    let Some(p) = params.p.as_deref() else {
        return Err(SubsonicError::missing_param("t and s"));
    };
    let password = match p.strip_prefix("enc:") {
        Some(hex) => decode_hex(hex).ok_or_else(SubsonicError::wrong_auth)?,
        None => p.to_string(),
    };
    if shared(&password) {
        return caller(Role::User);
    }
    let role = state
        .users
        .verify(username, &password)
        .ok_or_else(SubsonicError::wrong_auth)?;
    caller(role)
}

fn decode_hex(hex: &str) -> Option<String> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    let bytes = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect::<Option<Vec<u8>>>()?;
    String::from_utf8(bytes).ok()
}

/// Refuse a readonly account the endpoints that change anything.
fn require_write(role: Role) -> Result<(), SubsonicError> {
    if role.has_permission(Role::User) {
        Ok(())
    } else {
        Err(SubsonicError::not_authorized())
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
    respond_db_as(state, auth, Role::Readonly, f)
}

/// As `respond_db`, for an endpoint needing at least `need`.
fn respond_db_as(
    state: &AppState,
    auth: &SubsonicParams,
    need: Role,
    f: impl FnOnce(&Database, XmlBuilder) -> Result<XmlBuilder, SubsonicError>,
) -> Response {
    let json = auth.wants_json();
    let result = validate_auth(auth, state)
        .and_then(|caller| {
            if need == Role::Readonly {
                Ok(())
            } else {
                require_write(caller.role)
            }
        })
        .and_then(|()| state.open_db())
        .and_then(|db| f(&db, SubsonicResponse::ok(json)));
    match result {
        Ok(builder) => builder.build(),
        Err(e) => SubsonicResponse::error(json, &e),
    }
}

/// As `respond_db`, for endpoints that never touch the database. They are
/// handed who is asking.
fn respond(
    state: &AppState,
    auth: &SubsonicParams,
    f: impl FnOnce(&Caller, XmlBuilder) -> Result<XmlBuilder, SubsonicError>,
) -> Response {
    let json = auth.wants_json();
    match validate_auth(auth, state).and_then(|caller| f(&caller, SubsonicResponse::ok(json))) {
        Ok(builder) => builder.build(),
        Err(e) => SubsonicResponse::error(json, &e),
    }
}

/// Prologue for the two endpoints that answer with bytes rather than a
/// document, and so cannot go through `respond_db`.
fn authed_db<'a>(state: &'a AppState, auth: &SubsonicParams) -> Result<Handle<'a>, SubsonicError> {
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
/// The OpenSubsonic fields koan has data for are always present, empty when a
/// track has no value, so a client can tell a supported field from a missing
/// one; `played` is the exception, absent until the track has been played.
fn track_node(track: &queries::TrackRow, tag: &str, extras: &SongExtras) -> XmlNode {
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
        .attr("mediaType", "song")
        .attr_int("bitDepth", track.bit_depth.unwrap_or(0).into())
        .attr_int("samplingRate", track.sample_rate.unwrap_or(0).into())
        .attr_int("channelCount", track.channels.unwrap_or(0).into())
        .attr("displayArtist", &track.artist_name)
        .attr("displayAlbumArtist", &track.album_artist_name)
        .attr(
            "musicBrainzId",
            extras.mbid.get(&track.id).map_or("", String::as_str),
        )
        .attr_opt(
            "played",
            extras.played.get(&track.id).map(|&at| iso(at)).as_deref(),
        )
        .list("genres", track.genre.iter().map(|g| genre_node(g)))
        .list(
            "artists",
            track
                .artist_id
                .map(|id| artist_ref("artists", id, &track.artist_name)),
        )
}

fn track_to_xml_node(track: &queries::TrackRow, extras: &SongExtras) -> XmlNode {
    track_node(track, "song", extras)
}

fn genre_node(name: &str) -> XmlNode {
    XmlNode::new("genres").attr("name", name)
}

/// An `ArtistID3` inside a list field, with only its required fields.
fn artist_ref(tag: &str, id: i64, name: &str) -> XmlNode {
    XmlNode::new(tag)
        .attr("id", &id.to_string())
        .attr("name", name)
}

fn year_from_date(date: Option<&str>) -> Option<i64> {
    date.and_then(|d| d.get(..4)).and_then(|y| y.parse().ok())
}

/// An OpenSubsonic `ItemDate` from a tag date: `2020`, `2020-05` or
/// `2020-05-17`, each part given only when it and those before it parse.
fn item_date(tag: &str, date: Option<&str>) -> XmlNode {
    let mut node = XmlNode::new(tag);
    let date = date.unwrap_or_default();
    let fields = ["year", "month", "day"]
        .into_iter()
        .zip([1..=9999, 1..=12, 1..=31]);
    for ((key, range), part) in fields.zip(date.get(..10).unwrap_or(date).split('-')) {
        match part.parse::<i64>() {
            Ok(n) if range.contains(&n) => node = node.attr_int(key, n),
            _ => break,
        }
    }
    node
}

/// An album as an `AlbumID3` element. `title` rides alongside `name` because
/// the file-browse half of the protocol spells it that way and clients mix the
/// two freely. koan keeps one date per album, a tag's release or recording
/// date, and gives it as `releaseDate`.
fn album_to_xml_node(album: &queries::AlbumRow, extras: &AlbumExtras) -> XmlNode {
    let stats = extras.stats.get(&album.id).copied().unwrap_or_default();
    let (mbid, sort_name) = extras
        .names
        .get(&album.id)
        .map(|(m, s)| (m.as_deref(), s.as_deref()))
        .unwrap_or_default();
    let genres = extras
        .genres
        .get(&album.id)
        .map(Vec::as_slice)
        .unwrap_or_default();
    XmlNode::new("album")
        .attr("id", &album.id.to_string())
        .attr("name", &album.title)
        .attr("title", &album.title)
        .attr("artist", &album.artist_name)
        .attr("artistId", &album.artist_id.to_string())
        .attr("parent", &format!("{}{}", ARTIST_PREFIX, album.artist_id))
        .attr("coverArt", &format!("{}{}", ALBUM_PREFIX, album.id))
        .attr_int("songCount", stats.track_count)
        .attr_int("duration", stats.total_duration_ms / 1000)
        .attr_opt("created", album.added_at.as_deref())
        .attr_opt_int("year", year_from_date(album.date.as_deref()))
        .attr_opt("genre", genres.first().map(String::as_str))
        .attr_bool("isDir", true)
        .attr("musicBrainzId", mbid.unwrap_or_default())
        .attr("sortName", sort_name.unwrap_or_default())
        .attr("displayArtist", &album.artist_name)
        .attr_opt(
            "played",
            extras.played.get(&album.id).map(|&at| iso(at)).as_deref(),
        )
        .child(item_date("releaseDate", album.date.as_deref()))
        .list("genres", genres.iter().map(|g| genre_node(g)))
        .list(
            "artists",
            [artist_ref("artists", album.artist_id, &album.artist_name)],
        )
        .list(
            "recordLabels",
            album
                .label
                .iter()
                .map(|l| XmlNode::new("recordLabels").attr("name", l)),
        )
}

/// An artist as an `ArtistID3` element, without `albumCount`, which each
/// caller knows its own way.
fn artist_id3_node(id: i64, name: &str, extras: &ArtistExtras) -> XmlNode {
    let (mbid, sort_name) = extras
        .0
        .get(&id)
        .map(|(m, s)| (m.as_deref(), s.as_deref()))
        .unwrap_or_default();
    XmlNode::new("artist")
        .attr("id", &id.to_string())
        .attr("name", name)
        .attr("coverArt", &format!("{}{}", ARTIST_PREFIX, id))
        .attr("musicBrainzId", mbid.unwrap_or_default())
        .attr("sortName", sort_name.unwrap_or_default())
}

// ---------------------------------------------------------------------------
// OpenSubsonic fields the row types do not carry
// ---------------------------------------------------------------------------
//
// Read once per response for every entity in it, not once per entity: an album
// list is up to 500 albums.

/// Ids as one JSON array, for `IN (SELECT value FROM json_each(?1))` — a
/// single bound parameter however long the list.
fn json_ids(ids: impl IntoIterator<Item = i64>) -> String {
    serde_json::to_string(&ids.into_iter().collect::<Vec<_>>()).unwrap_or_default()
}

fn by_id<T>(
    db: &Database,
    sql: &str,
    ids: &str,
    mut row: impl FnMut(&rusqlite::Row) -> rusqlite::Result<(i64, T)>,
) -> Result<Vec<(i64, T)>, SubsonicError> {
    let internal = |e: rusqlite::Error| SubsonicError::internal(e.to_string());
    let mut stmt = db.conn.prepare_cached(sql).map_err(internal)?;
    stmt.query_map([ids], |r| row(r))
        .map_err(internal)?
        .collect::<Result<_, _>>()
        .map_err(internal)
}

/// What `Child` carries beyond `TrackRow`.
#[derive(Default)]
struct SongExtras {
    mbid: HashMap<i64, String>,
    /// Last play, seconds since the epoch.
    played: HashMap<i64, i64>,
}

fn song_extras<'a>(
    db: &Database,
    tracks: impl IntoIterator<Item = &'a queries::TrackRow>,
) -> Result<SongExtras, SubsonicError> {
    let ids = json_ids(tracks.into_iter().map(|t| t.id));
    Ok(SongExtras {
        mbid: by_id(
            db,
            "SELECT id, mbid FROM tracks
             WHERE id IN (SELECT value FROM json_each(?1)) AND mbid IS NOT NULL",
            &ids,
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?
        .into_iter()
        .collect(),
        played: by_id(
            db,
            "SELECT track_id, MAX(played_at) FROM play_history
             WHERE track_id IN (SELECT value FROM json_each(?1)) GROUP BY track_id",
            &ids,
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?
        .into_iter()
        .collect(),
    })
}

/// What `AlbumID3` carries beyond `AlbumRow`.
#[derive(Default)]
struct AlbumExtras {
    /// MusicBrainz release id and sort name.
    names: HashMap<i64, (Option<String>, Option<String>)>,
    genres: HashMap<i64, Vec<String>>,
    stats: HashMap<i64, queries::AlbumStats>,
    played: HashMap<i64, i64>,
}

fn album_extras<'a>(
    db: &Database,
    albums: impl IntoIterator<Item = &'a queries::AlbumRow>,
) -> Result<AlbumExtras, SubsonicError> {
    let album_ids: Vec<i64> = albums.into_iter().map(|a| a.id).collect();
    let ids = json_ids(album_ids.iter().copied());
    let mut genres: HashMap<i64, Vec<String>> = HashMap::new();
    for (id, genre) in by_id(
        db,
        "SELECT DISTINCT album_id, genre FROM tracks
         WHERE album_id IN (SELECT value FROM json_each(?1)) AND genre IS NOT NULL AND genre != ''
         ORDER BY album_id, genre",
        &ids,
        |r| Ok((r.get(0)?, r.get(1)?)),
    )? {
        genres.entry(id).or_default().push(genre);
    }
    Ok(AlbumExtras {
        names: by_id(
            db,
            "SELECT id, mbid, sort_name FROM albums WHERE id IN (SELECT value FROM json_each(?1))",
            &ids,
            |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))),
        )?
        .into_iter()
        .collect(),
        genres,
        stats: queries::album_stats(&db.conn, &album_ids)
            .map_err(|e| SubsonicError::internal(e.to_string()))?,
        played: by_id(
            db,
            "SELECT t.album_id, MAX(h.played_at) FROM play_history h
             JOIN tracks t ON t.id = h.track_id
             WHERE t.album_id IN (SELECT value FROM json_each(?1)) GROUP BY t.album_id",
            &ids,
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?
        .into_iter()
        .collect(),
    })
}

/// MusicBrainz artist id and sort name, which `ArtistID3` carries.
struct ArtistExtras(HashMap<i64, (Option<String>, Option<String>)>);

fn artist_extras(
    db: &Database,
    ids: impl IntoIterator<Item = i64>,
) -> Result<ArtistExtras, SubsonicError> {
    Ok(ArtistExtras(
        by_id(
            db,
            "SELECT id, mbid, sort_name FROM artists WHERE id IN (SELECT value FROM json_each(?1))",
            &json_ids(ids),
            |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))),
        )?
        .into_iter()
        .collect(),
    ))
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
        "APE" => ("ape", "audio/x-ape"),
        _ => ("bin", "application/octet-stream"),
    }
}

pub(crate) fn extension_to_mime(ext: &str) -> &str {
    match ext.to_lowercase().as_str() {
        "flac" => "audio/flac",
        "mp3" => "audio/mpeg",
        "m4a" | "aac" | "mp4" => "audio/mp4",
        "opus" => "audio/opus",
        "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        "aiff" | "aif" => "audio/aiff",
        "ape" => "audio/x-ape",
        _ => "application/octet-stream",
    }
}

/// Resolve a track's file path (local preferred, then cached).
pub(crate) fn track_file_path(track: &queries::TrackRow) -> Option<&str> {
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
    submission: Option<bool>,
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
    respond(&state, &params, |_, b| Ok(b))
}

async fn get_license(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    respond(&state, &params, |_, b| {
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
        let extras = artist_extras(db, index_map.values().flatten().map(|a| a.id))?;

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
                    artist_id3_node(artist.id, &artist.name, &extras).attr_int("albumCount", count),
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
        let extras = song_extras(db, &tracks)?;
        for track in &tracks {
            dir = dir.child(track_node(track, "child", &extras));
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

        let artists = artist_extras(db, [artist.id])?;
        let extras = album_extras(db, &albums)?;
        Ok(b.child(
            artist_id3_node(artist.id, &artist.name, &artists)
                .attr_int("albumCount", albums.len() as i64)
                .list(
                    "album",
                    albums.iter().map(|album| album_to_xml_node(album, &extras)),
                ),
        ))
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
        let albums = album_extras(db, [&album])?;
        let songs = song_extras(db, &tracks)?;
        Ok(b.child(
            album_to_xml_node(&album, &albums)
                .list("song", tracks.iter().map(|t| track_to_xml_node(t, &songs))),
        ))
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

    let page: Vec<_> = albums.into_iter().skip(offset).take(size).collect();
    let extras = album_extras(db, &page)?;
    Ok(XmlNode::new(tag).list(
        "album",
        page.iter().map(|album| album_to_xml_node(album, &extras)),
    ))
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
        let extras = song_extras(db, [&track])?;
        Ok(b.child(track_to_xml_node(&track, &extras)))
    })
}

/// The lyrics koan has cached for a song, as one `structuredLyrics` entry, or
/// none. Only the cache is read: fetching from LRCLIB is the player's job, and
/// not something to do inside a client's request.
async fn get_lyrics_by_song_id(
    State(state): State<Arc<AppState>>,
    Query(params): Query<IdParam>,
) -> Response {
    respond_db(&state, &params.auth, |db, b| {
        let track_id = require_id(params.id.as_deref())?;
        let track = queries::get_track_row(&db.conn, track_id)
            .map_err(|e| SubsonicError::internal(e.to_string()))?
            .ok_or_else(|| SubsonicError::not_found("Song"))?;
        let cached = queries::get_cached_lyrics(&db.conn, track_id)
            .map_err(|e| SubsonicError::internal(e.to_string()))?;
        let entries = cached.map(|(content, synced)| structured_lyrics(&track, &content, synced));
        Ok(b.child(XmlNode::new("lyricsList").list("structuredLyrics", entries)))
    })
}

/// A lyrics text as `structuredLyrics`. Synced lyrics are LRC, whose lines
/// carry their start time; plain lyrics are one `line` per line of text.
fn structured_lyrics(track: &queries::TrackRow, content: &str, synced: bool) -> XmlNode {
    let lines: Vec<XmlNode> = if synced {
        koan_core::lyrics::parse_lrc(content)
            .into_iter()
            .map(|l| {
                XmlNode::new("line")
                    .attr_int("start", (l.time_secs * 1000.0).round() as i64)
                    .text(&l.text)
            })
            .collect()
    } else {
        content
            .lines()
            .map(|l| XmlNode::new("line").text(l.trim_end()))
            .collect()
    };
    XmlNode::new("structuredLyrics")
        .attr("displayArtist", &track.artist_name)
        .attr("displayTitle", &track.title)
        // LRCLIB does not say which language a text is in.
        .attr("lang", "und")
        .attr_bool("synced", synced)
        .list("line", lines)
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

        let artist_ids: Vec<(i64, &str)> = {
            let mut seen = std::collections::HashSet::new();
            tracks
                .iter()
                .filter_map(|t| Some((t.artist_id?, t.artist_name.as_str())))
                .filter(|(id, _)| seen.insert(*id))
                .take(artist_count as usize)
                .collect()
        };
        let albums: Vec<queries::AlbumRow> = {
            let mut seen = std::collections::HashSet::new();
            tracks
                .iter()
                .filter_map(|t| t.album_id)
                .filter(|id| seen.insert(*id))
                .take(album_count as usize)
                .map(|id| queries::get_album(&db.conn, id))
                .filter_map(Result::transpose)
                .collect::<Result<_, _>>()
                .map_err(|e| SubsonicError::internal(e.to_string()))?
        };
        let songs: Vec<&queries::TrackRow> = tracks.iter().take(song_count as usize).collect();

        let artist_extras = artist_extras(db, artist_ids.iter().map(|(id, _)| *id))?;
        let album_extras = album_extras(db, &albums)?;
        let song_extras = song_extras(db, songs.iter().copied())?;
        let result_node = XmlNode::new("searchResult3")
            .list(
                "artist",
                artist_ids
                    .iter()
                    .map(|(id, name)| artist_id3_node(*id, name, &artist_extras)),
            )
            .list(
                "album",
                albums.iter().map(|a| album_to_xml_node(a, &album_extras)),
            )
            .list(
                "song",
                songs.iter().map(|t| track_to_xml_node(t, &song_extras)),
            );

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
    serve_local_file(&path, headers).await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            SubsonicError::not_found("File not found on disk")
        } else {
            SubsonicError::internal(e.to_string())
        }
    })
}

/// A file from disk, honouring a Range header so players can seek. Shared by
/// Subsonic's `stream` and the public share pages.
pub(crate) async fn serve_local_file(
    path: &std::path::Path,
    headers: &HeaderMap,
) -> std::io::Result<Response> {
    let total_size = tokio::fs::metadata(path).await?.len();
    let content_type = path
        .extension()
        .and_then(|e| e.to_str())
        .map(extension_to_mime)
        .unwrap_or("application/octet-stream");
    let built = |r: Result<Response, axum::http::Error>| r.map_err(std::io::Error::other);

    if let Some(range_str) = headers.get(header::RANGE).and_then(|v| v.to_str().ok()) {
        match parse_range(range_str, total_size) {
            RangeRequest::Satisfiable { start, end } => {
                let length = end - start + 1;
                let mut file = tokio::fs::File::open(path).await?;
                tokio::io::AsyncSeekExt::seek(&mut file, std::io::SeekFrom::Start(start)).await?;
                let stream = tokio_util::io::ReaderStream::new(file.take(length));
                return built(
                    Response::builder()
                        .status(StatusCode::PARTIAL_CONTENT)
                        .header(header::CONTENT_TYPE, content_type)
                        .header(header::CONTENT_LENGTH, length)
                        .header(
                            header::CONTENT_RANGE,
                            format!("bytes {}-{}/{}", start, end, total_size),
                        )
                        .header(header::ACCEPT_RANGES, "bytes")
                        .body(axum::body::Body::from_stream(stream)),
                );
            }
            RangeRequest::Unsatisfiable => {
                return built(
                    Response::builder()
                        .status(StatusCode::RANGE_NOT_SATISFIABLE)
                        .header(header::CONTENT_RANGE, format!("bytes */{}", total_size))
                        .header(header::ACCEPT_RANGES, "bytes")
                        .body(axum::body::Body::empty()),
                );
            }
            // A header that does not parse is ignored and the whole body sent.
            RangeRequest::Malformed => {}
        }
    }

    let file = tokio::fs::File::open(path).await?;
    built(
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, content_type)
            .header(header::CONTENT_LENGTH, total_size)
            .header(header::ACCEPT_RANGES, "bytes")
            .body(axum::body::Body::from_stream(
                tokio_util::io::ReaderStream::new(file),
            )),
    )
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
    respond_db_as(&state, &params.auth, Role::User, |db, b| {
        toggle_star(db, params.id.as_deref(), true)?;
        Ok(b)
    })
}

async fn unstar(State(state): State<Arc<AppState>>, Query(params): Query<IdParam>) -> Response {
    respond_db_as(&state, &params.auth, Role::User, |db, b| {
        toggle_star(db, params.id.as_deref(), false)?;
        Ok(b)
    })
}

fn toggle_star(db: &Database, id: Option<&str>, star: bool) -> Result<(), SubsonicError> {
    let op = if star {
        queries::add_favourite
    } else {
        queries::remove_favourite
    };
    let track_id = match require_entity(id)? {
        (Some(EntityKind::Song) | None, id) => id,
        _ => return Err(SubsonicError::not_found("Track")),
    };

    let key = queries::track_favourite_key(&db.conn, track_id)
        .map_err(|e| SubsonicError::internal(e.to_string()))?
        .ok_or_else(|| SubsonicError::not_found("Track"))?;
    let path = std::path::Path::new(&key);
    op(&db.conn, path).map_err(|e| SubsonicError::internal(e.to_string()))?;
    koan_core::helpers::sync_favourite_to_remote(db, path, star);
    Ok(())
}

async fn get_starred2(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    respond_db(&state, &params, |db, b| {
        let favourites = queries::load_favourites(&db.conn)
            .map_err(|e| SubsonicError::internal(e.to_string()))?;

        let tracks: Vec<queries::TrackRow> = favourites
            .iter()
            .filter_map(|fav_path| {
                let track_id =
                    queries::track_id_by_path(&db.conn, &fav_path.to_string_lossy()).ok()??;
                queries::get_track_row(&db.conn, track_id).ok()?
            })
            .collect();
        let extras = song_extras(db, &tracks)?;
        Ok(b.child(
            XmlNode::new("starred2")
                .list("song", tracks.iter().map(|t| track_to_xml_node(t, &extras))),
        ))
    })
}

async fn scrobble(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ScrobbleParams>,
) -> Response {
    respond_db_as(&state, &params.auth, Role::User, |db, b| {
        let track_id = require_id(params.id.as_deref())?;

        queries::get_track_row(&db.conn, track_id)
            .map_err(|e| SubsonicError::internal(e.to_string()))?
            .ok_or_else(|| SubsonicError::not_found("Track"))?;

        // `submission=false` is a now-playing notice, not a play.
        if params.submission == Some(false) {
            return Ok(b);
        }

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

        let picked: Vec<&queries::TrackRow> = tracks
            .iter()
            .filter(|t| genre.is_none_or(|g| t.genre.as_deref() == Some(g)))
            .take(size as usize)
            .collect();
        let extras = song_extras(db, picked.iter().copied())?;
        Ok(b.child(
            XmlNode::new("randomSongs")
                .list("song", picked.iter().map(|t| track_to_xml_node(t, &extras))),
        ))
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

        let similar = match track.artist_id {
            Some(artist_id) => queries::get_similar_artists(&db.conn, artist_id)
                .map_err(|e| SubsonicError::internal(e.to_string()))?,
            None => Vec::new(),
        };

        let songs: Vec<queries::TrackRow> = similar
            .iter()
            .filter_map(|(artist_row, _score)| {
                queries::tracks_for_artist(&db.conn, artist_row.id).ok()
            })
            .flatten()
            .take(count)
            .collect();
        let extras = song_extras(db, &songs)?;
        Ok(b.child(
            XmlNode::new("similarSongs2")
                .list("song", songs.iter().map(|t| track_to_xml_node(t, &extras))),
        ))
    })
}

// ---------------------------------------------------------------------------
// Endpoints — server/user metadata
// ---------------------------------------------------------------------------

async fn get_music_folders(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    respond(&state, &params, |_, b| {
        Ok(b.child(
            XmlNode::new("musicFolders").list(
                "musicFolder",
                [XmlNode::new("musicFolder")
                    .attr_int("id", 1)
                    .attr("name", "Music")],
            ),
        ))
    })
}

/// Clients call this during setup to decide which features to offer. It
/// reports the caller's own roles, whatever `username` asks about.
async fn get_user(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    respond(&state, &params, |caller, b| {
        let role = caller.role;
        let writes = role.has_permission(Role::User);
        Ok(b.child(
            XmlNode::new("user")
                .attr("username", &caller.username)
                .attr_bool("scrobblingEnabled", writes)
                .attr_bool("adminRole", role == Role::Admin)
                .attr_bool("settingsRole", false)
                .attr_bool("downloadRole", true)
                .attr_bool("uploadRole", false)
                .attr_bool("playlistRole", writes)
                .attr_bool("coverArtRole", true)
                .attr_bool("commentRole", false)
                .attr_bool("podcastRole", false)
                .attr_bool("streamRole", true)
                .attr_bool("jukeboxRole", false)
                .attr_bool("shareRole", writes)
                .attr_bool("videoConversionRole", false),
        ))
    })
}

/// Answered without authentication, as OpenSubsonic requires: a client asks
/// before it knows which sign-in methods it may use.
async fn get_open_subsonic_extensions(Query(params): Query<SubsonicParams>) -> Response {
    SubsonicResponse::ok(params.wants_json())
        .list(
            "openSubsonicExtensions",
            EXTENSIONS.iter().map(|(name, versions)| {
                XmlNode::new("openSubsonicExtensions")
                    .attr("name", name)
                    .list(
                        "versions",
                        versions
                            .iter()
                            .map(|v| XmlNode::scalar("versions", AttrValue::Int(*v))),
                    )
            }),
        )
        .build()
}

/// Who the credentials belong to. Meant for API keys, which carry no username.
async fn token_info(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    respond(&state, &params, |caller, b| {
        Ok(b.child(XmlNode::new("tokenInfo").attr("username", &caller.username)))
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
// Playlist endpoints
// ---------------------------------------------------------------------------

async fn get_playlists(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    respond_db(&state, &params, |db, b| {
        let lists = queries::list_playlists(&db.conn)
            .map_err(|e| SubsonicError::internal(e.to_string()))?;

        let mut playlists_node = XmlNode::new("playlists").array_of("playlist");
        for list in &lists {
            playlists_node = playlists_node.child(playlist_attrs(
                XmlNode::new("playlist"),
                list,
                &state.username,
            ));
        }

        Ok(b.child(playlists_node))
    })
}

/// The attributes every `<playlist>` carries, list or detail.
fn playlist_attrs(node: XmlNode, list: &queries::PlaylistRow, username: &str) -> XmlNode {
    let node = node
        .attr("id", &list.id.to_string())
        .attr("name", &list.name)
        .attr_int("songCount", list.track_count)
        .attr_int("duration", list.duration_ms / 1000)
        .attr("owner", list.owner.as_deref().unwrap_or(username))
        .attr_bool("public", list.public)
        .attr("created", &list.created_at)
        .attr("changed", &list.changed_at);
    match &list.comment {
        Some(comment) => node.attr("comment", comment),
        None => node,
    }
}

/// A playlist as a `<playlist>` with its members resolved.
fn playlist_node(db: &Database, id: i64, owner: &str) -> Result<XmlNode, SubsonicError> {
    let list = queries::get_playlist(&db.conn, id)
        .map_err(|e| SubsonicError::internal(e.to_string()))?
        .ok_or_else(|| SubsonicError::not_found("Playlist"))?;

    let mut node = playlist_attrs(XmlNode::new("playlist"), &list, owner).array_of("entry");

    // Playlist members are `<entry>`, not `<song>` — an XML client shown
    // `<song>` sees an empty playlist.
    let tracks = queries::playlist_tracks(&db.conn, id)
        .map_err(|e| SubsonicError::internal(e.to_string()))?;
    let extras = song_extras(db, &tracks)?;
    for track in &tracks {
        node = node.child(track_node(track, "entry", &extras));
    }

    Ok(node)
}

/// Parse a playlist id. Subsonic ids are opaque strings; koan's are its row ids.
fn playlist_id(raw: Option<&str>) -> Result<i64, SubsonicError> {
    raw.ok_or_else(|| SubsonicError::missing_param("id"))?
        .parse()
        .map_err(|_| SubsonicError::not_found("Playlist"))
}

async fn get_playlist(
    State(state): State<Arc<AppState>>,
    Query(params): Query<IdParam>,
) -> Response {
    respond_db(&state, &params.auth, |db, b| {
        let id = playlist_id(params.id.as_deref())?;
        Ok(b.child(playlist_node(db, id, &state.username)?))
    })
}

/// `createPlaylist` — new when given a `name`, a wholesale replacement when
/// given a `playlistId`. That second form is the only Subsonic call that can
/// set a playlist's order, which is why koan's own pushes use it too.
async fn create_playlist(State(state): State<Arc<AppState>>, RawQuery(raw): RawQuery) -> Response {
    let params = RawParams::parse(raw.as_deref());
    let auth = params.auth();

    respond_db_as(&state, &auth, Role::User, |db, b| {
        let track_ids: Vec<i64> = params
            .all("songId")
            .filter_map(|id| id.parse::<i64>().ok())
            .collect();

        let id = match params.get("playlistId") {
            Some(existing) => {
                let id = playlist_id(Some(existing))?;
                if let Some(name) = params.get("name") {
                    queries::rename_playlist(&db.conn, id, name)
                        .map_err(|e| SubsonicError::internal(e.to_string()))?;
                }
                queries::set_playlist_tracks(&db.conn, id, &track_ids)
                    .map_err(|e| SubsonicError::internal(e.to_string()))?;
                id
            }
            None => {
                let name = params
                    .get("name")
                    .ok_or_else(|| SubsonicError::missing_param("name"))?;
                let id = queries::create_playlist(&db.conn, name, None)
                    .map_err(|e| SubsonicError::internal(e.to_string()))?;
                queries::add_tracks(&db.conn, id, &track_ids)
                    .map_err(|e| SubsonicError::internal(e.to_string()))?;
                id
            }
        };

        // Since 1.14.0 the response carries the playlist that was created;
        // clients read the id back off it rather than guessing.
        Ok(b.child(playlist_node(db, id, &state.username)?))
    })
}

/// `updatePlaylist` — rename, re-comment, and add or remove members by index.
///
/// Removals are applied by descending index so each one does not shift the
/// next; a client sends them against the list as it stood when it asked.
async fn update_playlist(State(state): State<Arc<AppState>>, RawQuery(raw): RawQuery) -> Response {
    let params = RawParams::parse(raw.as_deref());
    let auth = params.auth();

    respond_db_as(&state, &auth, Role::User, |db, b| {
        let id = playlist_id(params.get("playlistId").or_else(|| params.get("id")))?;
        if queries::get_playlist(&db.conn, id)
            .map_err(|e| SubsonicError::internal(e.to_string()))?
            .is_none()
        {
            return Err(SubsonicError::not_found("Playlist"));
        }

        if let Some(name) = params.get("name") {
            queries::rename_playlist(&db.conn, id, name)
                .map_err(|e| SubsonicError::internal(e.to_string()))?;
        }

        let mut doomed: Vec<usize> = params
            .all("songIndexToRemove")
            .filter_map(|i| i.parse::<usize>().ok())
            .collect();
        if !doomed.is_empty() {
            doomed.sort_unstable();
            doomed.dedup();
            let mut ids = queries::playlist_track_ids(&db.conn, id)
                .map_err(|e| SubsonicError::internal(e.to_string()))?;
            for index in doomed.into_iter().rev() {
                if index < ids.len() {
                    ids.remove(index);
                }
            }
            queries::set_playlist_tracks(&db.conn, id, &ids)
                .map_err(|e| SubsonicError::internal(e.to_string()))?;
        }

        let added: Vec<i64> = params
            .all("songIdToAdd")
            .filter_map(|s| s.parse::<i64>().ok())
            .collect();
        if !added.is_empty() {
            queries::add_tracks(&db.conn, id, &added)
                .map_err(|e| SubsonicError::internal(e.to_string()))?;
        }

        Ok(b)
    })
}

async fn delete_playlist(
    State(state): State<Arc<AppState>>,
    Query(params): Query<IdParam>,
) -> Response {
    respond_db_as(&state, &params.auth, Role::User, |db, b| {
        let id = playlist_id(params.id.as_deref())?;
        match queries::delete_playlist(&db.conn, id) {
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

// ---------------------------------------------------------------------------
// Sharing: links this server serves itself at /share/{id}
// ---------------------------------------------------------------------------

fn iso(secs: i64) -> String {
    chrono::DateTime::from_timestamp(secs, 0)
        .unwrap_or_default()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

fn share_node(
    db: &Database,
    share: &queries::shares::ShareRow,
    base: &str,
    username: &str,
) -> Result<XmlNode, SubsonicError> {
    let rows = queries::tracks_by_ids(&db.conn, &share.track_ids)
        .map_err(|e| SubsonicError::internal(e.to_string()))?;
    let mut node = XmlNode::new("share")
        .array_of("entry")
        .attr("id", &share.id)
        .attr("url", &koan_core::helpers::share_url(base, &share.id))
        .attr("username", username)
        .attr("created", &iso(share.created_at))
        .attr_int("visitCount", share.visits)
        .attr_opt("description", share.description.as_deref());
    if let Some(e) = share.expires_at {
        node = node.attr("expires", &iso(e));
    }
    if let Some(v) = share.last_visited {
        node = node.attr("lastVisited", &iso(v));
    }
    let extras = song_extras(db, &rows)?;
    for id in &share.track_ids {
        if let Some(t) = rows.iter().find(|t| t.id == *id) {
            node = node.child(track_node(t, "entry", &extras));
        }
    }
    Ok(node)
}

/// Where share links point; sharing is refused without it rather than
/// handing out an address that may not be reachable.
fn share_base() -> Result<String, SubsonicError> {
    Config::load()
        .unwrap_or_default()
        .sharing
        .public_url
        .filter(|u| !u.trim().is_empty())
        .ok_or_else(|| SubsonicError::internal("sharing.public_url is not set on this server"))
}

/// `expires` is milliseconds since the epoch; 0 or absent never expires.
fn expires_param(params: &RawParams) -> Option<i64> {
    params
        .get("expires")
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|ms| *ms > 0)
        .map(|ms| ms / 1000)
}

/// What a `createShare` asks for. One song shares its album cued to it, an
/// album the album, an artist their albums; several ids stay a track list,
/// albums among them expanded in place.
fn share_target(
    db: &Database,
    ids: &[(Option<EntityKind>, i64)],
) -> Result<koan_core::helpers::ShareTarget, SubsonicError> {
    use koan_core::helpers::ShareTarget;
    Ok(match ids {
        [] => return Err(SubsonicError::missing_param("id")),
        [(Some(EntityKind::Artist), id)] => ShareTarget::Artist(*id),
        [(Some(EntityKind::Album), id)] => ShareTarget::Album {
            album_id: *id,
            start_track_id: None,
        },
        _ => {
            let mut track_ids = Vec::new();
            for (kind, id) in ids {
                match kind {
                    Some(EntityKind::Album) => track_ids.extend(
                        queries::tracks_for_album(&db.conn, *id)
                            .map_err(|e| SubsonicError::internal(e.to_string()))?
                            .into_iter()
                            .map(|t| t.id),
                    ),
                    Some(EntityKind::Song) | None => track_ids.push(*id),
                    Some(EntityKind::Artist) => return Err(SubsonicError::bad_param("id")),
                }
            }
            ShareTarget::Tracks(track_ids)
        }
    })
}

/// `createShare`, as a slice of the library: see `share_target`.
async fn create_share(State(state): State<Arc<AppState>>, RawQuery(raw): RawQuery) -> Response {
    let params = RawParams::parse(raw.as_deref());
    let auth = params.auth();
    respond_db_as(&state, &auth, Role::User, |db, b| {
        let base = share_base()?;
        let ids: Vec<_> = params
            .all("id")
            .map(|raw| parse_entity_id(raw).ok_or_else(|| SubsonicError::bad_param("id")))
            .collect::<Result<_, _>>()?;
        let target = share_target(db, &ids)?;
        let (slice, track_ids) =
            koan_core::helpers::resolve_share(&db.conn, &target).map_err(|e| match e {
                koan_core::helpers::ShareError::NothingToShare => SubsonicError::not_found("Song"),
                e => SubsonicError::internal(e.to_string()),
            })?;
        let now = chrono::Utc::now().timestamp();
        let share = queries::shares::create_share(
            &db.conn,
            slice,
            &track_ids,
            params.get("description"),
            now,
            expires_param(&params),
        )
        .map_err(|e| SubsonicError::internal(e.to_string()))?;
        Ok(
            b.child(XmlNode::new("shares").array_of("share").child(share_node(
                db,
                &share,
                &base,
                &state.username,
            )?)),
        )
    })
}

async fn get_shares(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    respond_db(&state, &params, |db, b| {
        let base = share_base()?;
        let mut node = XmlNode::new("shares").array_of("share");
        for share in queries::shares::list_shares(&db.conn)
            .map_err(|e| SubsonicError::internal(e.to_string()))?
        {
            node = node.child(share_node(db, &share, &base, &state.username)?);
        }
        Ok(b.child(node))
    })
}

async fn update_share(State(state): State<Arc<AppState>>, RawQuery(raw): RawQuery) -> Response {
    let params = RawParams::parse(raw.as_deref());
    let auth = params.auth();
    respond_db_as(&state, &auth, Role::User, |db, b| {
        let id = params
            .get("id")
            .ok_or_else(|| SubsonicError::missing_param("id"))?;
        let found = queries::shares::update_share(
            &db.conn,
            id,
            params.get("description"),
            expires_param(&params),
        )
        .map_err(|e| SubsonicError::internal(e.to_string()))?;
        if found {
            Ok(b)
        } else {
            Err(SubsonicError::not_found("Share"))
        }
    })
}

async fn delete_share(State(state): State<Arc<AppState>>, RawQuery(raw): RawQuery) -> Response {
    let params = RawParams::parse(raw.as_deref());
    let auth = params.auth();
    respond_db_as(&state, &auth, Role::User, |db, b| {
        let id = params
            .get("id")
            .ok_or_else(|| SubsonicError::missing_param("id"))?;
        let found = queries::shares::delete_share(&db.conn, id)
            .map_err(|e| SubsonicError::internal(e.to_string()))?;
        if found {
            Ok(b)
        } else {
            Err(SubsonicError::not_found("Share"))
        }
    })
}

/// OpenSubsonic `formPost`: the parameters of an
/// `application/x-www-form-urlencoded` POST body are appended to the query
/// string, so every handler reads one set of parameters however they were
/// sent, repeated keys included. Query parameters come first, so they win
/// where a handler reads a single value.
async fn form_post(req: Request, next: Next) -> Response {
    let is_form = req.method() == Method::POST
        && req
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .is_some_and(|v| {
                v.trim()
                    .eq_ignore_ascii_case("application/x-www-form-urlencoded")
            });
    if !is_form {
        return next.run(req).await;
    }

    let (mut parts, body) = req.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, MAX_FORM_BODY).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    let Ok(form) = std::str::from_utf8(&bytes) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let form = form.trim();
    let query = match parts.uri.query().filter(|q| !q.is_empty()) {
        Some(q) if !form.is_empty() => format!("{q}&{form}"),
        Some(q) => q.to_owned(),
        None => form.to_owned(),
    };
    let path_and_query = format!("{}?{query}", parts.uri.path());
    let mut uri = parts.uri.into_parts();
    uri.path_and_query = match path_and_query.parse() {
        Ok(pq) => Some(pq),
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    parts.uri = match axum::http::Uri::from_parts(uri) {
        Ok(u) => u,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    next.run(Request::from_parts(parts, axum::body::Body::empty()))
        .await
}

/// Register all Subsonic REST routes on the given router.
fn register_subsonic_routes(router: axum::Router<Arc<AppState>>) -> axum::Router<Arc<AppState>> {
    router
        // Browsing (ID3)
        .route("/rest/ping", get(ping).post(ping))
        .route("/rest/ping.view", get(ping).post(ping))
        // Sharing
        .route("/rest/createShare", get(create_share).post(create_share))
        .route(
            "/rest/createShare.view",
            get(create_share).post(create_share),
        )
        .route("/rest/getShares", get(get_shares).post(get_shares))
        .route("/rest/getShares.view", get(get_shares).post(get_shares))
        .route("/rest/updateShare", get(update_share).post(update_share))
        .route(
            "/rest/updateShare.view",
            get(update_share).post(update_share),
        )
        .route("/rest/deleteShare", get(delete_share).post(delete_share))
        .route(
            "/rest/deleteShare.view",
            get(delete_share).post(delete_share),
        )
        .route(
            "/rest/getOpenSubsonicExtensions",
            get(get_open_subsonic_extensions).post(get_open_subsonic_extensions),
        )
        .route(
            "/rest/getOpenSubsonicExtensions.view",
            get(get_open_subsonic_extensions).post(get_open_subsonic_extensions),
        )
        .route("/rest/tokenInfo", get(token_info).post(token_info))
        .route("/rest/tokenInfo.view", get(token_info).post(token_info))
        .route(
            "/rest/getLyricsBySongId",
            get(get_lyrics_by_song_id).post(get_lyrics_by_song_id),
        )
        .route(
            "/rest/getLyricsBySongId.view",
            get(get_lyrics_by_song_id).post(get_lyrics_by_song_id),
        )
        .route("/rest/getLicense", get(get_license).post(get_license))
        .route("/rest/getLicense.view", get(get_license).post(get_license))
        .route("/rest/getArtists", get(get_artists).post(get_artists))
        .route("/rest/getArtists.view", get(get_artists).post(get_artists))
        .route("/rest/getArtist", get(get_artist).post(get_artist))
        .route("/rest/getArtist.view", get(get_artist).post(get_artist))
        .route("/rest/getAlbum", get(get_album).post(get_album))
        .route("/rest/getAlbum.view", get(get_album).post(get_album))
        .route(
            "/rest/getAlbumList",
            get(get_album_list).post(get_album_list),
        )
        .route(
            "/rest/getAlbumList.view",
            get(get_album_list).post(get_album_list),
        )
        .route(
            "/rest/getAlbumList2",
            get(get_album_list2).post(get_album_list2),
        )
        .route(
            "/rest/getAlbumList2.view",
            get(get_album_list2).post(get_album_list2),
        )
        .route("/rest/getSong", get(get_song).post(get_song))
        .route("/rest/getSong.view", get(get_song).post(get_song))
        // Browsing (file tree)
        .route("/rest/getIndexes", get(get_indexes).post(get_indexes))
        .route("/rest/getIndexes.view", get(get_indexes).post(get_indexes))
        .route(
            "/rest/getMusicDirectory",
            get(get_music_directory).post(get_music_directory),
        )
        .route(
            "/rest/getMusicDirectory.view",
            get(get_music_directory).post(get_music_directory),
        )
        // Search
        .route("/rest/search3", get(search3).post(search3))
        .route("/rest/search3.view", get(search3).post(search3))
        // Streaming + media
        .route("/rest/stream", get(stream).post(stream))
        .route("/rest/stream.view", get(stream).post(stream))
        .route("/rest/getCoverArt", get(get_cover_art).post(get_cover_art))
        .route(
            "/rest/getCoverArt.view",
            get(get_cover_art).post(get_cover_art),
        )
        // Interaction
        .route("/rest/star", get(star).post(star))
        .route("/rest/star.view", get(star).post(star))
        .route("/rest/unstar", get(unstar).post(unstar))
        .route("/rest/unstar.view", get(unstar).post(unstar))
        .route("/rest/getStarred2", get(get_starred2).post(get_starred2))
        .route(
            "/rest/getStarred2.view",
            get(get_starred2).post(get_starred2),
        )
        .route("/rest/scrobble", get(scrobble).post(scrobble))
        .route("/rest/scrobble.view", get(scrobble).post(scrobble))
        .route(
            "/rest/getRandomSongs",
            get(get_random_songs).post(get_random_songs),
        )
        .route(
            "/rest/getRandomSongs.view",
            get(get_random_songs).post(get_random_songs),
        )
        .route(
            "/rest/getSimilarSongs2",
            get(get_similar_songs2).post(get_similar_songs2),
        )
        .route(
            "/rest/getSimilarSongs2.view",
            get(get_similar_songs2).post(get_similar_songs2),
        )
        // Server + user metadata
        .route(
            "/rest/getMusicFolders",
            get(get_music_folders).post(get_music_folders),
        )
        .route(
            "/rest/getMusicFolders.view",
            get(get_music_folders).post(get_music_folders),
        )
        .route("/rest/getGenres", get(get_genres).post(get_genres))
        .route("/rest/getGenres.view", get(get_genres).post(get_genres))
        .route("/rest/getUser", get(get_user).post(get_user))
        .route("/rest/getUser.view", get(get_user).post(get_user))
        .route(
            "/rest/getScanStatus",
            get(get_scan_status).post(get_scan_status),
        )
        .route(
            "/rest/getScanStatus.view",
            get(get_scan_status).post(get_scan_status),
        )
        // Playlists
        .route("/rest/getPlaylists", get(get_playlists).post(get_playlists))
        .route(
            "/rest/getPlaylists.view",
            get(get_playlists).post(get_playlists),
        )
        .route("/rest/getPlaylist", get(get_playlist).post(get_playlist))
        .route(
            "/rest/getPlaylist.view",
            get(get_playlist).post(get_playlist),
        )
        .route(
            "/rest/createPlaylist",
            get(create_playlist).post(create_playlist),
        )
        .route(
            "/rest/createPlaylist.view",
            get(create_playlist).post(create_playlist),
        )
        .route(
            "/rest/updatePlaylist",
            get(update_playlist).post(update_playlist),
        )
        .route(
            "/rest/updatePlaylist.view",
            get(update_playlist).post(update_playlist),
        )
        .route(
            "/rest/deletePlaylist",
            get(delete_playlist).post(delete_playlist),
        )
        .route(
            "/rest/deletePlaylist.view",
            get(delete_playlist).post(delete_playlist),
        )
        // Everything else under /rest/
        .route(
            "/rest/{*endpoint}",
            get(unsupported_endpoint).post(unsupported_endpoint),
        )
        .layer(axum::middleware::from_fn(form_post))
}

/// Build a Subsonic-compatible REST API router.
///
/// Returns `None` unless `[subsonic]` is enabled and has its own credentials.
/// `/rest/*` carries no JWT layer, so these credentials alone guard every byte
/// of the library — they must never be the upstream `[remote]` password.
pub fn subsonic_router(pool: Arc<Pool>) -> Option<axum::Router> {
    let cfg = Config::load().unwrap_or_default();

    if !cfg.subsonic.enabled {
        return None;
    }

    let password = koan_core::helpers::get_subsonic_password(&cfg)
        .filter(|_| !cfg.subsonic.username.is_empty());
    if password.is_none() {
        log::info!("Subsonic: no shared secret, so only koan accounts sign in (with p=).");
    }

    let state = Arc::new(AppState {
        users: crate::auth::password::PasswordVerifier::new(pool.clone()),
        pool,
        username: cfg.subsonic.username.clone(),
        password,
        upstream: koan_core::helpers::subsonic_auth(&cfg),
        http: reqwest::Client::builder()
            // A whole-request deadline would cut off long proxied streams.
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(30))
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

        koan_core::db::queries::auth::create_user(&db.conn, "mate", "hunter22", Role::Readonly)
            .unwrap();
        koan_core::db::queries::auth::create_user(&db.conn, "owner", "sesame", Role::Admin)
            .unwrap();

        let pool = Arc::new(Pool::new(db_path));
        let state = Arc::new(AppState {
            users: crate::auth::password::PasswordVerifier::new(pool.clone()),
            pool,
            username: "testuser".into(),
            password: Some("testpass".into()),
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
            album_remote_id: None,
            artist_remote_id: None,
            mbid: None,
            album_mbid: None,
            album_added_at: None,
        }
    }

    fn seed_data(state: &AppState) {
        let db = Database::open(state.pool.path()).unwrap();
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
        let db = Database::open(state.pool.path()).unwrap();
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
    fn create_share_picks_the_slice_from_what_was_picked() {
        use koan_core::helpers::ShareTarget;
        let (state, _dir) = test_state();
        let db = Database::open(state.pool.path()).unwrap();
        let a = queries::upsert_track(&db.conn, &track_meta("/m/a.flac", "A", "One", 1)).unwrap();
        let b = queries::upsert_track(&db.conn, &track_meta("/m/b.flac", "B", "One", 2)).unwrap();
        let album = queries::tracks_by_ids(&db.conn, &[a]).unwrap()[0]
            .album_id
            .unwrap();
        let target = |raw: &[&str]| {
            let ids: Vec<_> = raw.iter().map(|r| parse_entity_id(r).unwrap()).collect();
            share_target(&db, &ids).ok()
        };
        assert_eq!(
            target(&[&format!("mf-{a}")]),
            Some(ShareTarget::Tracks(vec![a]))
        );
        assert_eq!(
            target(&[&format!("al-{album}")]),
            Some(ShareTarget::Album {
                album_id: album,
                start_track_id: None
            })
        );
        assert_eq!(target(&["ar-7"]), Some(ShareTarget::Artist(7)));
        assert_eq!(
            target(&[&format!("al-{album}"), &b.to_string()]),
            Some(ShareTarget::Tracks(vec![a, b, b]))
        );
        assert_eq!(target(&["ar-7", "ar-8"]), None);
        assert_eq!(target(&[]), None);
    }

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

    /// Token auth for any username but the shared secret's is 41, which tells
    /// a client to fall back to a password or an API key.
    #[tokio::test]
    async fn test_token_auth_for_other_users_is_41() {
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
        assert!(body.contains("code=\"41\""), "{body}");
        assert!(
            body.contains(&format!("helpUrl=\"{AUTH_HELP_URL}\"")),
            "{body}"
        );
    }

    #[tokio::test]
    async fn test_account_password_auth() {
        let (state, _dir) = test_state();
        // "hunter22" as enc: hex.
        for q in ["p=hunter22", "p=enc:68756e7465723232"] {
            let (_, body) = get_response(
                build_test_router(state.clone()),
                &format!("/rest/ping?u=mate&{q}&v=1.16.1&c=test"),
            )
            .await;
            assert!(body.contains("status=\"ok\""), "{q}: {body}");
        }
        let (_, body) = get_response(
            build_test_router(state),
            "/rest/ping?u=mate&p=wrong&v=1.16.1&c=test",
        )
        .await;
        assert!(body.contains("code=\"40\""));
    }

    #[tokio::test]
    async fn test_shared_secret_as_password() {
        let (state, _dir) = test_state();
        let (_, body) = get_response(
            build_test_router(state),
            "/rest/ping?u=testuser&p=enc:7465737470617373&v=1.16.1&c=test",
        )
        .await;
        assert!(body.contains("status=\"ok\""));
    }

    #[tokio::test]
    async fn test_readonly_account_cannot_write() {
        let (state, _dir) = test_state();
        let (_, body) = get_response(
            build_test_router(state.clone()),
            "/rest/star?id=mf-1&u=mate&p=hunter22&v=1.16.1&c=test",
        )
        .await;
        assert!(body.contains("code=\"50\""), "{body}");
        let (_, body) = get_response(
            build_test_router(state.clone()),
            "/rest/getUser?username=mate&u=mate&p=hunter22&v=1.16.1&c=test",
        )
        .await;
        assert!(body.contains("shareRole=\"false\""), "{body}");
        let (_, body) = get_response(
            build_test_router(state),
            "/rest/getUser?username=owner&u=owner&p=sesame&v=1.16.1&c=test",
        )
        .await;
        assert!(body.contains("adminRole=\"true\""), "{body}");
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

        let db = Database::open(state.pool.path()).unwrap();
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

        let db = Database::open(state.pool.path()).unwrap();
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

    /// Sequential requests, the shape of a client syncing a library. Ignored:
    /// a timing to read, not an assertion. `cargo test -p koan-server --release
    /// -- --ignored --nocapture sequential_request_timing`.
    #[tokio::test]
    #[ignore]
    async fn sequential_request_timing() {
        const N: usize = 500;
        let (state, dir) = test_state();
        seed_data(&state);
        let db = Database::open(&dir.path().join("test.db")).unwrap();
        let album = queries::all_albums(&db.conn).unwrap()[0].id;
        drop(db);
        let app = build_test_router(state);

        for (label, auth) in [
            ("shared secret", auth_query("")),
            ("account", "u=mate&p=hunter22&v=1.16.1&c=test".to_string()),
        ] {
            let uri = format!("/rest/getAlbum?{auth}&id={album}");
            let (status, body) = get_response(app.clone(), &uri).await;
            assert_eq!(status, StatusCode::OK);
            assert!(body.contains("Test Album"), "{body}");
            let start = std::time::Instant::now();
            for _ in 0..N {
                get_response(app.clone(), &uri).await;
            }
            let elapsed = start.elapsed();
            println!(
                "{N} x getAlbum ({label}): {elapsed:?}, {:?} per request",
                elapsed / N as u32
            );
        }
    }

    #[tokio::test]
    async fn test_get_song_by_id() {
        let (state, _dir) = test_state();
        seed_data(&state);

        let db = Database::open(state.pool.path()).unwrap();
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

        let db = Database::open(state.pool.path()).unwrap();
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
                    r#"parent="al-1" coverArt="mf-{id}" type="music" isDir="false" "#,
                    r#"mediaType="song" bitDepth="16" samplingRate="44100" channelCount="2" "#,
                    r#"displayArtist="Test Artist" displayAlbumArtist="Test Artist" "#,
                    r#"musicBrainzId="">"#
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

        let db = Database::open(state.pool.path()).unwrap();
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

        let db = Database::open(state.pool.path()).unwrap();
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

        let db = Database::open(state.pool.path()).unwrap();
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

        let db = Database::open(state.pool.path()).unwrap();
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

        let db = Database::open(state.pool.path()).unwrap();
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

        let db = Database::open(state.pool.path()).unwrap();
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
        // The response carries the playlist itself, per 1.14.0 — which is how
        // the client learns the id it was given.
        assert!(body.contains("name=\"testmix\""), "{}", body);
        let id = playlist_id_in(&body);

        let app = build_test_router(state);
        let (_, body) = get_response(
            app,
            &format!("/rest/getPlaylist?{}&id={id}", auth_query("")),
        )
        .await;
        assert!(body.contains("songCount=\"2\""), "{}", body);
        // Playlist members are `<entry>`, never `<song>`.
        assert_eq!(body.matches("<entry ").count(), 2, "{}", body);
        assert!(!body.contains("<song "));
    }

    /// The `id` off a `<playlist>` element in a response body.
    fn playlist_id_in(body: &str) -> String {
        let start = body
            .find("<playlist id=\"")
            .expect("a playlist in the response")
            + "<playlist id=\"".len();
        let rest = &body[start..];
        rest[..rest.find('"').unwrap()].to_string()
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
        let id = playlist_id_in(&body);

        // List
        let app = build_test_router(state.clone());
        let (_, body) = get_response(app, &format!("/rest/getPlaylists?{}", auth_query(""))).await;
        assert!(body.contains("testmix"));

        // Get
        let app = build_test_router(state.clone());
        let (_, body) = get_response(
            app,
            &format!("/rest/getPlaylist?{}&id={id}", auth_query("")),
        )
        .await;
        assert!(body.contains("testmix"));

        // Delete
        let app = build_test_router(state.clone());
        let (_, body) = get_response(
            app,
            &format!("/rest/deletePlaylist?{}&id={id}", auth_query("")),
        )
        .await;
        assert!(body.contains("status=\"ok\""));

        // Verify deleted
        let app = build_test_router(state);
        let (_, body) = get_response(
            app,
            &format!("/rest/getPlaylist?{}&id={id}", auth_query("")),
        )
        .await;
        assert!(body.contains("status=\"failed\""));
    }

    /// `updatePlaylist` adds and removes members by index, and a removal must
    /// not shift the index of the one after it.
    #[tokio::test]
    async fn test_update_playlist_adds_and_removes() {
        let (state, _dir) = test_state();
        seed_data(&state);

        let db = Database::open(state.pool.path()).unwrap();
        let a = queries::upsert_track(&db.conn, &track_meta("/music/a.flac", "A", "Test Album", 1))
            .unwrap();
        let b = queries::upsert_track(&db.conn, &track_meta("/music/b.flac", "B", "Test Album", 2))
            .unwrap();
        let c = queries::upsert_track(&db.conn, &track_meta("/music/c.flac", "C", "Test Album", 3))
            .unwrap();
        drop(db);

        let app = build_test_router(state.clone());
        let (_, body) = get_response(
            app,
            &format!(
                "/rest/createPlaylist?{}&name=mix&songId={a}&songId={b}&songId={c}",
                auth_query("")
            ),
        )
        .await;
        let id = playlist_id_in(&body);

        let app = build_test_router(state.clone());
        let (_, body) = get_response(
            app,
            &format!(
                "/rest/updatePlaylist?{}&playlistId={id}&songIndexToRemove=0&songIndexToRemove=1&songIdToAdd={a}",
                auth_query("")
            ),
        )
        .await;
        assert!(body.contains("status=\"ok\""), "updatePlaylist: {}", body);

        let db = Database::open(state.pool.path()).unwrap();
        let left = queries::playlist_track_ids(&db.conn, id.parse().unwrap()).unwrap();
        assert_eq!(left, vec![c, a]);
    }

    #[tokio::test]
    async fn test_scrobble() {
        let (state, _dir) = test_state();
        seed_data(&state);

        let db = Database::open(state.pool.path()).unwrap();
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
    async fn test_now_playing_scrobble_records_no_play() {
        let (state, _dir) = test_state();
        seed_data(&state);

        let db = Database::open(state.pool.path()).unwrap();
        let track_id = queries::all_tracks(&db.conn).unwrap()[0].id;

        let app = build_test_router(state);
        let (_, body) = get_response(
            app,
            &format!(
                "/rest/scrobble?{}&id={}&submission=false",
                auth_query(""),
                track_id
            ),
        )
        .await;
        assert!(body.contains("status=\"ok\""));
        assert_eq!(queries::play_count(&db.conn, track_id).unwrap(), 0);
    }

    #[tokio::test]
    async fn test_star_rejects_album_id() {
        let (state, _dir) = test_state();
        seed_data(&state);

        let app = build_test_router(state.clone());
        let (_, body) = get_response(app, &format!("/rest/star?{}&id=al-1", auth_query(""))).await;
        assert!(body.contains("status=\"failed\""), "{}", body);

        let db = Database::open(state.pool.path()).unwrap();
        assert!(queries::load_favourites(&db.conn).unwrap().is_empty());
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
        let db = Database::open(state.pool.path()).unwrap();
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

    // --- OpenSubsonic ---

    async fn json_of(app: axum::Router, uri: &str) -> serde_json::Value {
        let (_, body) = get_response(app, uri).await;
        let parsed: serde_json::Value = serde_json::from_str(&body).expect(&body);
        parsed["subsonic-response"].clone()
    }

    async fn post_form(app: axum::Router, uri: &str, form: &str) -> String {
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header(
                        header::CONTENT_TYPE,
                        "application/x-www-form-urlencoded; charset=utf-8",
                    )
                    .body(Body::from(form.to_owned()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8_lossy(&body).into_owned()
    }

    fn api_key(state: &AppState, username: &str) -> String {
        let db = Database::open(state.pool.path()).unwrap();
        let user = koan_core::db::queries::auth::get_user_by_username(&db.conn, username)
            .unwrap()
            .unwrap();
        queries::api_keys::create_api_key(&db.conn, user.id, "test")
            .unwrap()
            .1
    }

    fn assert_envelope(r: &serde_json::Value, status: &str) {
        assert_eq!(r["status"], status);
        assert_eq!(r["version"], SUBSONIC_API_VERSION);
        assert_eq!(r["type"], "koan");
        assert_eq!(r["serverVersion"], env!("CARGO_PKG_VERSION"));
        assert_eq!(r["openSubsonic"], true);
    }

    #[tokio::test]
    async fn test_envelope_on_success_and_error() {
        let (state, _dir) = test_state();
        let ok = json_of(
            build_test_router(state.clone()),
            &format!("/rest/ping?{}", auth_query("f=json")),
        )
        .await;
        assert_envelope(&ok, "ok");
        let failed = json_of(
            build_test_router(state.clone()),
            "/rest/ping?u=mate&p=wrong&f=json",
        )
        .await;
        assert_envelope(&failed, "failed");
        assert_eq!(failed["error"]["code"], 40);

        let expected = format!(
            "type=\"koan\" serverVersion=\"{}\" openSubsonic=\"true\"",
            env!("CARGO_PKG_VERSION")
        );
        for uri in [
            format!("/rest/ping?{}", auth_query("")),
            "/rest/ping?u=mate&p=wrong".into(),
            format!("/rest/nope?{}", auth_query("")),
        ] {
            let (_, body) = get_response(build_test_router(state.clone()), &uri).await;
            assert!(body.contains(&expected), "{uri}: {body}");
        }
    }

    #[tokio::test]
    async fn test_extensions_need_no_auth() {
        let (state, _dir) = test_state();
        let r = json_of(
            build_test_router(state.clone()),
            "/rest/getOpenSubsonicExtensions?f=json",
        )
        .await;
        assert_envelope(&r, "ok");
        let listed: Vec<(String, Vec<i64>)> = r["openSubsonicExtensions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                (
                    e["name"].as_str().unwrap().to_owned(),
                    serde_json::from_value(e["versions"].clone()).unwrap(),
                )
            })
            .collect();
        let expected: Vec<(String, Vec<i64>)> = EXTENSIONS
            .iter()
            .map(|(n, v)| (n.to_string(), v.to_vec()))
            .collect();
        assert_eq!(listed, expected);

        let (_, xml) = get_response(
            build_test_router(state),
            "/rest/getOpenSubsonicExtensions.view?u=nobody&p=wrong",
        )
        .await;
        assert!(xml.contains("status=\"ok\""), "{xml}");
        assert!(
            xml.contains("<openSubsonicExtensions name=\"formPost\">"),
            "{xml}"
        );
        assert!(xml.contains("<versions>1</versions>"), "{xml}");
    }

    #[tokio::test]
    async fn test_form_post_merges_body_with_query() {
        let (state, dir) = test_state();
        seed_data(&state);
        let second = seed_local_file(&state, dir.path(), b"x");
        let first = {
            let db = Database::open(state.pool.path()).unwrap();
            queries::track_id_by_path(&db.conn, "/music/test.flac")
                .unwrap()
                .unwrap()
        };

        let body = post_form(
            build_test_router(state.clone()),
            "/rest/ping.view",
            &auth_query("f=json"),
        )
        .await;
        assert!(body.contains("\"status\":\"ok\""), "{body}");

        // Auth in the query, the repeated key in the body.
        let body = post_form(
            build_test_router(state.clone()),
            &format!("/rest/createPlaylist?{}", auth_query("f=json")),
            &format!("name=mix&songId={first}&songId%5B%5D={second}"),
        )
        .await;
        let r: serde_json::Value = serde_json::from_str(&body).unwrap();
        let entries = r["subsonic-response"]["playlist"]["entry"]
            .as_array()
            .unwrap();
        let ids: Vec<_> = entries.iter().map(|e| e["id"].clone()).collect();
        assert_eq!(ids, [first.to_string(), second.to_string()]);
    }

    #[tokio::test]
    async fn test_api_key_auth() {
        let (state, _dir) = test_state();
        seed_data(&state);
        let owner = api_key(&state, "owner");
        let mate = api_key(&state, "mate");
        let app = || build_test_router(state.clone());

        let r = json_of(app(), &format!("/rest/ping?apiKey={owner}&f=json")).await;
        assert_envelope(&r, "ok");

        let r = json_of(app(), &format!("/rest/tokenInfo?apiKey={mate}&f=json")).await;
        assert_eq!(r["tokenInfo"]["username"], "mate");
        let r = json_of(app(), &format!("/rest/getUser?apiKey={mate}&f=json")).await;
        assert_eq!(r["user"]["username"], "mate");

        // `u` alongside a key, or any other credential, conflicts.
        for extra in ["u=owner", "p=sesame"] {
            let r = json_of(app(), &format!("/rest/ping?apiKey={owner}&{extra}&f=json")).await;
            assert_eq!(r["error"]["code"], 43, "{extra}");
            assert_eq!(r["error"]["helpUrl"], AUTH_HELP_URL);
        }

        let r = json_of(app(), "/rest/ping?apiKey=not-a-key&f=json").await;
        assert_eq!(r["error"]["code"], 44);

        // A readonly account's key is readonly.
        let r = json_of(app(), &format!("/rest/star?id=mf-1&apiKey={mate}&f=json")).await;
        assert_eq!(r["error"]["code"], 50);
        let r = json_of(app(), &format!("/rest/star?id=mf-1&apiKey={owner}&f=json")).await;
        assert_eq!(r["status"], "ok");

        // Accounts cannot use token auth, whatever the token.
        let r = json_of(app(), "/rest/ping?u=mate&t=abc&s=def&f=json").await;
        assert_eq!(r["error"]["code"], 41);
    }

    #[tokio::test]
    async fn test_lyrics_by_song_id() {
        let (state, _dir) = test_state();
        seed_data(&state);
        let db = Database::open(state.pool.path()).unwrap();
        let id = queries::all_tracks(&db.conn).unwrap()[0].id;
        let uri = format!("/rest/getLyricsBySongId?{}&id={id}", auth_query("f=json"));

        let r = json_of(build_test_router(state.clone()), &uri).await;
        assert_eq!(r["lyricsList"]["structuredLyrics"], serde_json::json!([]));

        queries::cache_lyrics(
            &db.conn,
            id,
            "lrclib",
            true,
            "[ar:Test Artist]\n[00:12.34]First\n[01:00.00]Second",
        )
        .unwrap();
        let r = json_of(build_test_router(state.clone()), &uri).await;
        let entry = &r["lyricsList"]["structuredLyrics"][0];
        assert_eq!(entry["synced"], true);
        assert_eq!(entry["lang"], "und");
        assert_eq!(entry["displayTitle"], "Test Song");
        assert_eq!(
            entry["line"],
            serde_json::json!([
                {"start": 12340, "value": "First"},
                {"start": 60000, "value": "Second"},
            ])
        );

        queries::cache_lyrics(&db.conn, id, "lrclib", false, "One\nTwo").unwrap();
        let r = json_of(build_test_router(state), &uri).await;
        let entry = &r["lyricsList"]["structuredLyrics"][0];
        assert_eq!(entry["synced"], false);
        assert_eq!(
            entry["line"],
            serde_json::json!([{"value": "One"}, {"value": "Two"}])
        );
    }

    #[tokio::test]
    async fn test_opensubsonic_fields() {
        let (state, _dir) = test_state();
        let db = Database::open(state.pool.path()).unwrap();
        let mut meta = track_meta("/music/a.flac", "Song", "Record", 1);
        meta.date = Some("2020-05-17".into());
        meta.mbid = Some("rec-mbid".into());
        meta.album_mbid = Some("rel-mbid".into());
        meta.label = Some("Warp".into());
        queries::upsert_track(&db.conn, &meta).unwrap();
        let track = queries::all_tracks(&db.conn).unwrap().remove(0);
        let album_id = track.album_id.unwrap();
        let app = || build_test_router(state.clone());

        let r = json_of(
            app(),
            &format!("/rest/getAlbum?{}&id={album_id}", auth_query("f=json")),
        )
        .await;
        let album = &r["album"];
        assert_eq!(album["musicBrainzId"], "rel-mbid");
        assert_eq!(album["sortName"], "");
        assert_eq!(album["displayArtist"], "Test Artist");
        assert_eq!(album["songCount"], 1);
        assert_eq!(album["duration"], 240);
        assert_eq!(album["genre"], "Rock");
        assert_eq!(album["genres"], serde_json::json!([{"name": "Rock"}]));
        assert_eq!(album["recordLabels"], serde_json::json!([{"name": "Warp"}]));
        assert_eq!(
            album["releaseDate"],
            serde_json::json!({"year": 2020, "month": 5, "day": 17})
        );
        assert_eq!(album["artists"][0]["name"], "Test Artist");
        assert!(album.get("played").is_none());

        let song = &album["song"][0];
        assert_eq!(song["mediaType"], "song");
        assert_eq!(song["musicBrainzId"], "rec-mbid");
        assert_eq!(song["bitDepth"], 16);
        assert_eq!(song["samplingRate"], 44100);
        assert_eq!(song["channelCount"], 2);
        assert_eq!(song["displayAlbumArtist"], "Test Artist");
        assert_eq!(song["genres"], serde_json::json!([{"name": "Rock"}]));
        assert_eq!(song["artists"][0]["name"], "Test Artist");
        assert!(song.get("played").is_none());

        queries::record_play_at(&db.conn, track.id, 1_700_000_000, None, "local").unwrap();
        let r = json_of(
            app(),
            &format!("/rest/getSong?{}&id={}", auth_query("f=json"), track.id),
        )
        .await;
        assert_eq!(r["song"]["played"], "2023-11-14T22:13:20.000Z");

        let r = json_of(app(), &format!("/rest/getArtists?{}", auth_query("f=json"))).await;
        let artist = &r["artists"]["index"][0]["artist"][0];
        assert_eq!(artist["musicBrainzId"], "");
        assert_eq!(artist["sortName"], "");

        // A lone search hit is still a one-element array.
        let r = json_of(
            app(),
            &format!("/rest/search3?{}&query=Song", auth_query("f=json")),
        )
        .await;
        let found = &r["searchResult3"];
        assert!(
            found["song"].is_array() && found["album"].is_array(),
            "{found}"
        );
        assert_eq!(found["album"][0]["songCount"], 1);
    }
}
