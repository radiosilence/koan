//! Subsonic-compatible REST API layer.
//!
//! Implements a subset of the Subsonic/OpenSubsonic REST API backed by the
//! local koan database.  Supports both XML (default) and JSON (`f=json`)
//! responses.  Clients sign in with a koan account, or with the dedicated
//! `[subsonic]` secret — see `validate_auth`.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path as UrlPath, Query, RawQuery, Request, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use koan_core::auth::Role;
use koan_core::config::Config;
use koan_core::db::connection::Database;
use koan_core::db::pool::{Handle, Pool};
use koan_core::db::queries;
use koan_core::db::queries::app_passwords::AppPasswordAuth;
use koan_core::remote::client::SubsonicAuth;
use serde::Deserialize;
use tokio::io::AsyncReadExt as _;

use crate::auth::password::{FailureLimiter, PasswordVerifier};

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
    // koan's own. See `koan_core::remote::profile`.
    (koan_core::remote::profile::LINK, &[1]),
    (koan_core::remote::profile::DEVICES, &[1]),
    (koan_core::remote::profile::INVITE, &[1]),
    (koan_core::remote::profile::SHARES, &[1]),
    (koan_core::remote::profile::PAIR, &[1]),
    (koan_core::remote::profile::HISTORY, &[1]),
    (koan_core::remote::profile::SIGN_IN, &[1]),
    (koan_core::remote::profile::PASSWORDS, &[1]),
    (koan_core::remote::profile::API_KEYS, &[1]),
];

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
    users: Arc<PasswordVerifier>,
    /// What app passwords are sealed under; `None` until the server has a
    /// signing key, and with it no app passwords.
    app_key: Option<[u8; 32]>,
    /// Upstream Navidrome/Subsonic, used to build signed stream URLs for tracks
    /// with no local file. Resolved once at startup rather than per request,
    /// which would re-read two TOML files. Credentials rather than a
    /// `SubsonicClient`: that builds blocking `reqwest` clients, which panics
    /// when constructed inside the tokio runtime.
    upstream: Option<SubsonicAuth>,
    /// Async client for proxying those streams. `reqwest::Client` owns a
    /// connection pool, so it is built once and cloned.
    http: reqwest::Client,
    /// The web UI's cover cache, so every front end reads one set of renders.
    covers: Arc<crate::covers::Covers>,
    /// What `getIndexes`'s `lastModified` was last worked out from.
    last_modified: parking_lot::Mutex<Option<LibraryModified>>,
    /// `ffmpeg`, where transcoding is on and it was found at startup.
    transcoder: Option<crate::transcode::Transcoder>,
}

/// When the library last changed, as far as this process has seen.
#[derive(Clone, Copy)]
struct LibraryModified {
    fingerprint: u64,
    /// The five-minute window `newest_file` was read in.
    window: u64,
    /// The newest track file's mtime, in milliseconds.
    newest_file: i64,
    /// When the fingerprint was last seen to change, in milliseconds. The
    /// first reading in a process counts as a change, since what changed while
    /// it was down cannot be known: a restart has clients walk once.
    changed: i64,
}

impl AppState {
    fn open_db(&self) -> Result<Handle<'_>, SubsonicError> {
        self.pool
            .get()
            .map_err(|e| SubsonicError::from(e.to_string()))
    }

    /// When the library last changed, in milliseconds: the later of the
    /// newest track file and the last time a track, album or artist was added
    /// or removed. Clients that keep a copy of the library — koan's own among
    /// them — compare it with what they last walked, so it has to move for a
    /// deletion too, which no file's mtime records.
    ///
    /// `MAX(mtime)` reads every track, and clients poll `getIndexes`. Read
    /// again when the library's rows change, and every five minutes for a file
    /// rescanned in place, which changes no row count.
    fn last_modified(&self, db: &Database) -> Result<i64, SubsonicError> {
        let internal =
            |e: koan_core::db::connection::DbError| SubsonicError::internal(e.to_string());
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let window = now.as_secs() / 300;
        let fingerprint = queries::library_fingerprint(&db.conn).map_err(internal)?;
        let held = *self.last_modified.lock();
        if let Some(h) = held
            && h.fingerprint == fingerprint
            && h.window == window
        {
            return Ok(h.newest_file.max(h.changed));
        }
        let newest_file: i64 = db
            .conn
            .query_row(
                "SELECT COALESCE(MAX(mtime), 0) * 1000 FROM tracks",
                [],
                |r| r.get(0),
            )
            .map_err(|e| SubsonicError::internal(e.to_string()))?;
        let changed = match held {
            Some(h) if h.fingerprint == fingerprint => h.changed,
            _ => now.as_millis() as i64,
        };
        *self.last_modified.lock() = Some(LibraryModified {
            fingerprint,
            window,
            newest_file,
            changed,
        });
        Ok(newest_file.max(changed))
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

    /// Every password check slot was taken; see `auth::password`.
    fn busy() -> Self {
        Self::new(
            SubsonicErrorCode::Generic,
            "Server busy checking passwords; try again shortly",
        )
    }

    fn token_auth_unsupported() -> Self {
        Self::auth(
            SubsonicErrorCode::TokenAuthUnsupported,
            "Token authentication needs an app password for this account: make one on the Account page of kōan's web UI, or sign in with your password or an API key",
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

impl From<rusqlite::Error> for SubsonicError {
    fn from(e: rusqlite::Error) -> Self {
        Self::internal(e.to_string())
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
/// key into a `Vec`. `createPlaylist` repeats `songId` once per track, which
/// `Query` would reject with a bare HTTP 400 before the handler ran.
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
        let mut response = XmlBuilder {
            json,
            status: "failed",
            root: XmlNode::new("subsonic-response").child(error),
        }
        .build();
        if matches!(err.code, SubsonicErrorCode::WrongAuth) {
            response.extensions_mut().insert(AuthFailed);
        }
        response
    }
}

/// Marks a response that refused a credential, for [`throttle_auth`] to count.
#[derive(Clone, Copy)]
struct AuthFailed;

/// Failed sign-ins allowed per client and username in a minute.
const AUTH_FAILURES_PER_MINUTE: u32 = 10;
/// Failed sign-ins allowed per client, whatever the username, in a minute.
const AUTH_FAILURES_PER_ADDRESS_PER_MINUTE: u32 = 30;

/// Failed sign-ins counted three ways. An address is an IPv6 client's /64
/// (see `routes::network`).
///
/// By client address and username: behind a relay that does not pass
/// addresses on, every outside client arrives from the relay's, and a tight
/// limit on that alone would let anyone lock everyone out. With the username
/// in the key, guessing one account's password throttles that account's
/// password sign-ins and nothing else.
///
/// By address alone, with a looser limit: otherwise a new username per request
/// gets a fresh allowance every time.
///
/// By username alone, from every address and every door that takes a
/// password, with the loosest: the verifier's `failures`.
struct AuthThrottle {
    per_account: FailureLimiter<(std::net::IpAddr, String)>,
    per_address: FailureLimiter<std::net::IpAddr>,
    users: Arc<PasswordVerifier>,
    /// The `[subsonic]` user, whose token is checked against a random 256-bit
    /// secret rather than a password. `None` without a secret: the name is then
    /// an ordinary account's, and its tokens are checked against its password.
    shared_username: Option<String>,
}

impl AuthThrottle {
    fn new(shared_username: Option<String>, users: Arc<PasswordVerifier>) -> Self {
        Self {
            per_account: FailureLimiter::new(AUTH_FAILURES_PER_MINUTE),
            per_address: FailureLimiter::new(AUTH_FAILURES_PER_ADDRESS_PER_MINUTE),
            users,
            shared_username,
        }
    }
}

/// Refuse password sign-ins from a client that has failed too often, and
/// count failures.
///
/// Every request carries its credential, so each is a sign-in: a password
/// through argon2, or a token against an account's sealed password, which is
/// only an MD5 and cheap to try. Only refusals count, so a client syncing a
/// library is never slowed. API keys and the shared secret's token are random
/// and not worth guessing, so they are never throttled: a flood of wrong
/// passwords for an account cannot lock out the apps signed in with either.
///
/// The exemption follows `validate_auth` exactly. A request with an API key
/// has no other credential checked; one for the shared username with both `t`
/// and `s` is checked against the secret alone. Anything else — `t` without
/// `s` falls through to `p` — is a password sign-in and counted.
async fn throttle_auth(
    State(throttle): State<Arc<AuthThrottle>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let params = RawParams::parse(request.uri().query());
    let username = params.get("u").unwrap_or_default().to_owned();
    let unguessable = params.get("apiKey").is_some()
        || (params.get("t").is_some()
            && params.get("s").is_some()
            && throttle.shared_username.as_deref() == Some(username.as_str()));
    if unguessable {
        return next.run(request).await;
    }

    let from = crate::auth::routes::client_ip(&request);
    let ip = crate::auth::routes::network(from);
    let key = (ip, username);
    let refusal = if throttle.per_account.exhausted(&key) {
        Some("Too many failed sign-ins for this account from this address; try again in a minute")
    } else if throttle.per_address.exhausted(&ip) {
        Some("Too many failed sign-ins from this address; try again in a minute")
    } else if throttle.users.spent(&key.1, from) {
        Some("Too many failed sign-ins for this account; try again in a minute")
    } else {
        None
    };
    if let Some(message) = refusal {
        let json = params.get("f") == Some("json");
        return SubsonicResponse::error(
            json,
            &SubsonicError::new(SubsonicErrorCode::Generic, message),
        );
    }
    let response = next.run(request).await;
    if response.extensions().get::<AuthFailed>().is_some() {
        throttle.users.failed(&key.1);
        throttle.per_account.record(key);
        throttle.per_address.record(ip);
    } else if throttle.users.took_pass(&credential_digest(&params.auth())) {
        throttle.users.signed_in(&key.1, from);
    }
    response
}

/// The credential a request signs in with besides an API key, as
/// `validate_auth` and `throttle_auth` both name it.
fn credential_digest(auth: &SubsonicParams) -> [u8; 32] {
    use sha2::Digest as _;
    let mut digest = sha2::Sha256::new();
    for part in [&auth.u, &auth.p, &auth.t, &auth.s] {
        match part {
            Some(v) => digest.update([&[1u8][..], v.as_bytes(), &[0]].concat()),
            None => digest.update([0u8]),
        }
    }
    digest.finalize().into()
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
    /// Whose favourites, playlists and history the request reads and writes:
    /// the account's, or the local user's for the shared secret.
    user_id: i64,
    /// What signed the request.
    via: Via,
}

/// How a request authenticated, for the endpoints that care.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Via {
    ApiKey,
    /// The account's own password, as `p=`.
    Password,
    AppPassword,
    /// The `[subsonic]` shared secret, which is no account's.
    SharedSecret,
}

impl Caller {
    /// Refuse a request about the account's own credentials, its password and
    /// API keys, unless it is signed with one of them: an API key or the
    /// account's password. An app password is a credential handed to one
    /// client, which must not mint or revoke others; the shared secret is no
    /// account's.
    fn may_manage_credentials(&self) -> Result<(), SubsonicError> {
        match self.via {
            Via::ApiKey | Via::Password => Ok(()),
            Via::AppPassword | Via::SharedSecret => Err(SubsonicError::new(
                SubsonicErrorCode::NotAuthorized,
                "sign in with the account's password or an API key to manage its credentials",
            )),
        }
    }

    /// Whose shares the caller may list and change: their own, or everyone's
    /// for an admin.
    fn share_owner(&self) -> Option<i64> {
        (self.role != Role::Admin).then_some(self.user_id)
    }
}

/// Authenticate a request.
///
/// Three ways in:
/// - `apiKey=`, a key made for a koan account (`koan auth api-key create`, or
///   the web UI's keys page). It names its user, so `u` alongside it is a
///   conflict (43), as is any other credential.
/// - `p=` (plain or `enc:` hex) with a koan account's password, checked
///   against its argon2 hash. The protocol sends it with every request, so it
///   is only as private as the transport. The web login form sends it too.
/// - `t=md5(secret + s)` with the `[subsonic]` shared secret, for clients that
///   only speak token auth. The secret acts as `User`, and is also accepted as
///   `p=`.
/// - `t=md5(app password + s)`, or `p=` with an app password: generated per
///   client for an account, kept sealed (`queries::app_passwords`). Token auth
///   cannot work against the account's own password, which is only a hash, so
///   an account without app passwords is refused with 41, the code that tells
///   a client to fall back to a password or a key.
///
/// A password or app password that signs in is noted for `throttle_auth`,
/// which marks the client's network as the account's.
fn validate_auth(params: &SubsonicParams, state: &AppState) -> Result<Caller, SubsonicError> {
    let caller = check_credential(params, state)?;
    if params.api_key.is_none() && caller.user_id != queries::LOCAL_USER {
        state.users.passed(credential_digest(params));
    }
    Ok(caller)
}

fn check_credential(params: &SubsonicParams, state: &AppState) -> Result<Caller, SubsonicError> {
    use crate::auth::password::Refused;
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
            user_id: user.id,
            via: Via::ApiKey,
        });
    }

    let username = params
        .u
        .as_deref()
        .ok_or_else(|| SubsonicError::missing_param("u"))?;
    let caller = |(user_id, role), via| {
        Ok(Caller {
            username: username.to_owned(),
            role,
            user_id,
            via,
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
                caller((queries::LOCAL_USER, Role::User), Via::SharedSecret)
            } else {
                Err(SubsonicError::wrong_auth())
            };
        }
        // An account: its own password is only a hash, so a token is checked
        // against its app passwords, and without any, 41 tells the client to
        // send the password or a key instead.
        return match app_password(state, username, |password| {
            let expected = format!("{:x}", md5::compute(format!("{password}{salt}")));
            bool::from(token.as_bytes().ct_eq(expected.as_bytes()))
        })? {
            AppPasswordAuth::Matched(user) => Ok(Caller {
                username: user.username,
                role: user.role,
                user_id: user.id,
                via: Via::AppPassword,
            }),
            AppPasswordAuth::Wrong => Err(SubsonicError::wrong_auth()),
            AppPasswordAuth::NoneMade => Err(SubsonicError::token_auth_unsupported()),
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
        return caller((queries::LOCAL_USER, Role::User), Via::SharedSecret);
    }
    // An app password costs a decryption to check, the account's own an
    // argon2 hash, so the cheaper goes first.
    let given = password.as_bytes();
    if let AppPasswordAuth::Matched(user) =
        app_password(state, username, |p| bool::from(p.as_bytes().ct_eq(given)))?
    {
        return Ok(Caller {
            username: user.username,
            role: user.role,
            user_id: user.id,
            via: Via::AppPassword,
        });
    }
    match state.users.verify(username, &password) {
        Ok(account) => caller((account.id, account.role), Via::Password),
        Err(Refused::Busy) => Err(SubsonicError::busy()),
        Err(Refused::Wrong) => Err(SubsonicError::wrong_auth()),
    }
}

/// `username`'s app passwords, checked with `matches`.
fn app_password(
    state: &AppState,
    username: &str,
    matches: impl Fn(&str) -> bool,
) -> Result<AppPasswordAuth, SubsonicError> {
    let Some(key) = state.app_key.as_ref() else {
        return Ok(AppPasswordAuth::NoneMade);
    };
    let db = state.open_db()?;
    queries::app_passwords::authenticate_app_password(&db.conn, key, username, matches)
        .map_err(|e| SubsonicError::internal(e.to_string()))
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
    respond_db_user(state, auth, need, |db, _, b| f(db, b))
}

/// As `respond_db_as`, for an endpoint that reads or writes the caller's own
/// favourites, playlists or history. It is handed the caller's user id.
fn respond_db_user(
    state: &AppState,
    auth: &SubsonicParams,
    need: Role,
    f: impl FnOnce(&Database, i64, XmlBuilder) -> Result<XmlBuilder, SubsonicError>,
) -> Response {
    respond_db_caller(state, auth, need, |db, caller, b| f(db, caller.user_id, b))
}

/// As `respond_db_user`, handing over the whole caller.
fn respond_db_caller(
    state: &AppState,
    auth: &SubsonicParams,
    need: Role,
    f: impl FnOnce(&Database, &Caller, XmlBuilder) -> Result<XmlBuilder, SubsonicError>,
) -> Response {
    let json = auth.wants_json();
    let result = validate_auth(auth, state)
        .and_then(|caller| {
            if need == Role::Readonly {
                Ok(caller)
            } else {
                require_write(caller.role).map(|()| caller)
            }
        })
        .and_then(|caller| Ok((state.open_db()?, caller)))
        .and_then(|(db, caller)| f(&db, &caller, SubsonicResponse::ok(json)));
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

/// Run a handler's synchronous work — authentication, SQLite, argon2, cover
/// decoding — on the blocking pool. Done on a runtime worker it holds that
/// worker until it finishes, and a few concurrent clients then stall every
/// other route in the process, the web UI included.
async fn offload_response(f: impl FnOnce() -> Response + Send + 'static) -> Response {
    tokio::task::spawn_blocking(f)
        .await
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// As [`offload_response`], for work whose result a handler goes on to use.
async fn offload<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, SubsonicError> + Send + 'static,
) -> Result<T, SubsonicError> {
    tokio::task::spawn_blocking(f)
        .await
        .unwrap_or_else(|e| Err(SubsonicError::internal(e.to_string())))
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

/// koan publishes each artist, album and song by its uid, which no two rows of
/// any kind share. Clients that synced before uids existed still hold row ids,
/// which artists, albums and songs number separately: those arrive bare or with
/// a type prefix, and without one, `getCoverArt?id=5` cannot say whether it
/// means album 5 or track 5. Navidrome spells the prefixes the same way.
const ARTIST_PREFIX: &str = "ar-";
const ALBUM_PREFIX: &str = "al-";
const SONG_PREFIX: &str = "mf-";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EntityKind {
    Artist,
    Album,
    Song,
}

impl EntityKind {
    fn uid_kind(self) -> queries::UidKind {
        match self {
            Self::Artist => queries::UidKind::Artist,
            Self::Album => queries::UidKind::Album,
            Self::Song => queries::UidKind::Track,
        }
    }

    fn noun(self) -> &'static str {
        match self {
            Self::Artist => "Artist",
            Self::Album => "Album",
            Self::Song => "Song",
        }
    }
}

impl EntityKind {
    fn of(kind: queries::UidKind) -> Option<Self> {
        match kind {
            queries::UidKind::Artist => Some(Self::Artist),
            queries::UidKind::Album => Some(Self::Album),
            queries::UidKind::Track => Some(Self::Song),
            queries::UidKind::Playlist => None,
        }
    }
}

/// Parse a row id: `ar-3`, `al-3`, `mf-3`, or a bare `3`.
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

/// The row an id names, with its kind when the id carries one: a uid always
/// does, a bare row id never.
fn resolve_entity(db: &Database, raw: &str) -> Result<(Option<EntityKind>, i64), SubsonicError> {
    if let Some(parsed) = parse_entity_id(raw) {
        return Ok(parsed);
    }
    if !queries::is_uid(raw) {
        return Err(SubsonicError::bad_param("id"));
    }
    queries::find_uid(&db.conn, raw)
        .map_err(|e| SubsonicError::internal(e.to_string()))?
        .and_then(|(kind, id)| Some((Some(EntityKind::of(kind)?), id)))
        .ok_or_else(|| SubsonicError::not_found("Item"))
}

/// The row of `kind` an id names. A uid is looked up among that kind only, so
/// an album's uid given to `getSong` is not found rather than read as a song;
/// a row id's prefix is ignored, as it always was.
fn resolve_as(
    db: &Database,
    raw: &str,
    kind: EntityKind,
    param: &str,
) -> Result<i64, SubsonicError> {
    if let Some((_, id)) = parse_entity_id(raw) {
        return Ok(id);
    }
    if !queries::is_uid(raw) {
        return Err(SubsonicError::bad_param(param));
    }
    queries::resolve_id(&db.conn, kind.uid_kind(), raw)
        .map_err(|e| SubsonicError::internal(e.to_string()))?
        .ok_or_else(|| SubsonicError::not_found(kind.noun()))
}

/// The `id` parameter as a row of `kind`.
fn require_id(db: &Database, raw: Option<&str>, kind: EntityKind) -> Result<i64, SubsonicError> {
    let raw = raw.ok_or_else(|| SubsonicError::missing_param("id"))?;
    resolve_as(db, raw, kind, "id")
}

/// The `id` parameter for endpoints that serve more than one kind of entity.
fn require_entity(
    db: &Database,
    raw: Option<&str>,
) -> Result<(Option<EntityKind>, i64), SubsonicError> {
    let raw = raw.ok_or_else(|| SubsonicError::missing_param("id"))?;
    resolve_entity(db, raw)
}

/// Song ids from a repeated parameter, skipping any that name no song.
fn song_ids<'a>(db: &Database, raws: impl Iterator<Item = &'a str>) -> Vec<i64> {
    raws.filter_map(|raw| resolve_as(db, raw, EntityKind::Song, "songId").ok())
        .collect()
}

/// The uid each row in a response is published as, read once per response.
/// A row without one falls back to its prefixed row id, which is still
/// accepted everywhere.
#[derive(Default)]
struct Uids {
    artists: HashMap<i64, String>,
    albums: HashMap<i64, String>,
    tracks: HashMap<i64, String>,
}

impl Uids {
    fn load(
        db: &Database,
        artists: impl IntoIterator<Item = i64>,
        albums: impl IntoIterator<Item = i64>,
        tracks: impl IntoIterator<Item = i64>,
    ) -> Result<Self, SubsonicError> {
        let read = |kind, ids: Vec<i64>| {
            // Nothing to look up is no reason for a query.
            if ids.is_empty() {
                return Ok(HashMap::new());
            }
            queries::uids_for(&db.conn, kind, ids)
                .map_err(|e| SubsonicError::internal(e.to_string()))
        };
        Ok(Self {
            artists: read(
                queries::UidKind::Artist,
                artists.into_iter().collect::<Vec<_>>(),
            )?,
            albums: read(queries::UidKind::Album, albums.into_iter().collect())?,
            tracks: read(queries::UidKind::Track, tracks.into_iter().collect())?,
        })
    }

    fn artist(&self, id: i64) -> String {
        published(&self.artists, ARTIST_PREFIX, id)
    }

    fn album(&self, id: i64) -> String {
        published(&self.albums, ALBUM_PREFIX, id)
    }

    fn track(&self, id: i64) -> String {
        published(&self.tracks, SONG_PREFIX, id)
    }
}

fn published(uids: &HashMap<i64, String>, prefix: &str, id: i64) -> String {
    uids.get(&id)
        .cloned()
        .unwrap_or_else(|| format!("{prefix}{id}"))
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
        .attr("id", &extras.uids.track(track.id))
        .attr("title", &track.title)
        .attr("album", &track.album_title)
        .attr("artist", &track.artist_name)
        .attr_opt_int("track", track.track_number.map(i64::from))
        .attr_opt_int("discNumber", track.disc.map(i64::from))
        .attr_opt_int("duration", duration_secs)
        .attr_opt_int("bitRate", track.bitrate.map(i64::from))
        .attr_opt("suffix", Some(suffix))
        .attr_opt("contentType", Some(content_type))
        // koan's own: the suffix cannot tell ALAC from AAC, both being m4a.
        .attr_opt("codec", track.codec.as_deref())
        .attr_opt("genre", track.genre.as_deref())
        .attr_opt(
            "albumId",
            track.album_id.map(|id| extras.uids.album(id)).as_deref(),
        )
        .attr_opt(
            "artistId",
            track.artist_id.map(|id| extras.uids.artist(id)).as_deref(),
        )
        .attr_opt(
            "parent",
            track.album_id.map(|id| extras.uids.album(id)).as_deref(),
        )
        .attr("coverArt", &extras.uids.track(track.id))
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
        .attr_opt_int(
            "userRating",
            extras.rating.get(&track.id).map(|&r| r.into()),
        )
        .list("genres", track.genre.iter().map(|g| genre_node(g)))
        .list(
            "artists",
            track
                .artist_id
                .map(|id| artist_ref("artists", &extras.uids.artist(id), &track.artist_name)),
        )
}

fn track_to_xml_node(track: &queries::TrackRow, extras: &SongExtras) -> XmlNode {
    track_node(track, "song", extras)
}

fn genre_node(name: &str) -> XmlNode {
    XmlNode::new("genres").attr("name", name)
}

/// An `ArtistID3` inside a list field, with only its required fields.
fn artist_ref(tag: &str, id: &str, name: &str) -> XmlNode {
    XmlNode::new(tag).attr("id", id).attr("name", name)
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
        .attr("id", &extras.uids.album(album.id))
        .attr("name", &album.title)
        .attr("title", &album.title)
        .attr("artist", &album.artist_name)
        .attr("artistId", &extras.uids.artist(album.artist_id))
        .attr("parent", &extras.uids.artist(album.artist_id))
        .attr("coverArt", &extras.uids.album(album.id))
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
        .attr_opt_int(
            "userRating",
            extras.rating.get(&album.id).map(|&r| r.into()),
        )
        .child(item_date("releaseDate", album.date.as_deref()))
        .list("genres", genres.iter().map(|g| genre_node(g)))
        .list(
            "artists",
            [artist_ref(
                "artists",
                &extras.uids.artist(album.artist_id),
                &album.artist_name,
            )],
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
        .names
        .get(&id)
        .map(|(m, s)| (m.as_deref(), s.as_deref()))
        .unwrap_or_default();
    let uid = extras.uids.artist(id);
    XmlNode::new("artist")
        .attr("id", &uid)
        .attr("name", name)
        .attr("coverArt", &uid)
        .attr("musicBrainzId", mbid.unwrap_or_default())
        .attr("sortName", sort_name.unwrap_or_default())
        .attr_opt_int("userRating", extras.rating.get(&id).map(|&r| r.into()))
}

// ---------------------------------------------------------------------------
// OpenSubsonic fields the row types do not carry
// ---------------------------------------------------------------------------
//
// Read once per response for every entity in it, not once per entity: an album
// list is up to 500 albums.

/// `rows` in the order of `ids`, for rows read back by id in whatever order
/// the database chose.
fn in_order<T>(ids: &[i64], rows: Vec<T>, id: impl Fn(&T) -> i64) -> Vec<T> {
    let mut by_id: HashMap<i64, T> = rows.into_iter().map(|r| (id(&r), r)).collect();
    ids.iter().filter_map(|i| by_id.remove(i)).collect()
}

/// Ids as one JSON array, for `IN (SELECT value FROM json_each(?1))` — a
/// single bound parameter however long the list.
fn json_ids(ids: impl IntoIterator<Item = i64>) -> String {
    serde_json::to_string(&ids.into_iter().collect::<Vec<_>>()).unwrap_or_default()
}

fn by_id<T>(
    db: &Database,
    sql: &str,
    params: impl rusqlite::Params,
    mut row: impl FnMut(&rusqlite::Row) -> rusqlite::Result<(i64, T)>,
) -> Result<Vec<(i64, T)>, SubsonicError> {
    let internal = |e: rusqlite::Error| SubsonicError::internal(e.to_string());
    let mut stmt = db.conn.prepare_cached(sql).map_err(internal)?;
    stmt.query_map(params, |r| row(r))
        .map_err(internal)?
        .collect::<Result<_, _>>()
        .map_err(internal)
}

/// What `Child` carries beyond `TrackRow`.
#[derive(Default)]
struct SongExtras {
    uids: Uids,
    mbid: HashMap<i64, String>,
    /// Last play, seconds since the epoch.
    played: HashMap<i64, i64>,
    /// The caller's rating, 1 to 5.
    rating: HashMap<i64, u8>,
}

/// The caller's own history: when someone else last played a track is theirs
/// to know.
fn history_user(db: &Database, user: i64) -> Result<i64, SubsonicError> {
    queries::auth::resolve_user(&db.conn, user).map_err(|e| SubsonicError::internal(e.to_string()))
}

fn song_extras<'a>(
    db: &Database,
    user: i64,
    tracks: impl IntoIterator<Item = &'a queries::TrackRow>,
) -> Result<SongExtras, SubsonicError> {
    let tracks: Vec<_> = tracks.into_iter().collect();
    if tracks.is_empty() {
        return Ok(SongExtras::default());
    }
    let ids = json_ids(tracks.iter().map(|t| t.id));
    let user = history_user(db, user)?;
    Ok(SongExtras {
        uids: Uids::load(
            db,
            tracks.iter().filter_map(|t| t.artist_id),
            tracks.iter().filter_map(|t| t.album_id),
            tracks.iter().map(|t| t.id),
        )?,
        mbid: by_id(
            db,
            "SELECT id, mbid FROM tracks
             WHERE id IN (SELECT value FROM json_each(?1)) AND mbid IS NOT NULL",
            [&ids],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?
        .into_iter()
        .collect(),
        played: by_id(
            db,
            "SELECT track_id, MAX(played_at) FROM play_history
             WHERE track_id IN (SELECT value FROM json_each(?1)) AND user_id = ?2
             GROUP BY track_id",
            rusqlite::params![ids, user],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?
        .into_iter()
        .collect(),
        rating: rated(
            db,
            user,
            queries::RatingKind::Track,
            tracks.iter().map(|t| t.id),
        )?,
    })
}

/// The caller's ratings of these rows.
fn rated(
    db: &Database,
    user: i64,
    kind: queries::RatingKind,
    ids: impl IntoIterator<Item = i64>,
) -> Result<HashMap<i64, u8>, SubsonicError> {
    queries::ratings(&db.conn, user, kind, ids).map_err(|e| SubsonicError::internal(e.to_string()))
}

/// What `AlbumID3` carries beyond `AlbumRow`.
#[derive(Default)]
struct AlbumExtras {
    uids: Uids,
    /// MusicBrainz release id and sort name.
    names: HashMap<i64, (Option<String>, Option<String>)>,
    genres: HashMap<i64, Vec<String>>,
    stats: HashMap<i64, queries::AlbumStats>,
    played: HashMap<i64, i64>,
    rating: HashMap<i64, u8>,
}

/// `tracks`, when the caller already read every track of these albums, is
/// where their genres and totals come from instead of the database.
fn album_extras<'a>(
    db: &Database,
    user: i64,
    albums: impl IntoIterator<Item = &'a queries::AlbumRow>,
    tracks: Option<&[queries::TrackRow]>,
) -> Result<AlbumExtras, SubsonicError> {
    let albums: Vec<_> = albums.into_iter().collect();
    if albums.is_empty() {
        return Ok(AlbumExtras::default());
    }
    let album_ids: Vec<i64> = albums.iter().map(|a| a.id).collect();
    let ids = json_ids(album_ids.iter().copied());
    let user = history_user(db, user)?;
    let (genres, stats) = match tracks {
        Some(tracks) => album_totals(tracks),
        None => {
            let mut genres: HashMap<i64, Vec<String>> = HashMap::new();
            for (id, genre) in by_id(
                db,
                "SELECT DISTINCT album_id, genre FROM tracks
                 WHERE album_id IN (SELECT value FROM json_each(?1))
                   AND genre IS NOT NULL AND genre != ''
                 ORDER BY album_id, genre",
                [&ids],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )? {
                genres.entry(id).or_default().push(genre);
            }
            let stats = queries::album_stats(&db.conn, &album_ids)
                .map_err(|e| SubsonicError::internal(e.to_string()))?;
            (genres, stats)
        }
    };
    Ok(AlbumExtras {
        uids: Uids::load(
            db,
            albums.iter().map(|a| a.artist_id),
            album_ids.iter().copied(),
            [],
        )?,
        names: by_id(
            db,
            "SELECT id, mbid, sort_name FROM albums WHERE id IN (SELECT value FROM json_each(?1))",
            [&ids],
            |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))),
        )?
        .into_iter()
        .collect(),
        genres,
        stats,
        played: by_id(
            db,
            "SELECT t.album_id, MAX(h.played_at) FROM play_history h
             JOIN tracks t ON t.id = h.track_id
             WHERE t.album_id IN (SELECT value FROM json_each(?1)) AND h.user_id = ?2
             GROUP BY t.album_id",
            rusqlite::params![ids, user],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?
        .into_iter()
        .collect(),
        rating: rated(
            db,
            user,
            queries::RatingKind::Album,
            album_ids.iter().copied(),
        )?,
    })
}

/// Each album's genres, sorted and distinct, and its track count and running
/// time, from its tracks. What `album_extras` would otherwise ask the database.
fn album_totals(
    tracks: &[queries::TrackRow],
) -> (HashMap<i64, Vec<String>>, HashMap<i64, queries::AlbumStats>) {
    let mut genres: HashMap<i64, std::collections::BTreeSet<String>> = HashMap::new();
    let mut stats: HashMap<i64, queries::AlbumStats> = HashMap::new();
    for t in tracks {
        let Some(album) = t.album_id else { continue };
        let s = stats.entry(album).or_default();
        s.track_count += 1;
        s.total_duration_ms += t.duration_ms.unwrap_or(0);
        if let Some(genre) = t.genre.as_ref().filter(|g| !g.is_empty()) {
            genres.entry(album).or_default().insert(genre.clone());
        }
    }
    let genres = genres
        .into_iter()
        .map(|(id, g)| (id, g.into_iter().collect()))
        .collect();
    (genres, stats)
}

/// What `ArtistID3` carries beyond a name.
struct ArtistExtras {
    uids: Uids,
    /// MusicBrainz artist id and sort name.
    names: HashMap<i64, (Option<String>, Option<String>)>,
    rating: HashMap<i64, u8>,
}

fn artist_extras(
    db: &Database,
    user: i64,
    ids: impl IntoIterator<Item = i64>,
) -> Result<ArtistExtras, SubsonicError> {
    let ids: Vec<i64> = ids.into_iter().collect();
    Ok(ArtistExtras {
        uids: Uids::load(db, ids.iter().copied(), [], [])?,
        rating: rated(db, user, queries::RatingKind::Artist, ids.iter().copied())?,
        names: by_id(
            db,
            "SELECT id, mbid, sort_name FROM artists WHERE id IN (SELECT value FROM json_each(?1))",
            [json_ids(ids)],
            |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))),
        )?
        .into_iter()
        .collect(),
    })
}

/// An album as a directory `child`, for the file-browse endpoints.
fn album_child_node(album: &queries::AlbumRow, uids: &Uids) -> XmlNode {
    XmlNode::new("child")
        .attr("id", &uids.album(album.id))
        .attr("parent", &uids.artist(album.artist_id))
        .attr("title", &album.title)
        .attr("album", &album.title)
        .attr("artist", &album.artist_name)
        .attr("coverArt", &uids.album(album.id))
        .attr_opt_int("year", year_from_date(album.date.as_deref()))
        .attr_bool("isDir", true)
}

/// Artists bucketed by first letter — the shape `getArtists` (ID3) and
/// `getIndexes` (file-browse) both hang off.
type ArtistIndex = BTreeMap<String, Vec<queries::ArtistRow>>;

fn artist_index(db: &Database) -> Result<ArtistIndex, SubsonicError> {
    // Neither listing shows a track count.
    let artists = queries::list_artists(
        &db.conn,
        &queries::ArtistQuery {
            without_track_counts: true,
            ..Default::default()
        },
    )
    .map_err(|e| SubsonicError::internal(e.to_string()))?;

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

    Ok(index_map)
}

/// A stored codec as a Subsonic `suffix` and `contentType`.
///
/// Covers both what the indexer stores (`index::metadata`) and the suffixes a
/// track synced from another Subsonic server arrives with.
fn codec_to_mime(codec: &str) -> (&str, &str) {
    match codec.to_uppercase().as_str() {
        "FLAC" => ("flac", "audio/flac"),
        "MP3" => ("mp3", "audio/mpeg"),
        "AAC" | "ALAC" | "M4A" | "MP4" => ("m4a", "audio/mp4"),
        "OPUS" => ("opus", "audio/opus"),
        "VORBIS" | "OGG" | "OGA" => ("ogg", "audio/ogg"),
        "WAV" | "PCM" => ("wav", "audio/wav"),
        "AIFF" | "AIF" => ("aiff", "audio/aiff"),
        "APE" => ("ape", "audio/x-ape"),
        _ => ("bin", "application/octet-stream"),
    }
}

pub(crate) fn extension_to_mime(ext: &str) -> &str {
    match ext.to_lowercase().as_str() {
        "flac" => "audio/flac",
        "mp3" => "audio/mpeg",
        "m4a" | "aac" | "mp4" | "alac" => "audio/mp4",
        "opus" => "audio/opus",
        "ogg" | "oga" => "audio/ogg",
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
    genre: Option<String>,
    from_year: Option<i32>,
    to_year: Option<i32>,
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
    artist_offset: Option<u32>,
    album_count: Option<u32>,
    album_offset: Option<u32>,
    song_count: Option<u32>,
    song_offset: Option<u32>,
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
struct RandomSongsParams {
    #[serde(flatten)]
    auth: SubsonicParams,
    size: Option<u32>,
    genre: Option<String>,
    from_year: Option<i32>,
    to_year: Option<i32>,
}

// ===========================================================================
// Endpoints — browsing
// ===========================================================================

async fn ping(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    offload_response(move || respond(&state, &params, |_, b| Ok(b))).await
}

async fn get_license(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    offload_response(move || {
        respond(&state, &params, |_, b| {
            Ok(b.child(
                XmlNode::new("license")
                    .attr_bool("valid", true)
                    .attr("email", "koan@localhost"),
            ))
        })
    })
    .await
}

async fn get_artists(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    offload_response(move || {
        respond_db_user(&state, &params, Role::Readonly, |db, user, b| {
            let index_map = artist_index(db)?;
            let extras = artist_extras(db, user, index_map.values().flatten().map(|a| a.id))?;

            let mut artists_node = XmlNode::new("artists")
                .attr("ignoredArticles", IGNORED_ARTICLES)
                .array_of("index");
            for (letter, group) in &index_map {
                let mut index_node = XmlNode::new("index")
                    .attr("name", letter)
                    .array_of("artist");
                for artist in group {
                    index_node = index_node.child(
                        artist_id3_node(artist.id, &artist.name, &extras)
                            .attr_int("albumCount", artist.album_count),
                    );
                }
                artists_node = artists_node.child(index_node);
            }

            Ok(b.child(artists_node))
        })
    })
    .await
}

/// The file-browse counterpart of `getArtists`. DSub and every folder-oriented
/// client enumerate the library through this and `getMusicDirectory`, so
/// without them they see an empty server.
#[derive(Debug, Default, Deserialize)]
struct IndexesParams {
    #[serde(rename = "ifModifiedSince")]
    if_modified_since: Option<i64>,
}

async fn get_indexes(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
    Query(indexes): Query<IndexesParams>,
) -> Response {
    offload_response(move || {
        respond_db(&state, &params, |db, b| {
            let last_modified = state.last_modified(db)?;
            // Nothing changed since the caller last looked: the timestamp
            // alone, which is all a client checking for changes reads.
            if indexes
                .if_modified_since
                .is_some_and(|since| since >= last_modified)
            {
                return Ok(b.child(
                    XmlNode::new("indexes")
                        .attr_int("lastModified", last_modified)
                        .attr("ignoredArticles", IGNORED_ARTICLES),
                ));
            }
            let index_map = artist_index(db)?;
            let uids = Uids::load(db, index_map.values().flatten().map(|a| a.id), [], [])?;

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
                            .attr("id", &uids.artist(artist.id))
                            .attr("name", &artist.name),
                    );
                }
                indexes_node = indexes_node.child(index_node);
            }

            Ok(b.child(indexes_node))
        })
    })
    .await
}

/// One level of the browse tree: an artist directory lists its albums, an album
/// directory lists its songs.
async fn get_music_directory(
    State(state): State<Arc<AppState>>,
    Query(params): Query<IdParam>,
) -> Response {
    offload_response(move || {
        respond_db_user(&state, &params.auth, Role::Readonly, |db, user, b| {
            let (kind, id) = require_entity(db, params.id.as_deref())?;

            // A bare row id is ambiguous — artists and albums number separately —
            // so try the artist table first and fall through. Clients that arrived
            // via `getIndexes` send a uid and never hit this.
            if kind != Some(EntityKind::Album) {
                let artist = queries::get_artist(&db.conn, id)
                    .map_err(|e| SubsonicError::internal(e.to_string()))?
                    .filter(|a| a.album_count > 0);
                if let Some(artist) = artist {
                    let albums = queries::albums_for_artist(&db.conn, id)
                        .map_err(|e| SubsonicError::internal(e.to_string()))?;
                    let uids = Uids::load(db, [artist.id], albums.iter().map(|a| a.id), [])?;
                    let mut dir = XmlNode::new("directory")
                        .attr("id", &uids.artist(artist.id))
                        .attr("name", &artist.name)
                        .array_of("child");
                    for album in &albums {
                        dir = dir.child(album_child_node(album, &uids));
                    }
                    return Ok(b.child(dir));
                }
            }

            let album = queries::get_album(&db.conn, id)
                .map_err(|e| SubsonicError::internal(e.to_string()))?
                .ok_or_else(|| SubsonicError::not_found("Directory"))?;
            let tracks = queries::tracks_for_album(&db.conn, id)
                .map_err(|e| SubsonicError::internal(e.to_string()))?;

            let uids = Uids::load(db, [album.artist_id], [album.id], [])?;
            let mut dir = XmlNode::new("directory")
                .attr("id", &uids.album(album.id))
                .attr("parent", &uids.artist(album.artist_id))
                .attr("name", &album.title)
                .array_of("child");
            let extras = song_extras(db, user, &tracks)?;
            for track in &tracks {
                dir = dir.child(track_node(track, "child", &extras));
            }
            Ok(b.child(dir))
        })
    })
    .await
}

async fn get_artist(State(state): State<Arc<AppState>>, Query(params): Query<IdParam>) -> Response {
    offload_response(move || {
        respond_db_user(&state, &params.auth, Role::Readonly, |db, user, b| {
            let artist_id = require_id(db, params.id.as_deref(), EntityKind::Artist)?;

            let artist = queries::get_artist(&db.conn, artist_id)
                .map_err(|e| SubsonicError::internal(e.to_string()))?
                .ok_or_else(|| SubsonicError::not_found("Artist"))?;

            let albums = queries::albums_for_artist(&db.conn, artist_id)
                .map_err(|e| SubsonicError::internal(e.to_string()))?;

            let artists = artist_extras(db, user, [artist.id])?;
            let extras = album_extras(db, user, &albums, None)?;
            Ok(b.child(
                artist_id3_node(artist.id, &artist.name, &artists)
                    .attr_int("albumCount", albums.len() as i64)
                    .list(
                        "album",
                        albums.iter().map(|album| album_to_xml_node(album, &extras)),
                    ),
            ))
        })
    })
    .await
}

async fn get_album(State(state): State<Arc<AppState>>, Query(params): Query<IdParam>) -> Response {
    offload_response(move || {
        respond_db_user(&state, &params.auth, Role::Readonly, |db, user, b| {
            let album_id = require_id(db, params.id.as_deref(), EntityKind::Album)?;

            let album = queries::get_album(&db.conn, album_id)
                .map_err(|e| SubsonicError::internal(e.to_string()))?
                .ok_or_else(|| SubsonicError::not_found("Album"))?;

            let tracks = queries::tracks_for_album(&db.conn, album_id)
                .map_err(|e| SubsonicError::internal(e.to_string()))?;
            let albums = album_extras(db, user, [&album], Some(&tracks))?;
            let songs = song_extras(db, user, &tracks)?;
            Ok(b.child(
                album_to_xml_node(&album, &albums)
                    .list("song", tracks.iter().map(|t| track_to_xml_node(t, &songs))),
            ))
        })
    })
    .await
}

/// Albums ordered by `type`, paged in SQL. Shared by `getAlbumList` and
/// `getAlbumList2`, which differ only in the element they hang the list off.
fn album_list(
    db: &Database,
    user: i64,
    params: &AlbumListParams,
    tag: &str,
) -> Result<XmlNode, SubsonicError> {
    let limit = params.size.unwrap_or(20).clamp(0, 500) as u32;
    let offset = params.offset.unwrap_or(0).clamp(0, u32::MAX as i64) as u32;
    let played = |order| {
        queries::played_albums(&db.conn, user, order, limit, offset)
            .map_err(|e| SubsonicError::internal(e.to_string()))
    };
    let page = match params.list_type.as_deref().unwrap_or("alphabeticalByName") {
        "recent" => played(queries::PlayedOrder::Recent)?,
        "frequent" => played(queries::PlayedOrder::Frequent)?,
        "highest" => queries::highest_rated_albums(&db.conn, user, limit, offset)
            .map_err(|e| SubsonicError::internal(e.to_string()))?,
        list_type => {
            let q = queries::AlbumQuery {
                limit: Some(limit),
                offset,
                ..album_query(list_type, user, params)?
            };
            queries::list_albums(&db.conn, &q)
                .map_err(|e| SubsonicError::internal(e.to_string()))?
        }
    };
    let extras = album_extras(db, user, &page, None)?;
    Ok(XmlNode::new(tag).list(
        "album",
        page.iter().map(|album| album_to_xml_node(album, &extras)),
    ))
}

/// The library listing a `getAlbumList` type asks for, unpaged.
fn album_query<'a>(
    list_type: &str,
    user: i64,
    params: &'a AlbumListParams,
) -> Result<queries::AlbumQuery<'a>, SubsonicError> {
    use queries::{AlbumFilter, AlbumOrder, AlbumQuery};
    let order = |order| AlbumQuery {
        order,
        ..Default::default()
    };
    Ok(match list_type {
        "alphabeticalByName" => order(AlbumOrder::Title),
        "alphabeticalByArtist" => order(AlbumOrder::ArtistThenDate),
        // Recently added, as Subsonic means it — not release date.
        "newest" => order(AlbumOrder::RecentlyAdded),
        "random" => order(AlbumOrder::Random(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as i64,
        )),
        "starred" => AlbumQuery {
            favourites_of: Some(user),
            ..Default::default()
        },
        "byGenre" => AlbumQuery {
            filter: AlbumFilter {
                genre: Some(
                    params
                        .genre
                        .as_deref()
                        .ok_or_else(|| SubsonicError::missing_param("genre"))?,
                ),
                ..Default::default()
            },
            ..Default::default()
        },
        "byYear" => {
            let from = params
                .from_year
                .ok_or_else(|| SubsonicError::missing_param("fromYear"))?;
            let to = params
                .to_year
                .ok_or_else(|| SubsonicError::missing_param("toYear"))?;
            AlbumQuery {
                filter: AlbumFilter {
                    year_from: Some(from.min(to)),
                    year_to: Some(from.max(to)),
                    ..Default::default()
                },
                // fromYear after toYear asks for the range newest first.
                ..order(if from > to {
                    AlbumOrder::YearDesc
                } else {
                    AlbumOrder::Date
                })
            }
        }
        _ => return Err(SubsonicError::bad_param("type")),
    })
}

async fn get_album_list(
    State(state): State<Arc<AppState>>,
    Query(params): Query<AlbumListParams>,
) -> Response {
    offload_response(move || {
        respond_db_user(&state, &params.auth, Role::Readonly, |db, user, b| {
            Ok(b.child(album_list(db, user, &params, "albumList")?))
        })
    })
    .await
}

async fn get_album_list2(
    State(state): State<Arc<AppState>>,
    Query(params): Query<AlbumListParams>,
) -> Response {
    offload_response(move || {
        respond_db_user(&state, &params.auth, Role::Readonly, |db, user, b| {
            Ok(b.child(album_list(db, user, &params, "albumList2")?))
        })
    })
    .await
}

async fn get_song(State(state): State<Arc<AppState>>, Query(params): Query<IdParam>) -> Response {
    offload_response(move || {
        respond_db_user(&state, &params.auth, Role::Readonly, |db, user, b| {
            let track_id = require_id(db, params.id.as_deref(), EntityKind::Song)?;
            let track = queries::get_track_row(&db.conn, track_id)
                .map_err(|e| SubsonicError::internal(e.to_string()))?
                .ok_or_else(|| SubsonicError::not_found("Song"))?;
            let extras = song_extras(db, user, [&track])?;
            Ok(b.child(track_to_xml_node(&track, &extras)))
        })
    })
    .await
}

/// `getBookmarks`: the caller's places in tracks, most recently changed first,
/// each with the track as its `entry`.
async fn get_bookmarks(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    offload_response(move || {
        respond_db_caller(&state, &params, Role::Readonly, |db, caller, b| {
            let internal = |e: rusqlite::Error| SubsonicError::internal(e.to_string());
            let marks = queries::bookmarks(&db.conn, caller.user_id).map_err(internal)?;
            let ids: Vec<i64> = marks.iter().map(|m| m.track_id).collect();
            let tracks: HashMap<i64, queries::TrackRow> = queries::tracks_by_ids(&db.conn, &ids)
                .map_err(|e| SubsonicError::internal(e.to_string()))?
                .into_iter()
                .map(|t| (t.id, t))
                .collect();
            let extras = song_extras(db, caller.user_id, tracks.values())?;
            Ok(b.child(XmlNode::new("bookmarks").list(
                "bookmark",
                marks.iter().filter_map(|m| {
                    let track = tracks.get(&m.track_id)?;
                    Some(
                        XmlNode::new("bookmark")
                            .attr_int("position", m.position_ms)
                            .attr("username", &caller.username)
                            .attr_opt("comment", m.comment.as_deref())
                            .attr("created", &iso(m.created_at))
                            .attr("changed", &iso(m.changed_at))
                            .child(track_node(track, "entry", &extras)),
                    )
                }),
            )))
        })
    })
    .await
}

/// A note on a place in a track. Longer is refused rather than cut, so a
/// client never reads back something other than what it saved.
const MAX_BOOKMARK_COMMENT: usize = 1024;

/// `createBookmark`: save where the caller is in the song `id` names, as
/// `position` milliseconds and an optional `comment`. One per song; a second
/// replaces the first.
async fn create_bookmark(State(state): State<Arc<AppState>>, RawQuery(raw): RawQuery) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let auth = params.auth();
        respond_db_user(&state, &auth, Role::User, |db, user, b| {
            let track_id = require_id(db, params.get("id"), EntityKind::Song)?;
            let position = params
                .get("position")
                .ok_or_else(|| SubsonicError::missing_param("position"))?
                .parse::<i64>()
                .ok()
                .filter(|p| *p >= 0)
                .ok_or_else(|| SubsonicError::bad_param("position"))?;
            let comment = params.get("comment");
            if comment.is_some_and(|c| c.chars().count() > MAX_BOOKMARK_COMMENT) {
                return Err(SubsonicError::bad_param("comment"));
            }
            queries::get_track_row(&db.conn, track_id)
                .map_err(|e| SubsonicError::internal(e.to_string()))?
                .ok_or_else(|| SubsonicError::not_found("Song"))?;
            queries::save_bookmark(&db.conn, user, track_id, position, comment)
                .map_err(|e| SubsonicError::internal(e.to_string()))?;
            Ok(b)
        })
    })
    .await
}

/// `deleteBookmark`: forget the caller's place in the song `id` names. One
/// that was never saved is already forgotten, as Navidrome answers it.
async fn delete_bookmark(State(state): State<Arc<AppState>>, RawQuery(raw): RawQuery) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let auth = params.auth();
        respond_db_user(&state, &auth, Role::User, |db, user, b| {
            let track_id = require_id(db, params.get("id"), EntityKind::Song)?;
            queries::delete_bookmark(&db.conn, user, track_id)
                .map_err(|e| SubsonicError::internal(e.to_string()))?;
            Ok(b)
        })
    })
    .await
}

/// The lyrics koan has cached for a song, as one `structuredLyrics` entry, or
/// none. Only the cache is read: fetching from LRCLIB is the player's job, and
/// not something to do inside a client's request.
async fn get_lyrics_by_song_id(
    State(state): State<Arc<AppState>>,
    Query(params): Query<IdParam>,
) -> Response {
    offload_response(move || {
        respond_db(&state, &params.auth, |db, b| {
            let track_id = require_id(db, params.id.as_deref(), EntityKind::Song)?;
            let track = queries::get_track_row(&db.conn, track_id)
                .map_err(|e| SubsonicError::internal(e.to_string()))?
                .ok_or_else(|| SubsonicError::not_found("Song"))?;
            let cached = queries::get_cached_lyrics(&db.conn, track_id)
                .map_err(|e| SubsonicError::internal(e.to_string()))?;
            let entries =
                cached.map(|(content, synced)| structured_lyrics(&track, &content, synced));
            Ok(b.child(XmlNode::new("lyricsList").list("structuredLyrics", entries)))
        })
    })
    .await
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

/// The most `search3` returns of any one kind per request. Large enough that a
/// client walking the whole library needs few pages, small enough that one
/// response stays a few megabytes.
const SEARCH_PAGE_MAX: u32 = 1000;

/// Whether a `search3` query asks for everything. OpenSubsonic clients send an
/// empty query, and some send a pair of quotes, to list the whole library.
fn lists_everything(query: &str) -> bool {
    matches!(query.trim(), "" | "\"\"")
}

async fn search3(
    State(state): State<Arc<AppState>>,
    Query(params): Query<Search3Params>,
) -> Response {
    offload_response(move || {
        respond_db_user(&state, &params.auth, Role::Readonly, |db, user, b| {
            let query = params
                .query
                .as_deref()
                .ok_or_else(|| SubsonicError::missing_param("query"))?;

            let artist_count = params.artist_count.unwrap_or(20).min(SEARCH_PAGE_MAX);
            let album_count = params.album_count.unwrap_or(20).min(SEARCH_PAGE_MAX);
            let song_count = params.song_count.unwrap_or(20).min(SEARCH_PAGE_MAX);
            let artist_offset = params.artist_offset.unwrap_or(0);
            let album_offset = params.album_offset.unwrap_or(0);
            let song_offset = params.song_offset.unwrap_or(0);
            let internal =
                |e: koan_core::db::connection::DbError| SubsonicError::internal(e.to_string());

            let (artists, albums, songs): (
                Vec<(i64, String)>,
                Vec<queries::AlbumRow>,
                Vec<queries::TrackRow>,
            ) = if lists_everything(query) {
                // Every kind in id order, straight off the primary keys, so an
                // offset walk is exact: nothing added mid-walk can shift a page
                // that has already been read.
                let artists = if artist_count == 0 {
                    Vec::new()
                } else {
                    queries::list_artists(
                        &db.conn,
                        &queries::ArtistQuery {
                            order: queries::ArtistOrder::Id,
                            limit: Some(artist_count),
                            offset: artist_offset,
                            ..Default::default()
                        },
                    )
                    .map_err(internal)?
                    .into_iter()
                    .map(|a| (a.id, a.name))
                    .collect()
                };
                let albums = if album_count == 0 {
                    Vec::new()
                } else {
                    queries::list_albums(
                        &db.conn,
                        &queries::AlbumQuery {
                            order: queries::AlbumOrder::Id,
                            limit: Some(album_count),
                            offset: album_offset,
                            ..Default::default()
                        },
                    )
                    .map_err(internal)?
                };
                let songs = if song_count == 0 {
                    Vec::new()
                } else {
                    queries::tracks_page(&db.conn, song_count, song_offset).map_err(internal)?
                };
                (artists, albums, songs)
            } else {
                let songs = if song_count == 0 {
                    Vec::new()
                } else {
                    queries::search_tracks_paged(&db.conn, query, song_count, song_offset)
                        .map_err(internal)?
                };
                // Artists and albums are the distinct ones among the matching
                // tracks, read from enough of them to cover the page asked for.
                let reach = (artist_offset + artist_count)
                    .max(album_offset + album_count)
                    .saturating_mul(5)
                    .clamp(100, 5 * SEARCH_PAGE_MAX);
                let pool = if artist_count == 0 && album_count == 0 {
                    Vec::new()
                } else {
                    queries::search_tracks_paged(&db.conn, query, reach, 0).map_err(internal)?
                };
                let artists = {
                    let mut seen = std::collections::HashSet::new();
                    pool.iter()
                        .filter_map(|t| Some((t.artist_id?, t.artist_name.clone())))
                        .filter(|(id, _)| seen.insert(*id))
                        .skip(artist_offset as usize)
                        .take(artist_count as usize)
                        .collect()
                };
                let albums = {
                    let mut seen = std::collections::HashSet::new();
                    let ids: Vec<i64> = pool
                        .iter()
                        .filter_map(|t| t.album_id)
                        .filter(|id| seen.insert(*id))
                        .skip(album_offset as usize)
                        .take(album_count as usize)
                        .collect();
                    let rows = if ids.is_empty() {
                        Vec::new()
                    } else {
                        queries::list_albums(
                            &db.conn,
                            &queries::AlbumQuery {
                                ids: Some(&ids),
                                ..Default::default()
                            },
                        )
                        .map_err(internal)?
                    };
                    in_order(&ids, rows, |a| a.id)
                };
                (artists, albums, songs)
            };

            let artist_extras = artist_extras(db, user, artists.iter().map(|(id, _)| *id))?;
            let album_extras = album_extras(db, user, &albums, None)?;
            let song_extras = song_extras(db, user, &songs)?;
            let result_node = XmlNode::new("searchResult3")
                .list(
                    "artist",
                    artists
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
    })
    .await
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
    /// Strings for the same reason as `id`. Read only by `stream`; `download`
    /// is the original file by definition.
    #[serde(rename = "maxBitRate")]
    max_bit_rate: Option<String>,
    format: Option<String>,
    #[serde(rename = "timeOffset")]
    time_offset: Option<String>,
    #[serde(rename = "estimateContentLength")]
    estimate_content_length: Option<String>,
}

/// What `stream_inner` may send in place of the file itself.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Delivery {
    /// The original, always: `download`.
    Original,
    /// A transcode where the request asks for one.
    Transcode,
    /// The headers a transcode would have, without running one: `stream`'s HEAD.
    TranscodeHead,
}

async fn stream(
    State(state): State<Arc<AppState>>,
    method: axum::http::Method,
    Query(params): Query<StreamParams>,
    headers: HeaderMap,
) -> Response {
    let json = params.auth.wants_json();
    let delivery = if method == axum::http::Method::HEAD {
        Delivery::TranscodeHead
    } else {
        Delivery::Transcode
    };
    match stream_inner(state, params, &headers, delivery).await {
        Ok(resp) => resp,
        Err(e) => SubsonicResponse::error(json, &e),
    }
}

async fn download(
    State(state): State<Arc<AppState>>,
    Query(params): Query<StreamParams>,
    headers: HeaderMap,
) -> Response {
    let json = params.auth.wants_json();
    match stream_inner(state, params, &headers, Delivery::Original).await {
        Ok(resp) => resp,
        Err(e) => SubsonicResponse::error(json, &e),
    }
}

async fn stream_inner(
    state: Arc<AppState>,
    params: StreamParams,
    headers: &HeaderMap,
    delivery: Delivery,
) -> Result<Response, SubsonicError> {
    let lookup = state.clone();
    let StreamParams {
        auth,
        id,
        max_bit_rate,
        format,
        time_offset,
        estimate_content_length,
    } = params;
    let (username, track) = offload(move || {
        let caller = validate_auth(&auth, &lookup)?;
        let db = lookup.open_db()?;
        let track_id = require_id(&db, id.as_deref(), EntityKind::Song)?;
        let track = queries::get_track_row(&db.conn, track_id)
            .map_err(|e| SubsonicError::internal(e.to_string()))?
            .ok_or_else(|| SubsonicError::not_found("Track"))?;
        Ok((caller.username, track))
    })
    .await?;

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
            return proxy_stream_from_upstream(&state, remote_id, &track, headers).await;
        }
        return Err(SubsonicError::not_found(
            "Track has no local file and no remote source",
        ));
    }

    let path = local_path.unwrap();
    if delivery != Delivery::Original
        && let Some(transcoder) = &state.transcoder
    {
        let request = crate::transcode::Request {
            max_bit_rate: max_bit_rate.as_deref(),
            format: format.as_deref(),
            time_offset: time_offset.as_deref(),
        };
        let source_kbps = track.bitrate.and_then(|b| u32::try_from(b).ok());
        let plan = crate::transcode::plan(&request, track.codec.as_deref(), source_kbps)
            .filter(|plan| transcoder.encodes(plan.codec));
        if let Some(plan) = plan {
            let length = (estimate_content_length.as_deref() == Some("true"))
                .then(|| crate::transcode::estimated_length(&plan, track.duration_ms))
                .flatten();
            if delivery == Delivery::TranscodeHead {
                return crate::transcode::Transcoder::head(&plan, length)
                    .map_err(|e| SubsonicError::internal(e.to_string()));
            }
            match transcoder.permits(&username) {
                None => log::info!("transcode: at the limit, serving {username} the original"),
                Some(permits) => match transcoder.stream(&path, &plan, length, permits).await {
                    Ok(Some(resp)) => return Ok(resp),
                    Ok(None) => {}
                    Err(e) => {
                        log::warn!("transcode: could not start ffmpeg, serving the original: {e}")
                    }
                },
            }
        }
    }
    serve_local_file(&path, headers).await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            SubsonicError::not_found("File not found on disk")
        } else {
            SubsonicError::internal(e.to_string())
        }
    })
}

/// How much of a file each read takes. `tokio::fs` runs every read on the
/// blocking pool, and the 4 KiB default made a 500 MB album download some
/// 128,000 of them.
const STREAM_CHUNK: usize = 256 * 1024;

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
                let stream =
                    tokio_util::io::ReaderStream::with_capacity(file.take(length), STREAM_CHUNK);
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
                tokio_util::io::ReaderStream::with_capacity(file, STREAM_CHUNK),
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

    if let Some(range) = client_headers.get(header::RANGE)
        && let Ok(range_str) = range.to_str()
    {
        req = req.header("Range", range_str);
    }

    // reqwest's error names the URL, and the URL carries the upstream
    // account's credentials: never let it reach the client or the log.
    let upstream_resp = req.send().await.map_err(|e| {
        log::warn!("stream proxy: upstream request failed: {}", e.without_url());
        SubsonicError::internal("Upstream server unavailable")
    })?;

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
    offload_response(move || {
        let json = params.auth.wants_json();
        match cover_art_inner(&state, &params) {
            Ok(resp) => resp,
            Err(e) => SubsonicResponse::error(json, &e),
        }
    })
    .await
}

fn cover_art_inner(state: &AppState, params: &CoverArtParams) -> Result<Response, SubsonicError> {
    let db = authed_db(state, &params.auth)?;
    let (kind, id) = require_entity(&db, params.id.as_deref())?;
    // Snapped to the sizes `Covers` keeps; no size asks for the largest.
    let size = crate::covers::snap(Some(params.size.unwrap_or(u32::MAX)));
    let groups = cover_tracks(&db, kind, id)?;
    drop(db);
    let bytes = groups
        .iter()
        .find_map(|tracks| state.covers.cover(tracks, size))
        .ok_or_else(|| SubsonicError::not_found("Cover art"))?;
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "image/jpeg"),
            (header::CACHE_CONTROL, "max-age=86400"),
        ],
        bytes,
    )
        .into_response())
}

/// The tracks whose art answers a `getCoverArt` id, as groups tried in turn:
/// an album's tracks, a song alone, or an artist's albums one by one. A
/// track's art is the cover image beside it or, without one, its embedded art.
fn cover_tracks(
    db: &Database,
    kind: Option<EntityKind>,
    id: i64,
) -> Result<Vec<Vec<queries::TrackRow>>, SubsonicError> {
    let internal = |e: koan_core::db::connection::DbError| SubsonicError::internal(e.to_string());
    let groups = match kind {
        Some(EntityKind::Album) => {
            let tracks = queries::tracks_for_album(&db.conn, id).map_err(internal)?;
            if tracks.is_empty() {
                return Err(SubsonicError::not_found("Album"));
            }
            vec![tracks]
        }
        Some(EntityKind::Artist) => {
            // Every album's tracks in one query, then grouped in the order the
            // albums are tried.
            let albums: Vec<i64> = queries::albums_for_artist(&db.conn, id)
                .map_err(internal)?
                .iter()
                .map(|album| album.id)
                .collect();
            let mut tracks =
                queries::batch::tracks_for_albums(&db.conn, &albums).map_err(internal)?;
            let groups: Vec<_> = albums
                .iter()
                .filter_map(|album| tracks.remove(album))
                .filter(|tracks| !tracks.is_empty())
                .collect();
            if groups.is_empty() {
                return Err(SubsonicError::not_found("Artist"));
            }
            groups
        }
        Some(EntityKind::Song) | None => vec![vec![
            queries::get_track_row(&db.conn, id)
                .map_err(internal)?
                .ok_or_else(|| SubsonicError::not_found("Track"))?,
        ]],
    };
    Ok(groups)
}

// ===========================================================================
// Endpoints — interaction (star, unstar, scrobble, etc.)
// ===========================================================================

async fn star(State(state): State<Arc<AppState>>, RawQuery(raw): RawQuery) -> Response {
    set_starred(state, raw, true).await
}

async fn unstar(State(state): State<Arc<AppState>>, RawQuery(raw): RawQuery) -> Response {
    set_starred(state, raw, false).await
}

/// `star` and `unstar`. `id`, `albumId` and `artistId` may each repeat, and
/// an `id` may name an album or artist, by its uid or prefix, as well as a song.
async fn set_starred(state: Arc<AppState>, raw: Option<String>, star: bool) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let auth = params.auth();
        respond_db_caller(&state, &auth, Role::User, |db, caller, b| {
            let user = caller.user_id;
            let mut targets = Vec::new();
            for raw in params.all("id") {
                let (kind, id) = resolve_entity(db, raw)?;
                targets.push((kind.unwrap_or(EntityKind::Song), id));
            }
            for (key, kind) in [
                ("albumId", EntityKind::Album),
                ("artistId", EntityKind::Artist),
            ] {
                for raw in params.all(key) {
                    targets.push((kind, resolve_as(db, raw, kind, key)?));
                }
            }
            if targets.is_empty() {
                return Err(SubsonicError::missing_param("id"));
            }
            let songs = targets.iter().any(|(kind, _)| *kind == EntityKind::Song);
            for (kind, id) in targets {
                set_star(db, user, kind, id, star)?;
            }
            if songs {
                crate::clients::smart_activity(db, user, &[koan_core::smart::Field::Favourite]);
            }
            // The caller's other apps show hearts too: a track favourited on
            // a phone while it plays on the Mac should light up there.
            crate::clients::registry().broadcast(
                Some(&caller.username),
                koan_core::remote::link::LinkCommand::Sync { full: false },
            );
            Ok(b)
        })
    })
    .await
}

/// Favourite one artist, record or track for `user`, or stop, as `star` and
/// `unstar` do, and tell the account's apps to pick it up. What the web UI's
/// hearts call, so a heart there is the same favourite an app makes.
pub(crate) fn favourite(
    db: &Database,
    user: i64,
    username: &str,
    kind: EntityKind,
    id: i64,
    star: bool,
) -> Result<(), String> {
    set_star(db, user, kind, id, star).map_err(|e| e.message)?;
    crate::clients::registry().broadcast(
        Some(username),
        koan_core::remote::link::LinkCommand::Sync { full: false },
    );
    Ok(())
}

fn set_star(
    db: &Database,
    user: i64,
    kind: EntityKind,
    id: i64,
    star: bool,
) -> Result<(), SubsonicError> {
    let internal = |e: rusqlite::Error| SubsonicError::internal(e.to_string());
    match kind {
        EntityKind::Song => {
            queries::get_track_row(&db.conn, id)
                .map_err(|e| SubsonicError::internal(e.to_string()))?
                .ok_or_else(|| SubsonicError::not_found("Track"))?;
            let op = if star {
                queries::add_favourite
            } else {
                queries::remove_favourite
            };
            op(&db.conn, user, id).map_err(internal)?;
            // The upstream server has one account, and it is the local user's.
            if queries::auth::is_local_user(&db.conn, user).map_err(internal)? {
                koan_core::helpers::sync_favourite_to_remote(db, id, star);
            }
        }
        EntityKind::Album => {
            queries::get_album(&db.conn, id)
                .map_err(|e| SubsonicError::internal(e.to_string()))?
                .ok_or_else(|| SubsonicError::not_found("Album"))?;
            queries::set_favourite_album(&db.conn, user, id, star).map_err(internal)?;
        }
        EntityKind::Artist => {
            queries::get_artist(&db.conn, id)
                .map_err(|e| SubsonicError::internal(e.to_string()))?
                .ok_or_else(|| SubsonicError::not_found("Artist"))?;
            queries::set_favourite_artist(&db.conn, user, id, star).map_err(internal)?;
        }
    }
    Ok(())
}

/// `setRating`: `rating` 1 to 5 rates the song, album or artist `id` names,
/// by its uid or prefix; 0 clears it.
async fn set_rating(State(state): State<Arc<AppState>>, RawQuery(raw): RawQuery) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let auth = params.auth();
        respond_db_user(&state, &auth, Role::User, |db, user, b| {
            let raw = params
                .get("id")
                .ok_or_else(|| SubsonicError::missing_param("id"))?;
            let rating = params
                .get("rating")
                .ok_or_else(|| SubsonicError::missing_param("rating"))?
                .parse::<u8>()
                .ok()
                .filter(|r| *r <= 5)
                .ok_or_else(|| SubsonicError::bad_param("rating"))?;
            let (kind, id) = resolve_entity(db, raw)?;
            let internal = |e: rusqlite::Error| SubsonicError::internal(e.to_string());
            let kind = match kind.unwrap_or(EntityKind::Song) {
                EntityKind::Song => {
                    queries::get_track_row(&db.conn, id)
                        .map_err(|e| SubsonicError::internal(e.to_string()))?
                        .ok_or_else(|| SubsonicError::not_found("Track"))?;
                    queries::RatingKind::Track
                }
                EntityKind::Album => {
                    queries::get_album(&db.conn, id)
                        .map_err(|e| SubsonicError::internal(e.to_string()))?
                        .ok_or_else(|| SubsonicError::not_found("Album"))?;
                    queries::RatingKind::Album
                }
                EntityKind::Artist => {
                    queries::get_artist(&db.conn, id)
                        .map_err(|e| SubsonicError::internal(e.to_string()))?
                        .ok_or_else(|| SubsonicError::not_found("Artist"))?;
                    queries::RatingKind::Artist
                }
            };
            queries::set_rating(&db.conn, user, kind, id, rating).map_err(internal)?;
            Ok(b)
        })
    })
    .await
}

async fn get_starred2(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    offload_response(move || {
        respond_db_user(&state, &params, Role::Readonly, |db, user, b| {
            // A query per kind, not per favourite: clients read this on every sync.
            let tracks = queries::favourite_tracks(&db.conn, user, None)
                .map_err(|e| SubsonicError::internal(e.to_string()))?;
            let extras = song_extras(db, user, &tracks)?;

            let albums = queries::list_albums(
                &db.conn,
                &queries::AlbumQuery {
                    favourites_of: Some(user),
                    ..Default::default()
                },
            )
            .map_err(|e| SubsonicError::internal(e.to_string()))?;
            // By id rather than `list_artists`, which lists only artists who
            // own an album: a favourited guest artist would drop out.
            let artist_ids = queries::favourite_artist_id_set(&db.conn, user)
                .map_err(|e| SubsonicError::internal(e.to_string()))?;
            let artists = by_id(
                db,
                "SELECT id, name FROM artists WHERE id IN (SELECT value FROM json_each(?1)) ORDER BY id",
                [json_ids(artist_ids)],
                |r| Ok((r.get(0)?, r.get::<_, String>(1)?)),
            )?;
            let album_extras = album_extras(db, user, &albums, None)?;
            let artist_extras = artist_extras(db, user, artists.iter().map(|(id, _)| *id))?;

            Ok(b.child(
                XmlNode::new("starred2")
                    .list(
                        "artist",
                        artists
                            .iter()
                            .map(|(id, name)| artist_id3_node(*id, name, &artist_extras)),
                    )
                    .list(
                        "album",
                        albums.iter().map(|a| album_to_xml_node(a, &album_extras)),
                    )
                    .list("song", tracks.iter().map(|t| track_to_xml_node(t, &extras))),
            ))
        })
    })
    .await
}

/// `id` may repeat, each with its own `time`, since 1.8.0: a client flushing
/// an offline session sends every play at once.
async fn scrobble(State(state): State<Arc<AppState>>, RawQuery(raw): RawQuery) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let auth = params.auth();
        respond_db_caller(&state, &auth, Role::User, |db, caller, b| {
            let user = caller.user_id;
            // An offline session flushes hundreds at once: the uids among
            // them are resolved in one query rather than one each.
            let raws: Vec<&str> = params.all("id").collect();
            let by_uid = queries::ids_for_uids(
                &db.conn,
                queries::UidKind::Track,
                raws.iter()
                    .copied()
                    .filter(|raw| parse_entity_id(raw).is_none()),
            )?;
            let track_ids: Vec<i64> = raws
                .iter()
                .map(|raw| match parse_entity_id(raw) {
                    Some((_, id)) => Ok(id),
                    None if !queries::is_uid(raw) => Err(SubsonicError::bad_param("id")),
                    None => by_uid
                        .get(*raw)
                        .copied()
                        .ok_or_else(|| SubsonicError::not_found(EntityKind::Song.noun())),
                })
                .collect::<Result<_, _>>()?;
            if track_ids.is_empty() {
                return Err(SubsonicError::missing_param("id"));
            }

            // `submission=false` is a now-playing notice, not a play.
            if params.get("submission") == Some("false") {
                if let (Ok(user), Some(&track_id)) = (
                    queries::auth::resolve_user(&db.conn, user),
                    track_ids.first(),
                ) {
                    koan_core::scrobbling::now_playing(user, track_id);
                }
                return Ok(b);
            }

            // `time` is when the client played it, which can be well in the past
            // after an offline session.
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            let times: Vec<Option<i64>> = params.all("time").map(|t| t.parse().ok()).collect();
            let plays: Vec<(i64, i64)> = track_ids
                .iter()
                .enumerate()
                .map(|(i, &track_id)| {
                    let played_at = times
                        .get(i)
                        .copied()
                        .flatten()
                        .map_or(now, |time_ms| time_ms / 1000);
                    (track_id, played_at)
                })
                .collect();
            // The foreign key is the existence check: one id that names no
            // track fails the batch, and the transaction leaves none of it.
            match queries::record_plays_at(&db.conn, user, &plays, queries::SOURCE_SUBSONIC) {
                Ok(()) => {
                    koan_core::scrobbling::wake();
                    use koan_core::smart::Field;
                    crate::clients::smart_activity(
                        db,
                        user,
                        &[Field::PlayCount, Field::LastPlayed],
                    );
                    history_changed(&caller.username);
                    Ok(b)
                }
                Err(koan_core::db::connection::DbError::Sqlite(
                    rusqlite::Error::SqliteFailure(e, _),
                )) if e.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY => {
                    Err(SubsonicError::not_found("Track"))
                }
                Err(e) => Err(SubsonicError::from(format!("Database error: {}", e))),
            }
        })
    })
    .await
}

/// Tell the account's linked apps that its play history moved, so each reads
/// what changed (`koanHistory`).
fn history_changed(username: &str) {
    crate::clients::registry().broadcast(
        Some(username),
        koan_core::remote::link::LinkCommand::HistoryChanged,
    );
}

/// The caller's play history after `since` (`play.forgotten`, from the last
/// page; absent for the start), at most `count` plays and `count`
/// forgettings: koan's `koanHistory`. Times are ms since the epoch.
async fn koan_history(State(state): State<Arc<AppState>>, RawQuery(raw): RawQuery) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let auth = params.auth();
        respond_db_user(&state, &auth, Role::Readonly, |db, user, b| {
            let since = match params.get("since") {
                Some(raw) => queries::HistoryCursor::parse(raw)
                    .ok_or_else(|| SubsonicError::bad_param("since"))?,
                None => queries::HistoryCursor::default(),
            };
            let count = params
                .get("count")
                .and_then(|c| c.parse::<u32>().ok())
                .unwrap_or(500)
                .clamp(1, 1000);
            let page = queries::history_since(&db.conn, user, since, count)
                .map_err(|e| SubsonicError::internal(e.to_string()))?;
            Ok(b.child(
                XmlNode::new("koanHistory")
                    .attr("cursor", &page.cursor.to_string())
                    .attr_bool("more", page.more)
                    .list(
                        "play",
                        page.plays.iter().map(|p| {
                            XmlNode::new("play")
                                .attr("id", &p.track_uid)
                                .attr_int("seq", p.seq)
                                .attr_int("played", p.played_at * 1000)
                                .attr_opt_int("listenedMs", p.listened_ms)
                        }),
                    )
                    .list(
                        "forgotten",
                        page.forgotten.iter().map(|f| {
                            XmlNode::new("forgotten")
                                .attr_opt("id", f.track_uid.as_deref())
                                .attr_int("played", f.played_at * 1000)
                        }),
                    ),
            ))
        })
    })
    .await
}

/// Forget plays from the caller's history, for every one of the account's
/// devices: each `id` with its `time` (ms, when the play started, as
/// `scrobble` takes it), or with `through` every play up to then. koan's
/// `koanForgetPlays`.
async fn koan_forget_plays(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let auth = params.auth();
        respond_db_caller(&state, &auth, Role::User, |db, caller, b| {
            let failed =
                |e: koan_core::db::connection::DbError| SubsonicError::internal(e.to_string());
            if let Some(through) = params.get("through") {
                let through: i64 = through
                    .parse()
                    .map_err(|_| SubsonicError::bad_param("through"))?;
                queries::forget_shared_plays_through(&db.conn, caller.user_id, through / 1000)
                    .map_err(failed)?;
            } else {
                let ids: Vec<&str> = params.all("id").collect();
                let times: Vec<i64> = params
                    .all("time")
                    .map(|t| t.parse().map_err(|_| SubsonicError::bad_param("time")))
                    .collect::<Result<_, _>>()?;
                if ids.is_empty() {
                    return Err(SubsonicError::missing_param("id"));
                }
                if ids.len() != times.len() {
                    return Err(SubsonicError::missing_param("time"));
                }
                // A track that has left the library took its plays with it.
                let plays: Vec<(i64, i64)> = ids
                    .iter()
                    .zip(&times)
                    .filter_map(|(raw, time)| {
                        resolve_as(db, raw, EntityKind::Song, "id")
                            .ok()
                            .map(|track| (track, time / 1000))
                    })
                    .collect();
                queries::forget_shared_plays(&db.conn, caller.user_id, &plays).map_err(failed)?;
            }
            use koan_core::smart::Field;
            crate::clients::smart_activity(
                db,
                caller.user_id,
                &[Field::PlayCount, Field::LastPlayed],
            );
            history_changed(&caller.username);
            Ok(b)
        })
    })
    .await
}

async fn get_random_songs(
    State(state): State<Arc<AppState>>,
    Query(params): Query<RandomSongsParams>,
) -> Response {
    offload_response(move || {
        respond_db_user(&state, &params.auth, Role::Readonly, |db, user, b| {
            // Capped as getAlbumList is: every song is built in memory.
            let size = params.size.unwrap_or(10).min(500);
            let filter = queries::RandomFilter {
                genre: params.genre.as_deref(),
                year_from: params.from_year,
                year_to: params.to_year,
                ..Default::default()
            };
            let tracks = queries::random_tracks_where(&db.conn, size, &filter)
                .map_err(|e| SubsonicError::internal(e.to_string()))?;
            let extras = song_extras(db, user, &tracks)?;
            Ok(b.child(
                XmlNode::new("randomSongs")
                    .list("song", tracks.iter().map(|t| track_to_xml_node(t, &extras))),
            ))
        })
    })
    .await
}

// ---------------------------------------------------------------------------
// Endpoints — server/user metadata
// ---------------------------------------------------------------------------

async fn get_music_folders(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    offload_response(move || {
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
    })
    .await
}

/// Clients call this during setup to decide which features to offer. It
/// reports the caller's own roles, whatever `username` asks about.
async fn get_user(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    offload_response(move || {
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
    })
    .await
}

/// Answered without authentication, as OpenSubsonic requires: a client asks
/// before it knows which sign-in methods it may use.
async fn get_open_subsonic_extensions(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    // `timeOffset` seeks a transcode, so it is offered only where one can run.
    let transcode: &[(&str, &[i64])] = if state.transcoder.is_some() {
        &[("transcodeOffset", &[1])]
    } else {
        &[]
    };
    SubsonicResponse::ok(params.wants_json())
        .list(
            "openSubsonicExtensions",
            EXTENSIONS.iter().chain(transcode).map(|(name, versions)| {
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
    offload_response(move || {
        respond(&state, &params, |caller, b| {
            Ok(b.child(XmlNode::new("tokenInfo").attr("username", &caller.username)))
        })
    })
    .await
}

/// Scans are driven by `koan scan`, never by a client, so this only ever
/// reports the library size. Clients poll it after a connection test.
async fn get_scan_status(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    offload_response(move || {
        respond_db(&state, &params, |db, b| {
            // Clients poll this; the track count is all it reports.
            let count: i64 = db
                .conn
                .query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))
                .map_err(|e| SubsonicError::internal(e.to_string()))?;
            Ok(b.child(
                XmlNode::new("scanStatus")
                    .attr_bool("scanning", false)
                    .attr_int("count", count),
            ))
        })
    })
    .await
}

async fn get_genres(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    offload_response(move || {
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
    })
    .await
}

// ---------------------------------------------------------------------------
// Playlist endpoints
// ---------------------------------------------------------------------------

async fn get_playlists(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    offload_response(move || {
        respond_db_user(&state, &params, Role::Readonly, |db, user, b| {
            refresh_smart(db, user);
            let lists = queries::list_playlists(&db.conn, user)
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
    })
    .await
}

/// The attributes every `<playlist>` carries, list or detail.
fn playlist_attrs(node: XmlNode, list: &queries::PlaylistRow, username: &str) -> XmlNode {
    let node = node
        .attr("id", &list.uid)
        .attr("name", &list.name)
        .attr_int("songCount", list.track_count)
        .attr_int("duration", list.duration_ms / 1000)
        .attr("owner", list.owner.as_deref().unwrap_or(username))
        .attr_bool("public", list.public)
        .attr("created", &list.created_at)
        .attr("changed", &list.changed_at)
        // OpenSubsonic: a smart playlist takes no edits to its contents.
        .attr_bool("readonly", list.readonly);
    match &list.comment {
        Some(comment) => node.attr("comment", comment),
        None => node,
    }
}

/// A playlist as a `<playlist>` with its members resolved, for `user`.
fn playlist_node(
    db: &Database,
    user: i64,
    list: &queries::PlaylistRow,
    owner: &str,
) -> Result<XmlNode, SubsonicError> {
    let mut node = playlist_attrs(XmlNode::new("playlist"), list, owner).array_of("entry");

    // Playlist members are `<entry>`, not `<song>` — an XML client shown
    // `<song>` sees an empty playlist.
    let tracks = queries::playlist_tracks(&db.conn, list.id)
        .map_err(|e| SubsonicError::internal(e.to_string()))?;
    let extras = song_extras(db, user, &tracks)?;
    for track in &tracks {
        node = node.child(track_node(track, "entry", &extras));
    }

    Ok(node)
}

/// Playlist `id` if `user` may see it (their own, or a public one) — and, with
/// `edit`, change it (their own only).
fn playlist_for(
    db: &Database,
    user: i64,
    id: i64,
    edit: bool,
) -> Result<queries::PlaylistRow, SubsonicError> {
    let me = queries::auth::resolve_user(&db.conn, user)
        .map_err(|e| SubsonicError::internal(e.to_string()))?;
    let list = queries::get_playlist(&db.conn, id)
        .map_err(|e| SubsonicError::internal(e.to_string()))?
        .filter(|p| p.readable_by(me))
        .ok_or_else(|| SubsonicError::not_found("Playlist"))?;
    if edit && !list.editable_by(me) {
        return Err(SubsonicError::not_authorized());
    }
    Ok(list)
}

/// Evaluate the smart playlists `user` can see that are due, and have every
/// device pull any whose contents moved. A failure leaves the last contents
/// in place, which is what a read should serve anyway.
fn refresh_smart(db: &Database, user: i64) {
    match queries::smart::refresh_due(&db.conn, user) {
        Ok(changed) if !changed.is_empty() => crate::clients::changed(),
        Ok(_) => {}
        Err(e) => log::warn!("smart playlists not refreshed: {e}"),
    }
}

/// A smart playlist's contents are its rules', and a file's are the file's:
/// neither is for editing.
fn refuse_smart(list: &queries::PlaylistRow) -> Result<(), SubsonicError> {
    if list.readonly {
        return Err(SubsonicError::new(
            SubsonicErrorCode::NotAuthorized,
            "This playlist is read-only: its rules or its file decide what it holds",
        ));
    }
    Ok(())
}

/// After a playlist write: push it to the upstream, if there is one, and have
/// the account's devices pull it, as the GraphQL mutations do. A koan app's
/// own edits arrive through these endpoints.
fn playlist_changed(id: i64) {
    koan_core::playlists::push_to_remote(id);
    crate::clients::changed();
}

/// A playlist by its uid, or by the row id clients from before uids hold.
fn playlist_id(db: &Database, raw: Option<&str>) -> Result<i64, SubsonicError> {
    let raw = raw.ok_or_else(|| SubsonicError::missing_param("id"))?;
    queries::resolve_id(&db.conn, queries::UidKind::Playlist, raw)
        .map_err(|e| SubsonicError::internal(e.to_string()))?
        .ok_or_else(|| SubsonicError::not_found("Playlist"))
}

async fn get_playlist(
    State(state): State<Arc<AppState>>,
    Query(params): Query<IdParam>,
) -> Response {
    offload_response(move || {
        respond_db_user(&state, &params.auth, Role::Readonly, |db, user, b| {
            let id = playlist_id(db, params.id.as_deref())?;
            playlist_for(db, user, id, false)?;
            match queries::smart::refresh_if_due(&db.conn, id) {
                Ok(true) => crate::clients::changed(),
                Ok(false) => {}
                Err(e) => log::warn!("smart playlist {id} not refreshed: {e}"),
            }
            let list = playlist_for(db, user, id, false)?;
            Ok(b.child(playlist_node(db, user, &list, &state.username)?))
        })
    })
    .await
}

/// `createPlaylist` — new when given a `name`, a wholesale replacement when
/// given a `playlistId`. That second form is the only Subsonic call that can
/// set a playlist's order, which is why koan's own pushes use it too.
async fn create_playlist(State(state): State<Arc<AppState>>, RawQuery(raw): RawQuery) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let auth = params.auth();

        respond_db_user(&state, &auth, Role::User, |db, user, b| {
            let track_ids = song_ids(db, params.all("songId"));

            // One transaction, so no reader sees the name without the tracks.
            let id = queries::atomically(&db.conn, || match params.get("playlistId") {
                Some(existing) => {
                    let id = playlist_id(db, Some(existing))?;
                    refuse_smart(&playlist_for(db, user, id, true)?)?;
                    if let Some(name) = params.get("name") {
                        queries::rename_playlist(&db.conn, id, name)
                            .map_err(|e| SubsonicError::internal(e.to_string()))?;
                    }
                    queries::set_playlist_tracks(&db.conn, id, &track_ids)
                        .map_err(|e| SubsonicError::internal(e.to_string()))?;
                    Ok(id)
                }
                None => {
                    let name = params
                        .get("name")
                        .ok_or_else(|| SubsonicError::missing_param("name"))?;
                    let id = queries::create_playlist(&db.conn, user, name, None)
                        .map_err(|e| SubsonicError::internal(e.to_string()))?;
                    queries::add_tracks(&db.conn, id, &track_ids)
                        .map_err(|e| SubsonicError::internal(e.to_string()))?;
                    Ok::<_, SubsonicError>(id)
                }
            })?;

            playlist_changed(id);
            // Since 1.14.0 the response carries the playlist that was created;
            // clients read the id back off it rather than guessing.
            let list = playlist_for(db, user, id, false)?;
            Ok(b.child(playlist_node(db, user, &list, &state.username)?))
        })
    })
    .await
}

/// `updatePlaylist` — rename, re-comment, and add or remove members by index.
///
/// Removals are applied by descending index so each one does not shift the
/// next; a client sends them against the list as it stood when it asked.
async fn update_playlist(State(state): State<Arc<AppState>>, RawQuery(raw): RawQuery) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let auth = params.auth();

        respond_db_user(&state, &auth, Role::User, |db, user, b| {
            let id = playlist_id(db, params.get("playlistId").or_else(|| params.get("id")))?;
            let list = playlist_for(db, user, id, true)?;
            if params.get("name").is_some() && list.source_path.is_some() {
                return Err(SubsonicError::new(
                    SubsonicErrorCode::NotAuthorized,
                    "This playlist is named by its file in the library: rename the file",
                ));
            }
            if params.all("songIdToAdd").next().is_some()
                || params.all("songIndexToRemove").next().is_some()
            {
                refuse_smart(&list)?;
            }
            let added = song_ids(db, params.all("songIdToAdd"));

            // One transaction: the indexes to remove are against the list as
            // the client last saw it, and another edit landing between the
            // read and the write would shift them.
            queries::atomically(&db.conn, || {
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

                if !added.is_empty() {
                    queries::add_tracks(&db.conn, id, &added)
                        .map_err(|e| SubsonicError::internal(e.to_string()))?;
                }
                Ok::<_, SubsonicError>(())
            })?;

            playlist_changed(id);
            Ok(b)
        })
    })
    .await
}

async fn delete_playlist(
    State(state): State<Arc<AppState>>,
    Query(params): Query<IdParam>,
) -> Response {
    offload_response(move || {
        respond_db_user(&state, &params.auth, Role::User, |db, user, b| {
            let id = playlist_id(db, params.id.as_deref())?;
            // Read before deleting: the delete has to reach the upstream too,
            // or its next sync brings the playlist back.
            let remote_id = playlist_for(db, user, id, true)?.remote_id;
            match queries::delete_playlist(&db.conn, id) {
                Ok(true) => {
                    if let Some(remote_id) = remote_id {
                        koan_core::playlists::delete_on_remote(remote_id);
                    }
                    crate::clients::changed();
                    Ok(b)
                }
                Ok(false) => Err(SubsonicError::not_found("Playlist")),
                Err(e) => Err(SubsonicError::internal(e.to_string())),
            }
        })
    })
    .await
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
    user: i64,
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
    let extras = song_extras(db, user, &rows)?;
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
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let auth = params.auth();
        respond_db_user(&state, &auth, Role::User, |db, user, b| {
            let base = share_base()?;
            let ids: Vec<_> = params
                .all("id")
                .map(|raw| resolve_entity(db, raw))
                .collect::<Result<_, _>>()?;
            let target = share_target(db, &ids)?;
            let (slice, track_ids) =
                koan_core::helpers::resolve_share(&db.conn, &target).map_err(|e| match e {
                    koan_core::helpers::ShareError::NothingToShare => {
                        SubsonicError::not_found("Song")
                    }
                    e => SubsonicError::internal(e.to_string()),
                })?;
            let now = chrono::Utc::now().timestamp();
            let share = queries::shares::create_share(
                &db.conn,
                user,
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
                    user,
                    &share,
                    &base,
                    &state.username,
                )?)),
            )
        })
    })
    .await
}

async fn get_shares(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    offload_response(move || {
        respond_db_caller(&state, &params, Role::Readonly, |db, caller, b| {
            let base = share_base()?;
            let mut node = XmlNode::new("shares").array_of("share");
            for share in queries::shares::list_shares(&db.conn, caller.share_owner())
                .map_err(|e| SubsonicError::internal(e.to_string()))?
            {
                node = node.child(share_node(
                    db,
                    caller.user_id,
                    &share,
                    &base,
                    &state.username,
                )?);
            }
            Ok(b.child(node))
        })
    })
    .await
}

async fn update_share(State(state): State<Arc<AppState>>, RawQuery(raw): RawQuery) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let auth = params.auth();
        respond_db_caller(&state, &auth, Role::User, |db, caller, b| {
            let id = params
                .get("id")
                .ok_or_else(|| SubsonicError::missing_param("id"))?;
            let found = queries::shares::update_share(
                &db.conn,
                caller.share_owner(),
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
    })
    .await
}

async fn delete_share(State(state): State<Arc<AppState>>, RawQuery(raw): RawQuery) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let auth = params.auth();
        respond_db_caller(&state, &auth, Role::User, |db, caller, b| {
            let id = params
                .get("id")
                .ok_or_else(|| SubsonicError::missing_param("id"))?;
            let found = queries::shares::delete_share(&db.conn, caller.share_owner(), id)
                .map_err(|e| SubsonicError::internal(e.to_string()))?;
            if found {
                Ok(b)
            } else {
                Err(SubsonicError::not_found("Share"))
            }
        })
    })
    .await
}

// ---------------------------------------------------------------------------
// Accounts (koan extension, admins only)
// ---------------------------------------------------------------------------
//
// What the apps manage this server's accounts through. An invite comes back
// as the account's username and a token, with the password when one was just
// made; the app builds the link from the address it already reaches this
// server at. `koanJoin` is the other end: an app trading the token for a key.

fn respond_admin(
    state: &AppState,
    auth: &SubsonicParams,
    f: impl FnOnce(&Database, &Caller, XmlBuilder) -> Result<XmlBuilder, SubsonicError>,
) -> Response {
    let json = auth.wants_json();
    let result = validate_auth(auth, state).and_then(|caller| {
        if caller.role != Role::Admin {
            return Err(SubsonicError::not_authorized());
        }
        let db = state.open_db()?;
        f(&db, &caller, SubsonicResponse::ok(json))
    });
    match result {
        Ok(builder) => builder.build(),
        Err(e) => SubsonicResponse::error(json, &e),
    }
}

fn account_error(e: koan_core::invite::AccountError) -> SubsonicError {
    use koan_core::invite::AccountError;
    match e {
        AccountError::NoSuchUser(_) => SubsonicError::not_found("User"),
        AccountError::Other(e) => SubsonicError::internal(e.to_string()),
        e => SubsonicError::new(SubsonicErrorCode::Generic, e.to_string()),
    }
}

fn role_param(params: &RawParams) -> Result<Role, SubsonicError> {
    params
        .get("role")
        .ok_or_else(|| SubsonicError::missing_param("role"))?
        .parse()
        .map_err(|_| SubsonicError::bad_param("role"))
}

fn username_param(params: &RawParams) -> Result<&str, SubsonicError> {
    params
        .get("username")
        .ok_or_else(|| SubsonicError::missing_param("username"))
}

/// The server's signing keypair: what invite tokens are signed and checked
/// with.
fn keypair() -> Result<&'static crate::auth::Keypair, SubsonicError> {
    crate::auth::signing_keys().map_err(|e| SubsonicError::internal(e.to_string()))
}

fn invite_node(
    conn: &rusqlite::Connection,
    user_id: i64,
    username: &str,
    password: Option<&str>,
) -> Result<XmlNode, SubsonicError> {
    let token =
        koan_core::invite::mint_token(conn, &keypair()?.0, user_id).map_err(account_error)?;
    let node = XmlNode::new("invite")
        .attr("username", username)
        .attr("token", &token);
    Ok(match password {
        Some(p) => node.attr("password", p),
        None => node,
    })
}

async fn koan_users(State(state): State<Arc<AppState>>, RawQuery(raw): RawQuery) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        respond_admin(&state, &params.auth(), |db, _, b| {
            let users = queries::auth::list_users(&db.conn)
                .map_err(|e| SubsonicError::internal(e.to_string()))?;
            Ok(b.child(XmlNode::new("users").list(
                "user",
                users.iter().map(|u| {
                    XmlNode::new("user")
                        .attr("username", &u.username)
                        .attr("role", u.role.as_str())
                }),
            )))
        })
    })
    .await
}

async fn koan_create_user(State(state): State<Arc<AppState>>, RawQuery(raw): RawQuery) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        respond_admin(&state, &params.auth(), |db, _, b| {
            let username = username_param(&params)?;
            let role = role_param(&params)?;
            let made = koan_core::invite::create_account(&db.conn, username, role)
                .map_err(account_error)?;
            Ok(b.child(invite_node(
                &db.conn,
                made.id,
                username.trim(),
                Some(&made.password),
            )?))
        })
    })
    .await
}

async fn koan_invite(State(state): State<Arc<AppState>>, RawQuery(raw): RawQuery) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        respond_admin(&state, &params.auth(), |db, _, b| {
            let username = username_param(&params)?;
            let user = koan_core::invite::account(&db.conn, username).map_err(account_error)?;
            let password = if params.get("reset") == Some("true") {
                let p = koan_core::invite::set_password(&db.conn, username, None)
                    .map_err(account_error)?;
                crate::clients::registry().disconnect(username);
                Some(p)
            } else {
                None
            };
            Ok(b.child(invite_node(
                &db.conn,
                user.id,
                username,
                password.as_deref(),
            )?))
        })
    })
    .await
}

/// Set an account's password: an admin any account's, anyone else their own,
/// given the current one as `current`. The account's sessions, keys, app
/// passwords and links all end, as with any password change. Changing one's
/// own answers as `koanSignIn` does, with a new key named `name`: the key the
/// request came with is among those revoked.
///
/// Signed with an API key or the account's own password only: an app password
/// is a credential handed to one client, and the shared secret is no account's.
///
/// A wrong `current` counts against the account's sign-in budget, as a wrong
/// password at sign-in does, and a spent budget refuses the check from every
/// network, the account's own included: the sign-in throttle spares those, and
/// a check that is not a sign-in must not be repeatable from one without limit.
/// The request is usually signed with a key, which that throttle leaves alone,
/// so the budget is checked here.
///
/// POST only, so the passwords never sit in a URL, where a proxy in front of
/// the server would log them.
async fn koan_set_user_password(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        respond_db_caller(&state, &params.auth(), Role::Readonly, |db, caller, b| {
            caller.may_manage_credentials()?;
            let username = params
                .get("username")
                .map_or(caller.username.as_str(), str::trim);
            let own = username == caller.username;
            if caller.role != Role::Admin {
                if !own {
                    return Err(SubsonicError::new(
                        SubsonicErrorCode::NotAuthorized,
                        "only an admin can set another account's password",
                    ));
                }
                let current = params
                    .get("current")
                    .ok_or_else(|| SubsonicError::missing_param("current"))?;
                if state.users.exhausted(username) {
                    return Err(SubsonicError::new(
                        SubsonicErrorCode::Generic,
                        "Too many failed sign-ins for this account; try again in a minute",
                    ));
                }
                use crate::auth::password::Refused;
                match state.users.verify(username, current) {
                    Ok(_) => {}
                    Err(Refused::Busy) => return Err(SubsonicError::busy()),
                    Err(Refused::Wrong) => {
                        state.users.failed(username);
                        // Not error 40, which an app reads as its own sign-in
                        // failing.
                        return Err(SubsonicError::new(
                            SubsonicErrorCode::Generic,
                            "the current password is wrong",
                        ));
                    }
                }
            }
            let password = params
                .get("password")
                .ok_or_else(|| SubsonicError::missing_param("password"))?;
            koan_core::invite::set_password(&db.conn, username, Some(password))
                .map_err(account_error)?;
            crate::clients::registry().disconnect(username);
            if !own {
                return Ok(b);
            }
            let name = koan_core::invite::device_name(params.get("name").unwrap_or_default());
            let (_, api_key) = queries::api_keys::replace_api_key(&db.conn, caller.user_id, &name)
                .map_err(|e| SubsonicError::internal(e.to_string()))?;
            Ok(b.child(
                XmlNode::new("join")
                    .attr("username", &caller.username)
                    .attr("apiKey", &api_key),
            ))
        })
    })
    .await
}

/// The caller's API keys, with the one the request signed in with marked
/// `current`. Never the keys: only their hashes are kept.
async fn koan_api_keys(State(state): State<Arc<AppState>>, RawQuery(raw): RawQuery) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let auth = params.auth();
        respond_db_caller(&state, &auth, Role::Readonly, |db, caller, b| {
            caller.may_manage_credentials()?;
            let internal = |e: rusqlite::Error| SubsonicError::internal(e.to_string());
            let keys = queries::api_keys::list_api_keys(&db.conn, Some(caller.user_id))
                .map_err(internal)?;
            let current = match auth.api_key.as_deref() {
                Some(key) => queries::api_keys::id_of(&db.conn, key).map_err(internal)?,
                None => None,
            };
            Ok(b.child(XmlNode::new("apiKeys").list(
                "apiKey",
                keys.iter().map(|k| api_key_node(k, current == Some(k.id))),
            )))
        })
    })
    .await
}

fn api_key_node(key: &queries::api_keys::ApiKeyRow, current: bool) -> XmlNode {
    XmlNode::new("apiKey")
        .attr_int("id", key.id)
        .attr("name", &key.name)
        .attr("created", &iso(key.created_at))
        .attr_opt("lastUsed", key.last_used_at.map(iso).as_deref())
        .attr_bool("current", current)
}

/// Make an API key named `name` for the caller, for another Subsonic app. The
/// answer carries the key, once.
async fn koan_create_api_key(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        respond_db_caller(&state, &params.auth(), Role::Readonly, |db, caller, b| {
            caller.may_manage_credentials()?;
            let name = params
                .get("name")
                .map(koan_core::invite::device_name)
                .ok_or_else(|| SubsonicError::missing_param("name"))?;
            let (id, key) = queries::api_keys::create_api_key(&db.conn, caller.user_id, &name)
                .map_err(|e| SubsonicError::internal(e.to_string()))?;
            Ok(b.child(
                XmlNode::new("apiKey")
                    .attr_int("id", id)
                    .attr("name", &name)
                    .attr("key", &key),
            ))
        })
    })
    .await
}

/// Revoke one of the caller's API keys, by `id`. Not the one the request is
/// signed in with: that is signing out, which the app does itself.
async fn koan_revoke_api_key(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let auth = params.auth();
        respond_db_caller(&state, &auth, Role::Readonly, |db, caller, b| {
            caller.may_manage_credentials()?;
            let internal = |e: rusqlite::Error| SubsonicError::internal(e.to_string());
            let id: i64 = params
                .get("id")
                .ok_or_else(|| SubsonicError::missing_param("id"))?
                .parse()
                .map_err(|_| SubsonicError::bad_param("id"))?;
            if let Some(key) = auth.api_key.as_deref()
                && queries::api_keys::id_of(&db.conn, key).map_err(internal)? == Some(id)
            {
                return Err(SubsonicError::new(
                    SubsonicErrorCode::Generic,
                    "this device signs in with that key; sign out instead",
                ));
            }
            if !queries::api_keys::revoke_api_key(&db.conn, id, Some(caller.user_id))
                .map_err(internal)?
            {
                return Err(SubsonicError::not_found("API key"));
            }
            Ok(b)
        })
    })
    .await
}

/// Revoke the API key the request is signed in with: for an app giving up a
/// key it no longer holds, such as the one a join replaced.
async fn koan_revoke_key(State(state): State<Arc<AppState>>, RawQuery(raw): RawQuery) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let auth = params.auth();
        respond_db_caller(&state, &auth, Role::Readonly, |db, _, b| {
            let key = auth.api_key.as_deref().ok_or_else(|| {
                SubsonicError::new(SubsonicErrorCode::Generic, "sign in with the key to revoke")
            })?;
            queries::api_keys::revoke_api_key_value(&db.conn, key)
                .map_err(|e| SubsonicError::internal(e.to_string()))?;
            Ok(b)
        })
    })
    .await
}

/// Trade the account's password, proved by this request, for an API key
/// named `name`: what a koan app does once on signing in with a password, so
/// it keeps a key of the device's own and never sends the password again.
/// Answers as `koanJoin` does. Only the account's own password will do: a key
/// or app password is already a credential of its own, and the shared secret
/// is no account's. The key replaces any of the account's with the same name,
/// so a device that signs in again holds the one key.
async fn koan_sign_in(State(state): State<Arc<AppState>>, RawQuery(raw): RawQuery) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let auth = params.auth();
        respond_db_caller(&state, &auth, Role::Readonly, |db, caller, b| {
            if caller.via != Via::Password {
                return Err(SubsonicError::new(
                    SubsonicErrorCode::NotAuthorized,
                    "sign in with the account's own password to be given a key",
                ));
            }
            let name = koan_core::invite::device_name(params.get("name").unwrap_or_default());
            let (_, api_key) = queries::api_keys::replace_api_key(&db.conn, caller.user_id, &name)
                .map_err(|e| SubsonicError::internal(e.to_string()))?;
            Ok(b.child(
                XmlNode::new("join")
                    .attr("username", &caller.username)
                    .attr("apiKey", &api_key),
            ))
        })
    })
    .await
}

/// Trade an invite token for an API key named `name`. The token is the
/// credential, so this alone of koan's endpoints asks for no other.
async fn koan_join(State(state): State<Arc<AppState>>, RawQuery(raw): RawQuery) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let json = params.auth().wants_json();
        let joined = params
            .get("invite")
            .ok_or_else(|| SubsonicError::missing_param("invite"))
            .and_then(|token| {
                let public = &keypair()?.1;
                let db = state.open_db()?;
                koan_core::invite::redeem(
                    &db.conn,
                    public,
                    token,
                    params.get("name").unwrap_or_default(),
                )
                .map_err(account_error)
            });
        match joined {
            Ok(j) => SubsonicResponse::ok(json)
                .child(
                    XmlNode::new("join")
                        .attr("username", &j.username)
                        .attr("apiKey", &j.api_key),
                )
                .build(),
            Err(e) => SubsonicResponse::error(json, &e),
        }
    })
    .await
}

async fn koan_set_user_role(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        respond_admin(&state, &params.auth(), |db, _, b| {
            koan_core::invite::set_role(&db.conn, username_param(&params)?, role_param(&params)?)
                .map_err(account_error)?;
            Ok(b)
        })
    })
    .await
}

async fn koan_delete_user(State(state): State<Arc<AppState>>, RawQuery(raw): RawQuery) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        respond_admin(&state, &params.auth(), |db, caller, b| {
            let username = username_param(&params)?;
            if username == caller.username {
                return Err(SubsonicError::new(
                    SubsonicErrorCode::Generic,
                    "an account cannot delete itself",
                ));
            }
            koan_core::invite::delete_account(&db.conn, username).map_err(account_error)?;
            crate::clients::registry().disconnect(username);
            Ok(b)
        })
    })
    .await
}

// ---------------------------------------------------------------------------
// Pairing (koan extension)
// ---------------------------------------------------------------------------

/// A pairing as an approver sees it: the device's name, the address it asked
/// from, and whether that address is on a private network.
fn pair_node(info: &crate::pair::PairInfo) -> XmlNode {
    XmlNode::new("pair")
        .attr("device", &info.device)
        .attr("from", &info.from.to_string())
        .attr_bool("local", info.local())
}

/// The device waiting on `pair`, an id or a code, and where it asked from: for
/// an app to ask whether to sign it in.
async fn koan_pair_info(State(state): State<Arc<AppState>>, RawQuery(raw): RawQuery) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        respond(&state, &params.auth(), |_, b| {
            let pair = params
                .get("pair")
                .ok_or_else(|| SubsonicError::missing_param("pair"))?;
            let info = crate::pair::pairings()
                .info(pair)
                .ok_or_else(|| SubsonicError::not_found("Pairing"))?;
            Ok(b.child(pair_node(&info)))
        })
    })
    .await
}

/// Sign the device waiting on `pair` in as the caller, with an API key of its
/// own, or with `decline=true` turn it away. Any role: a device is signed in
/// to the caller's own account, with no more than the caller can do.
async fn koan_pair_approve(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        respond_db_caller(&state, &params.auth(), Role::Readonly, |db, caller, b| {
            let pair = params
                .get("pair")
                .ok_or_else(|| SubsonicError::missing_param("pair"))?;
            let decline = params.get("decline") == Some("true");
            if !decline && caller.user_id == queries::LOCAL_USER {
                return Err(SubsonicError::new(
                    SubsonicErrorCode::Generic,
                    "sign in with an account to sign a device in as it",
                ));
            }
            let info = crate::pair::pairings()
                .settle(&db.conn, pair, caller.user_id, &caller.username, decline)
                .map_err(|e| match e {
                    crate::pair::SettleError::NotFound => SubsonicError::not_found("Pairing"),
                    crate::pair::SettleError::Internal(e) => SubsonicError::internal(e),
                })?;
            Ok(b.child(pair_node(&info)))
        })
    })
    .await
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

/// A koan client linking itself to this server: authenticated like any other
/// call, then held open for as long as the client stays, so the server can
/// send it `LinkCommand`s.
async fn koan_link(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
    ws: axum::extract::WebSocketUpgrade,
    request: axum::extract::Request,
) -> Response {
    // Where the device is, as the rate limits see it: what lets a device wake
    // another account's on the same network.
    let addr = crate::auth::routes::client_ip(&request);
    let params = RawParams::parse(raw.as_deref());
    let json = params.auth().wants_json();
    let mark = koan_core::auth::account_mark();
    let caller = {
        let auth = params.auth();
        match tokio::task::spawn_blocking(move || validate_auth(&auth, &state)).await {
            Ok(Ok(caller)) => caller,
            Ok(Err(e)) => return SubsonicResponse::error(json, &e),
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    };
    let name = params.get("client").unwrap_or("koan").to_owned();
    let platform = params.get("platform").unwrap_or("").to_owned();
    // Without a device id every connection is its own device.
    let device = params
        .get("device")
        .map(str::to_owned)
        .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
    let wants_devices = params.get("devices") == Some("1");
    let lease = crate::auth::Lease {
        user_id: caller.user_id,
        mark,
        expires: None,
    };
    ws.on_upgrade(move |socket| {
        link_session(
            socket,
            caller.username,
            lease,
            LinkPeer {
                name,
                platform,
                device,
                wants_devices,
                addr,
            },
        )
    })
}

/// Who is at the far end of a link.
struct LinkPeer {
    name: String,
    platform: String,
    device: String,
    wants_devices: bool,
    /// The client's address, through a trusted proxy if there is one.
    addr: std::net::IpAddr,
}

/// Hand a link command to another of the caller's devices, in one request:
/// what a phone has when iOS has taken its link down and a button on its lock
/// screen is pressed. `to` is the device's id, `command` the command as the
/// link carries it.
async fn koan_command(State(state): State<Arc<AppState>>, RawQuery(raw): RawQuery) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let auth = params.auth();
        respond(&state, &auth, |caller, b| {
            let to = params
                .get("to")
                .ok_or_else(|| SubsonicError::missing_param("to"))?;
            let command = params
                .get("command")
                .and_then(|c| koan_core::remote::link::parse_command(c).ok())
                .ok_or_else(|| SubsonicError::bad_param("command"))?;
            crate::clients::registry()
                .relay(&caller.username, to, command)
                .map_err(|e| SubsonicError::not_found(&e))?;
            Ok(b)
        })
    })
    .await
}

const LINK_CHECK: Duration = Duration::from_secs(15);
/// Over twice the client's idle ping interval.
const LINK_SILENCE: Duration = Duration::from_secs(100);

/// A link until the device goes, a newer link from it replaces this one, or
/// the account changes (see `Lease`). The device reconnects after that last,
/// as it does after any close, and is authenticated afresh.
async fn link_session(
    mut socket: axum::extract::ws::WebSocket,
    username: String,
    lease: crate::auth::Lease,
    peer: LinkPeer,
) {
    use axum::extract::ws::{CloseFrame, Message, close_code};
    let LinkPeer {
        name,
        platform,
        device,
        wants_devices,
        addr,
    } = peer;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let registry = crate::clients::registry();
    // Registry calls read and write the outbox tables, so they run off the
    // async workers. Each is awaited in turn: a device's reports must land in
    // the order it sent them.
    let id = {
        let (username, name, platform, device) = (
            username.clone(),
            name.clone(),
            platform.clone(),
            device.clone(),
        );
        let registered = tokio::task::spawn_blocking(move || {
            let id = registry.register(&username, &name, &platform, &device, tx, wants_devices);
            // After `register`, which records the device the address is kept on.
            registry.seen_at(&device, &username, addr);
            id
        })
        .await;
        match registered {
            Ok(id) => id,
            Err(_) => return,
        }
    };
    log::info!("link: {name} ({platform}) linked for {username}");
    // A client pings when it has heard nothing for a while. A phone the OS
    // has suspended never closes its socket, so one that goes quiet is gone.
    let mut last_heard = tokio::time::Instant::now();
    let mut check = tokio::time::interval(LINK_CHECK);
    let ended = lease.ended();
    tokio::pin!(ended);
    loop {
        tokio::select! {
            _ = &mut ended => {
                let _ = socket
                    .send(Message::Close(Some(CloseFrame {
                        code: close_code::NORMAL,
                        reason: "sign in again".into(),
                    })))
                    .await;
                break;
            }
            _ = check.tick() => {
                if last_heard.elapsed() > LINK_SILENCE {
                    break;
                }
            }
            cmd = rx.recv() => {
                // None: a newer connection from the same device replaced this one.
                let Some(cmd) = cmd else { break };
                let Ok(text) = serde_json::to_string(&cmd) else { continue };
                if socket.send(Message::Text(text.into())).await.is_err() {
                    break;
                }
            }
            msg = socket.recv() => match msg {
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(msg)) => {
                    last_heard = tokio::time::Instant::now();
                    use koan_core::remote::link::LinkReport;
                    if let Message::Text(text) = msg {
                        match serde_json::from_str(&text) {
                            Ok(LinkReport::State(state)) => {
                                let id = id.clone();
                                let _ = tokio::task::spawn_blocking(move || {
                                    registry.report(&id, state);
                                })
                                .await;
                            }
                            Ok(LinkReport::Push { token, sandbox }) => {
                                let (username, device) = (username.clone(), device.clone());
                                let _ = tokio::task::spawn_blocking(move || {
                                    registry.set_push(&username, &device, &token, sandbox);
                                })
                                .await;
                            }
                            Ok(LinkReport::Command {
                                to,
                                command: koan_core::remote::link::LinkCommand::WatchLevels { on },
                            }) => {
                                registry.watch_levels(&username, &device, &to, on);
                            }
                            // At the analyser's rate: relayed in place, with
                            // no database and no thread hop.
                            Ok(LinkReport::Levels { f }) => {
                                registry.levels(&username, &device, f);
                            }
                            Ok(LinkReport::Command { to, command }) => {
                                // Relaying may push to a phone, which blocks.
                                let (username, device) = (username.clone(), device.clone());
                                tokio::task::spawn_blocking(move || {
                                    if let Err(e) = registry.relay_from(&username, Some(&device), &to, command) {
                                        log::info!("link: relay to {to}: {e}");
                                    }
                                });
                            }
                            Ok(LinkReport::Activity { token, device: shown, sandbox }) => {
                                let activity = token.zip(shown).map(|(t, d)| (t, d, sandbox));
                                let (username, device) = (username.clone(), device.clone());
                                tokio::task::spawn_blocking(move || {
                                    registry.set_activity(&username, &device, activity);
                                });
                            }
                            Ok(LinkReport::Wake { to, notify }) => {
                                let (username, device) = (username.clone(), device.clone());
                                tokio::task::spawn_blocking(move || {
                                    registry.wake(&username, &device, &to, notify);
                                });
                            }
                            Ok(LinkReport::Share { grantee, allow }) => {
                                // Only ever about the device sending it: an
                                // owner shares a device from that device.
                                let (username, device) = (username.clone(), device.clone());
                                tokio::task::spawn_blocking(move || {
                                    let known = koan_core::db::pool::shared()
                                        .get()
                                        .ok()
                                        .and_then(|db| {
                                            koan_core::db::queries::auth::get_user_by_username(&db.conn, &grantee)
                                                .ok()
                                                .flatten()
                                        })
                                        .is_some();
                                    if allow && !known {
                                        log::info!("share: {username} asked to share with {grantee}, who has no account here");
                                        registry.send_shares(
                                            &username,
                                            &device,
                                            Some(format!("There is no account called {grantee} on this server.")),
                                        );
                                        return;
                                    }
                                    if let Err(e) = registry.share(&username, &device, &grantee, allow) {
                                        log::info!("share: {e}");
                                        registry.send_shares(&username, &device, Some(e));
                                    }
                                });
                            }
                            Ok(LinkReport::Forget { device: forgotten }) => {
                                let username = username.clone();
                                tokio::task::spawn_blocking(move || {
                                    if let Err(e) = registry.forget(&username, &forgotten) {
                                        log::info!("devices: {e}");
                                    }
                                });
                            }
                            Ok(LinkReport::Hello(_)) | Err(_) => {}
                        }
                    }
                }
            },
        }
    }
    let _ = tokio::task::spawn_blocking(move || registry.unregister(&id)).await;
    log::info!("link: {name} ({platform}) unlinked for {username}");
}

/// Register all Subsonic REST routes on the given router.
fn register_subsonic_routes(router: axum::Router<Arc<AppState>>) -> axum::Router<Arc<AppState>> {
    router
        // Browsing (ID3)
        .route("/rest/ping", get(ping).post(ping))
        // koan's own: a koan client's standing connection, for the server to
        // command it. See `crate::clients`.
        .route("/rest/koanLink", get(koan_link))
        // A device without a keyboard waiting to be signed in, and the
        // endpoints that sign it in. See `crate::pair`.
        .route("/rest/koanPair", get(crate::pair::route))
        .route(
            "/rest/koanPairInfo",
            get(koan_pair_info).post(koan_pair_info),
        )
        .route(
            "/rest/koanPairApprove",
            get(koan_pair_approve).post(koan_pair_approve),
        )
        .route("/rest/koanUsers", get(koan_users).post(koan_users))
        .route(
            "/rest/koanCreateUser",
            get(koan_create_user).post(koan_create_user),
        )
        .route("/rest/koanInvite", get(koan_invite).post(koan_invite))
        .route("/rest/koanApiKeys", get(koan_api_keys).post(koan_api_keys))
        .route("/rest/koanCreateApiKey", post(koan_create_api_key))
        .route("/rest/koanRevokeApiKey", post(koan_revoke_api_key))
        .route("/rest/koanSetUserPassword", post(koan_set_user_password))
        .route("/rest/koanJoin", get(koan_join).post(koan_join))
        .route("/rest/koanSignIn", get(koan_sign_in).post(koan_sign_in))
        .route(
            "/rest/koanRevokeKey",
            get(koan_revoke_key).post(koan_revoke_key),
        )
        .route(
            "/rest/koanSetUserRole",
            get(koan_set_user_role).post(koan_set_user_role),
        )
        .route(
            "/rest/koanDeleteUser",
            get(koan_delete_user).post(koan_delete_user),
        )
        .route("/rest/koanCommand", get(koan_command).post(koan_command))
        .route("/rest/koanHistory", get(koan_history).post(koan_history))
        .route(
            "/rest/koanHistory.view",
            get(koan_history).post(koan_history),
        )
        .route(
            "/rest/koanForgetPlays",
            get(koan_forget_plays).post(koan_forget_plays),
        )
        .route(
            "/rest/koanForgetPlays.view",
            get(koan_forget_plays).post(koan_forget_plays),
        )
        .route(
            "/rest/koanCommand.view",
            get(koan_command).post(koan_command),
        )
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
        // `download` is the untranscoded original, which is all `stream` ever
        // serves here. koan's own download queue fetches through it.
        .route("/rest/download", get(download).post(download))
        .route("/rest/download.view", get(download).post(download))
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
        .route("/rest/getBookmarks", get(get_bookmarks).post(get_bookmarks))
        .route(
            "/rest/getBookmarks.view",
            get(get_bookmarks).post(get_bookmarks),
        )
        .route(
            "/rest/createBookmark",
            get(create_bookmark).post(create_bookmark),
        )
        .route(
            "/rest/createBookmark.view",
            get(create_bookmark).post(create_bookmark),
        )
        .route(
            "/rest/deleteBookmark",
            get(delete_bookmark).post(delete_bookmark),
        )
        .route(
            "/rest/deleteBookmark.view",
            get(delete_bookmark).post(delete_bookmark),
        )
        .route("/rest/setRating", get(set_rating).post(set_rating))
        .route("/rest/setRating.view", get(set_rating).post(set_rating))
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
}

/// The Subsonic routes with their state and the layers every request passes.
/// `form_post` is outermost, so the sign-in throttle sees credentials sent in
/// a form body as well as in the query.
fn subsonic_app(state: Arc<AppState>) -> axum::Router {
    let throttle = Arc::new(AuthThrottle::new(
        state.password.is_some().then(|| state.username.clone()),
        state.users.clone(),
    ));
    register_subsonic_routes(axum::Router::new())
        .with_state(state)
        .layer(axum::middleware::from_fn_with_state(
            throttle,
            throttle_auth,
        ))
        .layer(axum::middleware::from_fn(form_post))
}

/// Build a Subsonic-compatible REST API router.
///
/// Returns `None` unless `[subsonic]` is enabled. Without a shared secret only
/// koan accounts sign in. `/rest/*` carries no JWT layer, so these credentials
/// alone guard every byte of the library — the secret must never be the
/// upstream `[remote]` password.
pub fn subsonic_router(
    pool: Arc<Pool>,
    covers: Arc<crate::covers::Covers>,
    users: Arc<PasswordVerifier>,
) -> Option<axum::Router> {
    let cfg = Config::load().unwrap_or_default();

    if !cfg.subsonic.enabled {
        return None;
    }

    let password = koan_core::helpers::get_subsonic_password(&cfg)
        .filter(|_| !cfg.subsonic.username.is_empty());
    if password.is_none() {
        log::info!("Subsonic: no shared secret, so only koan accounts sign in (with p=).");
    }

    let transcoder = cfg
        .subsonic
        .transcode
        .then(|| {
            let found = crate::transcode::Transcoder::find(&cfg.subsonic.ffmpeg);
            if found.is_none() {
                log::info!(
                    "Subsonic: {} did not run, so clients asking for a lower bitrate get the original.",
                    cfg.subsonic.ffmpeg
                );
            }
            found
        })
        .flatten();

    let state = Arc::new(AppState {
        users,
        pool,
        username: cfg.subsonic.username.clone(),
        password,
        app_key: koan_core::auth::load_keypair()
            .ok()
            .map(|(private, _)| koan_core::auth::app_password_key(&private)),
        upstream: koan_core::helpers::subsonic_auth(&cfg),
        http: reqwest::Client::builder()
            // A whole-request deadline would cut off long proxied streams.
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default(),
        covers,
        last_modified: Default::default(),
        transcoder,
    });

    Some(subsonic_app(state))
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
        // Playlist writes read config to push upstream.
        koan_core::config::isolate_config_for_tests();
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
            users: Arc::new(PasswordVerifier::new(pool.clone())),
            pool,
            username: "testuser".into(),
            password: Some("testpass".into()),
            app_key: Some(koan_core::auth::app_password_key(b"test signing key")),
            upstream: None,
            http: reqwest::Client::new(),
            covers: Arc::new(crate::covers::Covers::new(dir.path().join("covers"))),
            last_modified: Default::default(),
            transcoder: None,
        });
        (state, dir)
    }

    fn build_test_router(state: Arc<AppState>) -> axum::Router {
        register_subsonic_routes(axum::Router::new())
            .with_state(state)
            .layer(axum::middleware::from_fn(form_post))
    }

    #[tokio::test]
    async fn test_failed_password_sign_ins_are_throttled_per_account() {
        let (state, _dir) = test_state();
        let app = subsonic_app(state);
        let wrong = "/rest/ping?u=testuser&p=wrong&v=1.16.1&c=test";

        // Successes never count.
        for _ in 0..20 {
            let (_, body) =
                get_response(app.clone(), &format!("/rest/ping?{}", auth_query(""))).await;
            assert!(body.contains("status=\"ok\""), "{}", body);
        }
        for _ in 0..10 {
            let (_, body) = get_response(app.clone(), wrong).await;
            assert!(body.contains("code=\"40\""), "{}", body);
        }
        // Spent: password sign-ins for this account are refused, even correct
        // ones, for the rest of the minute.
        let (_, body) = get_response(
            app.clone(),
            "/rest/ping?u=testuser&p=testpass&v=1.16.1&c=test",
        )
        .await;
        assert!(body.contains("Too many failed sign-ins"), "{}", body);
        // The shared secret's token is not a password and is not throttled,
        // so the apps signed in with it keep working.
        let (_, body) = get_response(app.clone(), &format!("/rest/ping?{}", auth_query(""))).await;
        assert!(body.contains("status=\"ok\""), "{}", body);
        // Nor is another account.
        let (_, body) = get_response(app, "/rest/ping?u=someone&p=wrong&v=1.16.1&c=test").await;
        assert!(body.contains("code=\"40\""), "{}", body);
    }

    /// A route that refuses every credential, behind the throttle.
    fn refusing_app(shared_username: Option<&str>) -> axum::Router {
        async fn refuse() -> Response {
            SubsonicResponse::error(false, &SubsonicError::wrong_auth())
        }
        axum::Router::new().route("/rest/ping", get(refuse)).layer(
            axum::middleware::from_fn_with_state(
                Arc::new(AuthThrottle::new(
                    shared_username.map(str::to_owned),
                    Arc::new(PasswordVerifier::new(Arc::new(Pool::new(
                        "/nonexistent/koan.db".into(),
                    )))),
                )),
                throttle_auth,
            ),
        )
    }

    #[tokio::test]
    async fn failures_are_also_counted_per_address_whatever_the_username() {
        let app = refusing_app(None);
        for n in 0..AUTH_FAILURES_PER_ADDRESS_PER_MINUTE {
            let (_, body) = get_response(app.clone(), &format!("/rest/ping?u=user{n}&p=x")).await;
            assert!(body.contains("code=\"40\""), "{}", body);
        }
        let (_, body) = get_response(app, "/rest/ping?u=fresh&p=x").await;
        assert!(
            body.contains("Too many failed sign-ins from this address"),
            "{}",
            body
        );
    }

    #[tokio::test]
    async fn a_token_without_a_salt_is_a_password_sign_in() {
        // `validate_auth` checks `p` when `s` is missing, so the shared
        // username's exemption must not cover it.
        let guess = "/rest/ping?u=koan&t=0123&p=guess";
        let app = refusing_app(Some("koan"));
        for _ in 0..AUTH_FAILURES_PER_MINUTE {
            let (_, body) = get_response(app.clone(), guess).await;
            assert!(body.contains("code=\"40\""), "{}", body);
        }
        let (_, body) = get_response(app, guess).await;
        assert!(body.contains("Too many failed sign-ins"), "{}", body);
    }

    #[tokio::test]
    async fn failures_are_also_counted_per_username_from_every_address() {
        let app = refusing_app(None);
        let from = |n: u32, query: &str| {
            let mut request = Request::builder()
                .uri(format!("/rest/ping?{query}"))
                .body(Body::empty())
                .unwrap();
            request.extensions_mut().insert(axum::extract::ConnectInfo(
                std::net::SocketAddr::from(([198, 51, (n / 256) as u8, (n % 256) as u8], 1234)),
            ));
            request
        };
        let body = |response: Response| async {
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            String::from_utf8_lossy(&bytes).into_owned()
        };
        // One guess from each address stays under every per-address limit.
        for n in 0..crate::auth::password::FAILURES_PER_USERNAME_PER_MINUTE {
            let response = app.clone().oneshot(from(n, "u=mate&p=x")).await.unwrap();
            assert!(body(response).await.contains("code=\"40\""));
        }
        let response = app.clone().oneshot(from(999, "u=mate&p=x")).await.unwrap();
        assert!(
            body(response)
                .await
                .contains("Too many failed sign-ins for this account;")
        );
        // Another account, and an API key, are untouched.
        let response = app.clone().oneshot(from(999, "u=other&p=x")).await.unwrap();
        assert!(body(response).await.contains("code=\"40\""));
        let response = app.oneshot(from(999, "apiKey=k")).await.unwrap();
        assert!(body(response).await.contains("code=\"40\""));
    }

    #[tokio::test]
    async fn a_spent_username_budget_spares_the_accounts_own_network() {
        let (state, _dir) = test_state();
        let users = state.users.clone();
        let app = subsonic_app(state);
        let from = |ip: [u8; 4]| {
            let mut request = Request::builder()
                .uri("/rest/ping?u=mate&p=hunter22&v=1.16.1&c=test")
                .body(Body::empty())
                .unwrap();
            request.extensions_mut().insert(axum::extract::ConnectInfo(
                std::net::SocketAddr::from((ip, 1234)),
            ));
            request
        };
        let body = |response: Response| async {
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            String::from_utf8_lossy(&bytes).into_owned()
        };
        let home = [198, 51, 100, 4];
        let r = body(app.clone().oneshot(from(home)).await.unwrap()).await;
        assert!(r.contains("status=\"ok\""), "{r}");
        for _ in 0..crate::auth::password::FAILURES_PER_USERNAME_PER_MINUTE {
            users.failed("mate");
        }
        let r = body(app.clone().oneshot(from([203, 0, 113, 9])).await.unwrap()).await;
        assert!(
            r.contains("Too many failed sign-ins for this account;"),
            "{r}"
        );
        let r = body(app.oneshot(from(home)).await.unwrap()).await;
        assert!(r.contains("status=\"ok\""), "{r}");
    }

    #[tokio::test]
    async fn an_ipv6_client_is_throttled_by_its_64() {
        let app = refusing_app(None);
        for n in 0..AUTH_FAILURES_PER_ADDRESS_PER_MINUTE {
            let mut request = Request::builder()
                .uri(format!("/rest/ping?u=user{n}&p=x"))
                .body(Body::empty())
                .unwrap();
            let ip: std::net::Ipv6Addr = format!("2001:db8:1:2::{n:x}").parse().unwrap();
            request.extensions_mut().insert(axum::extract::ConnectInfo(
                std::net::SocketAddr::from((ip, 1234)),
            ));
            app.clone().oneshot(request).await.unwrap();
        }
        let mut request = Request::builder()
            .uri("/rest/ping?u=fresh&p=x")
            .body(Body::empty())
            .unwrap();
        let ip: std::net::Ipv6Addr = "2001:db8:1:2:abcd::1".parse().unwrap();
        request
            .extensions_mut()
            .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                ip, 1234,
            ))));
        let response = app.oneshot(request).await.unwrap();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("from this address"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_link_closes_when_its_account_changes() {
        let (state, _dir) = test_state();
        let pool = state.pool.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, build_test_router(state)).await });
        let url = format!(
            "ws://{addr}/rest/koanLink?u=mate&p=hunter22&v=1.16.1&c=test&device=account-change-test"
        );
        let mut socket = tokio::task::spawn_blocking(move || {
            let (socket, _) = tungstenite::connect(url.as_str()).unwrap();
            if let tungstenite::stream::MaybeTlsStream::Plain(tcp) = socket.get_ref() {
                tcp.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            }
            socket
        })
        .await
        .unwrap();
        tokio::task::spawn_blocking(move || {
            let db = pool.get().unwrap();
            queries::auth::update_role(&db.conn, "mate", Role::User).unwrap();
        })
        .await
        .unwrap();
        let closed = tokio::task::spawn_blocking(move || {
            loop {
                match socket.read() {
                    Ok(tungstenite::Message::Close(_)) => return true,
                    Ok(_) => continue,
                    Err(_) => return false,
                }
            }
        })
        .await
        .unwrap();
        assert!(closed, "the link outlived a change to its account");
    }

    #[tokio::test]
    async fn the_shared_username_is_exempt_only_with_a_secret() {
        let guess = "/rest/ping?u=koan&t=0123&s=salt";
        let app = refusing_app(None);
        for _ in 0..AUTH_FAILURES_PER_MINUTE {
            get_response(app.clone(), guess).await;
        }
        let (_, body) = get_response(app, guess).await;
        assert!(body.contains("Too many failed sign-ins"), "{}", body);

        let app = refusing_app(Some("koan"));
        for _ in 0..AUTH_FAILURES_PER_ADDRESS_PER_MINUTE + 1 {
            let (_, body) = get_response(app.clone(), guess).await;
            assert!(body.contains("code=\"40\""), "{}", body);
        }
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

    /// What the API publishes as this row's id.
    fn uid_of(state: &AppState, kind: queries::UidKind, id: i64) -> String {
        let db = Database::open(state.pool.path()).unwrap();
        queries::uids_for(&db.conn, kind, [id]).unwrap()[&id].clone()
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
        assert_eq!(codec_to_mime("ALAC"), ("m4a", "audio/mp4"));
        assert_eq!(codec_to_mime("Vorbis"), ("ogg", "audio/ogg"));
        assert_eq!(codec_to_mime("PCM"), ("wav", "audio/wav"));
        assert_eq!(codec_to_mime("WAV"), ("wav", "audio/wav"));
        assert_eq!(codec_to_mime("AIFF"), ("aiff", "audio/aiff"));
        assert_eq!(codec_to_mime("aif"), ("aiff", "audio/aiff"));
        assert_eq!(codec_to_mime("APE"), ("ape", "audio/x-ape"));
        assert_eq!(
            codec_to_mime("Unknown"),
            ("bin", "application/octet-stream")
        );
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
    async fn an_app_password_signs_an_account_in_by_token_or_as_a_password() {
        let (state, _dir) = test_state();
        let ping = |q: String| {
            let app = build_test_router(state.clone());
            async move {
                get_response(app, &format!("/rest/ping?{q}&v=1.16.1&c=test"))
                    .await
                    .1
            }
        };
        let token =
            |secret: &str, salt: &str| format!("{:x}", md5::compute(format!("{secret}{salt}")));

        // No app password yet: 41, which tells the client to fall back.
        let body = ping(format!("u=mate&t={}&s=abc", token("hunter22", "abc"))).await;
        assert!(body.contains("code=\"41\""), "{body}");

        let password = {
            let db = state.open_db().unwrap();
            let mate = queries::auth::get_user_by_username(&db.conn, "mate")
                .unwrap()
                .unwrap();
            queries::app_passwords::create_app_password(
                &db.conn,
                state.app_key.as_ref().unwrap(),
                mate.id,
                "arpeggi",
            )
            .unwrap()
            .1
        };
        let body = ping(format!("u=mate&t={}&s=abc", token(&password, "abc"))).await;
        assert!(body.contains("status=\"ok\""), "{body}");
        let body = ping(format!("u=mate&p={password}")).await;
        assert!(body.contains("status=\"ok\""), "{body}");
        // The account's own password still does not work as a token, and a
        // wrong token is a wrong credential now that app passwords exist.
        let body = ping(format!("u=mate&t={}&s=abc", token("hunter22", "abc"))).await;
        assert!(body.contains("code=\"40\""), "{body}");
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

    #[tokio::test(flavor = "multi_thread")]
    async fn pairing_is_refused_to_web_pages() {
        let (state, _dir) = test_state();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, build_test_router(state)).await });
        let url = format!("ws://{addr}/rest/koanPair?name=tv");
        tokio::task::spawn_blocking(move || {
            use tungstenite::client::IntoClientRequest as _;
            let mut from_page = url.as_str().into_client_request().unwrap();
            from_page
                .headers_mut()
                .insert(header::ORIGIN, "https://evil.example".parse().unwrap());
            match tungstenite::connect(from_page) {
                Err(tungstenite::Error::Http(r)) => assert_eq!(r.status(), StatusCode::FORBIDDEN),
                other => panic!("a page was let in: {:?}", other.map(|_| ())),
            }
            // As the apps connect: tungstenite sends no Origin.
            let (mut socket, _) = tungstenite::connect(url.as_str()).unwrap();
            let first = socket.read().unwrap();
            let message: koan_core::remote::pair::PairMessage =
                serde_json::from_str(first.to_text().unwrap()).unwrap();
            assert!(
                matches!(
                    message,
                    koan_core::remote::pair::PairMessage::Pending { .. }
                ),
                "{message:?}"
            );
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn a_waiting_device_is_paired_with_the_approvers_account() {
        let (state, _dir) = test_state();
        let call = |path: String| {
            let state = state.clone();
            async move { get_response(build_test_router(state), &path).await.1 }
        };
        let mate = "u=mate&p=hunter22&v=1.16.1&c=test&f=json";
        let opened = crate::pair::pairings()
            .open("Den TV", "198.51.100.7".parse().unwrap())
            .unwrap();
        let code = opened.code.replace('-', "").to_lowercase();

        let body = call(format!("/rest/koanPairInfo?pair={code}")).await;
        assert!(body.contains("code=\"10\""), "{body}");
        let body = call(format!("/rest/koanPairInfo?pair={code}&{mate}")).await;
        assert!(
            body.contains("\"device\":\"Den TV\",\"from\":\"198.51.100.7\",\"local\":false"),
            "{body}"
        );

        let body = call(format!("/rest/koanPairApprove?pair={}&{mate}", opened.id)).await;
        assert!(body.contains("\"device\":\"Den TV\""), "{body}");
        let body = call(format!("/rest/koanPairApprove?pair={}&{mate}", opened.id)).await;
        assert!(body.contains("\"code\":70"), "{body}");
        let body = call(format!("/rest/koanPairInfo?pair=ABCD-EFGH&{mate}")).await;
        assert!(body.contains("\"code\":70"), "{body}");
        drop(opened);
    }

    #[tokio::test]
    async fn a_password_sign_in_is_traded_for_a_key_of_the_accounts_own() {
        let (state, _dir) = test_state();
        let call = |path: String| {
            let state = state.clone();
            async move { get_response(build_test_router(state), &path).await.1 }
        };
        let enc = |p: &str| p.bytes().map(|b| format!("{b:02x}")).collect::<String>();

        let body = call("/rest/getOpenSubsonicExtensions?f=json".to_owned()).await;
        assert!(
            body.contains("koanSignIn"),
            "listed for clients to find: {body}"
        );

        let body = call(format!(
            "/rest/koanSignIn?u=mate&p=enc:{}&name=Mate%27s%20iPhone&v=1.16.1&c=test&f=json",
            enc("hunter22")
        ))
        .await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["subsonic-response"]["join"]["username"], "mate", "{body}");
        let key = v["subsonic-response"]["join"]["apiKey"]
            .as_str()
            .unwrap()
            .to_owned();
        let body = call(format!("/rest/ping?apiKey={key}&v=1.16.1&c=test")).await;
        assert!(body.contains("status=\"ok\""), "{body}");
        let db = state.open_db().unwrap();
        let mate = koan_core::db::queries::auth::get_user_by_username(&db.conn, "mate")
            .unwrap()
            .unwrap();
        let keys = queries::api_keys::list_api_keys(&db.conn, Some(mate.id)).unwrap();
        assert_eq!(
            keys.iter().map(|k| k.name.as_str()).collect::<Vec<_>>(),
            ["Mate's iPhone"],
            "the account's own key, named for the device"
        );

        // Signing in again from the same device replaces its key.
        let body = call(format!(
            "/rest/koanSignIn?u=mate&p=enc:{}&name=Mate%27s%20iPhone&v=1.16.1&c=test&f=json",
            enc("hunter22")
        ))
        .await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let again = v["subsonic-response"]["join"]["apiKey"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_ne!(again, key);
        let body = call(format!("/rest/ping?apiKey={key}&v=1.16.1&c=test")).await;
        assert!(body.contains("code=\"44\""), "the old key is gone: {body}");
        let keys = queries::api_keys::list_api_keys(&db.conn, Some(mate.id)).unwrap();
        assert_eq!(keys.len(), 1, "one key per device name");
        let key = again;

        // Only the account's own password: not a key it already holds, not an
        // app password, not the shared secret, which is no account's, and not
        // a wrong password.
        let body = call(format!(
            "/rest/koanSignIn?apiKey={key}&name=more&v=1.16.1&c=test"
        ))
        .await;
        assert!(body.contains("code=\"50\""), "{body}");
        let app_password = queries::app_passwords::create_app_password(
            &db.conn,
            state.app_key.as_ref().unwrap(),
            mate.id,
            "arpeggi",
        )
        .unwrap()
        .1;
        let body = call(format!(
            "/rest/koanSignIn?u=mate&p=enc:{}&name=more&v=1.16.1&c=test",
            enc(&app_password)
        ))
        .await;
        assert!(body.contains("code=\"50\""), "{body}");
        let body =
            call("/rest/koanSignIn?u=testuser&p=testpass&name=shared&v=1.16.1&c=test".to_owned())
                .await;
        assert!(body.contains("code=\"50\""), "{body}");
        let body = call(format!(
            "/rest/koanSignIn?u=mate&p=enc:{}&name=guess&v=1.16.1&c=test",
            enc("wrong")
        ))
        .await;
        assert!(body.contains("code=\"40\""), "{body}");
        assert_eq!(
            queries::api_keys::list_api_keys(&db.conn, None)
                .unwrap()
                .len(),
            1,
            "no key made for any of them"
        );
    }

    #[tokio::test]
    async fn passwords_are_set_by_admins_and_changed_by_their_owners() {
        let (state, _dir) = test_state();
        let set = |form: String| {
            let state = state.clone();
            async move {
                let body =
                    post_form(build_test_router(state), "/rest/koanSetUserPassword", &form).await;
                serde_json::from_str::<serde_json::Value>(&body).unwrap()["subsonic-response"]
                    .clone()
            }
        };
        let ping = |query: String| {
            let state = state.clone();
            async move {
                get_response(
                    build_test_router(state),
                    &format!("/rest/ping?{query}&v=1.16.1&c=test"),
                )
                .await
                .1
            }
        };
        let db = state.open_db().unwrap();
        let mate = queries::auth::get_user_by_username(&db.conn, "mate")
            .unwrap()
            .unwrap();
        let (_, key) = queries::api_keys::replace_api_key(&db.conn, mate.id, "phone").unwrap();
        let as_mate = format!("apiKey={key}&v=1.16.1&c=test&f=json");

        // POST only: the passwords never sit in a URL.
        let (status, _) = get_response(
            build_test_router(state.clone()),
            &format!(
                "/rest/koanSetUserPassword?current=hunter22&password=correct%20horse&{as_mate}"
            ),
        )
        .await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);

        // Only their own, and only with the current password.
        let v = set(format!("username=owner&password=taken%20over&{as_mate}")).await;
        assert_eq!(v["error"]["code"], 50, "{v}");
        let v = set(format!("password=new%20enough&{as_mate}")).await;
        assert_eq!(v["error"]["code"], 10, "{v}");
        let v = set(format!("current=wrong&password=new%20enough&{as_mate}")).await;
        assert_eq!(
            v["error"]["code"], 0,
            "not 40, which reads as the app signed out: {v}"
        );
        assert!(
            ping(format!("apiKey={key}"))
                .await
                .contains("status=\"ok\"")
        );

        // Changed: every old credential ends, and the device gets a new key.
        let v = set(format!(
            "current=hunter22&password=correct%20horse&name=phone&{as_mate}"
        ))
        .await;
        assert_eq!(v["join"]["username"], "mate", "{v}");
        let fresh = v["join"]["apiKey"].as_str().unwrap().to_owned();
        assert!(ping(format!("apiKey={key}")).await.contains("code=\"44\""));
        assert!(
            ping(format!("apiKey={fresh}"))
                .await
                .contains("status=\"ok\"")
        );
        assert!(
            ping("u=mate&p=hunter22".into())
                .await
                .contains("code=\"40\"")
        );
        assert!(
            ping("u=mate&p=correct%20horse".into())
                .await
                .contains("status=\"ok\"")
        );

        // An admin sets anyone's, without theirs, and says nothing back.
        let owner = "u=owner&p=sesame&v=1.16.1&c=test&f=json";
        let v = set(format!("username=mate&password=battery%20staple&{owner}")).await;
        assert_eq!(v["status"], "ok", "{v}");
        assert!(v.get("join").is_none(), "{v}");
        assert!(
            ping(format!("apiKey={fresh}"))
                .await
                .contains("code=\"44\"")
        );
        assert!(
            ping("u=mate&p=battery%20staple".into())
                .await
                .contains("status=\"ok\"")
        );
        let v = set(format!("username=mate&password=short&{owner}")).await;
        assert_eq!(v["error"]["code"], 0, "too short: {v}");
    }

    /// An app password is one client's credential and the shared secret is no
    /// account's: neither may set a password, nor try current passwords.
    #[tokio::test]
    async fn only_a_key_or_the_password_itself_may_set_a_password() {
        let (state, _dir) = test_state();
        let set = |auth: String| {
            let state = state.clone();
            async move {
                post_form(
                    build_test_router(state),
                    "/rest/koanSetUserPassword",
                    &format!("current=hunter22&password=correct%20horse&{auth}&v=1.16.1&c=test"),
                )
                .await
            }
        };
        let db = state.open_db().unwrap();
        let mate = queries::auth::get_user_by_username(&db.conn, "mate")
            .unwrap()
            .unwrap();
        let app_password = queries::app_passwords::create_app_password(
            &db.conn,
            state.app_key.as_ref().unwrap(),
            mate.id,
            "arpeggi",
        )
        .unwrap()
        .1;
        let salt = "c0ffee";
        let token = format!("{:x}", md5::compute(format!("{app_password}{salt}")));

        let body = set(format!("u=mate&p={app_password}")).await;
        assert!(body.contains("code=\"50\""), "app password as p: {body}");
        let body = set(format!("u=mate&t={token}&s={salt}")).await;
        assert!(
            body.contains("code=\"50\""),
            "app password as a token: {body}"
        );
        let body = set(auth_query("")).await;
        assert!(body.contains("code=\"50\""), "the shared secret: {body}");
        assert!(
            queries::auth::get_user_by_username(&db.conn, "mate")
                .unwrap()
                .is_some_and(
                    |u| koan_core::auth::verify_password("hunter22", &u.password_hash).is_ok()
                ),
            "unchanged"
        );
    }

    /// Checked in the handler, from every network: the request is signed with
    /// a key, which the sign-in throttle leaves alone, and that throttle spares
    /// networks the account signed in from, which must not make guessing here
    /// free.
    #[tokio::test]
    async fn a_wrong_current_password_spends_the_sign_in_budget() {
        let (state, _dir) = test_state();
        let db = state.open_db().unwrap();
        let mate = queries::auth::get_user_by_username(&db.conn, "mate")
            .unwrap()
            .unwrap();
        let (_, key) = queries::api_keys::replace_api_key(&db.conn, mate.id, "phone").unwrap();
        let change = |current: &str| {
            let state = state.clone();
            let form =
                format!("current={current}&password=another%20one&apiKey={key}&v=1.16.1&c=test");
            async move { post_form(build_test_router(state), "/rest/koanSetUserPassword", &form).await }
        };
        // Signing in by password makes this network one the account is known on.
        let app = subsonic_app(state.clone());
        let body = get_response(app, "/rest/ping?u=mate&p=hunter22&v=1.16.1&c=test")
            .await
            .1;
        assert!(body.contains("status=\"ok\""), "{body}");

        let body = change("wrong").await;
        assert!(body.contains("the current password is wrong"), "{body}");
        for _ in 1..crate::auth::password::FAILURES_PER_USERNAME_PER_MINUTE {
            state.users.failed("mate");
        }
        let body = change("hunter22").await;
        assert!(body.contains("Too many failed sign-ins"), "{body}");
        let body = get_response(
            build_test_router(state.clone()),
            &format!("/rest/ping?apiKey={key}&v=1.16.1&c=test"),
        )
        .await
        .1;
        assert!(
            body.contains("status=\"ok\""),
            "the key still works: {body}"
        );
    }

    #[tokio::test]
    async fn an_account_lists_makes_and_revokes_its_own_api_keys() {
        let (state, _dir) = test_state();
        let phone = api_key(&state, "mate");
        let owners = api_key(&state, "owner");
        let auth = format!("apiKey={phone}&v=1.16.1&c=test&f=json");
        let json = |body: String| -> serde_json::Value {
            serde_json::from_str::<serde_json::Value>(&body).unwrap()["subsonic-response"].clone()
        };
        let post = |path: &str, form: String| {
            let state = state.clone();
            let path = path.to_owned();
            async move { json(post_form(build_test_router(state), &path, &form).await) }
        };
        let list = || {
            let state = state.clone();
            let auth = auth.clone();
            async move {
                json(
                    get_response(
                        build_test_router(state),
                        &format!("/rest/koanApiKeys?{auth}"),
                    )
                    .await
                    .1,
                )
            }
        };

        let made = post("/rest/koanCreateApiKey", format!("name=Feishin&{auth}")).await;
        let key = made["apiKey"]["key"].as_str().unwrap().to_owned();
        let id = made["apiKey"]["id"].as_i64().unwrap();
        assert_eq!(made["apiKey"]["name"], "Feishin", "{made}");
        let pinged = get_response(
            build_test_router(state.clone()),
            &format!("/rest/ping?apiKey={key}&v=1.16.1&c=test"),
        )
        .await
        .1;
        assert!(pinged.contains("status=\"ok\""), "{pinged}");

        // Only this account's, the request's own marked, and never a key.
        let v = list().await;
        let keys = v["apiKeys"]["apiKey"].as_array().unwrap();
        assert_eq!(keys.len(), 2, "{v}");
        assert_eq!(keys[0]["name"], "test");
        assert_eq!(keys[0]["current"], true);
        assert_eq!(keys[1]["current"], false);
        assert!(keys[0]["created"].as_str().unwrap().ends_with('Z'), "{v}");
        assert!(
            !v.to_string().contains(&phone) && !v.to_string().contains(&key),
            "{v}"
        );

        // Making and revoking take POST.
        let (status, _) = get_response(
            build_test_router(state.clone()),
            &format!("/rest/koanCreateApiKey?name=x&{auth}"),
        )
        .await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);

        // Not this device's own key, and not another account's.
        let db = state.open_db().unwrap();
        let own = queries::api_keys::id_of(&db.conn, &phone).unwrap().unwrap();
        let v = post("/rest/koanRevokeApiKey", format!("id={own}&{auth}")).await;
        assert!(
            v["error"]["message"].as_str().unwrap().contains("sign out"),
            "{v}"
        );
        let theirs = queries::api_keys::id_of(&db.conn, &owners)
            .unwrap()
            .unwrap();
        let v = post("/rest/koanRevokeApiKey", format!("id={theirs}&{auth}")).await;
        assert_eq!(v["error"]["code"], 70, "{v}");

        let v = post("/rest/koanRevokeApiKey", format!("id={id}&{auth}")).await;
        assert_eq!(v["status"], "ok", "{v}");
        let pinged = get_response(
            build_test_router(state.clone()),
            &format!("/rest/ping?apiKey={key}&v=1.16.1&c=test"),
        )
        .await
        .1;
        assert!(pinged.contains("code=\"44\""), "{pinged}");
        assert_eq!(
            list().await["apiKeys"]["apiKey"].as_array().unwrap().len(),
            1
        );
    }

    /// An app password must not mint keys that outlive it, nor revoke the
    /// account's devices; the shared secret is no account's.
    #[tokio::test]
    async fn only_a_key_or_the_password_itself_may_manage_api_keys() {
        let (state, _dir) = test_state();
        let db = state.open_db().unwrap();
        let mate = queries::auth::get_user_by_username(&db.conn, "mate")
            .unwrap()
            .unwrap();
        let (phone, _) = queries::api_keys::replace_api_key(&db.conn, mate.id, "phone").unwrap();
        let app_password = queries::app_passwords::create_app_password(
            &db.conn,
            state.app_key.as_ref().unwrap(),
            mate.id,
            "arpeggi",
        )
        .unwrap()
        .1;
        let salt = "c0ffee";
        let token = format!("{:x}", md5::compute(format!("{app_password}{salt}")));
        let callers = [
            ("app password as p", format!("u=mate&p={app_password}")),
            (
                "app password as a token",
                format!("u=mate&t={token}&s={salt}"),
            ),
            ("the shared secret", auth_query("")),
        ];
        for (who, auth) in &callers {
            let auth = format!("{auth}&v=1.16.1&c=test");
            for (path, form) in [
                ("/rest/koanCreateApiKey", format!("name=x&{auth}")),
                ("/rest/koanRevokeApiKey", format!("id={phone}&{auth}")),
                ("/rest/koanApiKeys", auth.clone()),
            ] {
                let body = post_form(build_test_router(state.clone()), path, &form).await;
                assert!(body.contains("code=\"50\""), "{who} on {path}: {body}");
            }
        }
        let keys = queries::api_keys::list_api_keys(&db.conn, None).unwrap();
        assert_eq!(keys.len(), 1, "none made, none revoked");
        assert_eq!(keys[0].id, phone);
    }

    #[tokio::test]
    async fn admins_manage_accounts_through_the_koan_endpoints() {
        let (state, _dir) = test_state();
        let call = |path: String| {
            let state = state.clone();
            async move { get_response(build_test_router(state), &path).await.1 }
        };
        let owner = "u=owner&p=sesame&v=1.16.1&c=test&f=json";

        let body = call("/rest/koanUsers?u=mate&p=hunter22&v=1.16.1&c=test".to_owned()).await;
        assert!(body.contains("code=\"50\""), "{body}");

        let body = call(format!(
            "/rest/koanCreateUser?username=sarita&role=readonly&{owner}"
        ))
        .await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let invite = &v["subsonic-response"]["invite"];
        let password = invite["password"].as_str().unwrap().to_owned();
        let token = invite["token"].as_str().unwrap().to_owned();
        assert_eq!(invite["username"], "sarita");

        // The password works in any Subsonic app; the token, redeemed with
        // nothing else, makes this device a key.
        let body = call(format!("/rest/ping?u=sarita&p={password}&v=1.16.1&c=test")).await;
        assert!(body.contains("status=\"ok\""), "{body}");
        let body = call(format!(
            "/rest/koanJoin?invite={token}&name=Sarita%27s%20iPhone&f=json"
        ))
        .await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            v["subsonic-response"]["join"]["username"], "sarita",
            "{body}"
        );
        let key = v["subsonic-response"]["join"]["apiKey"].as_str().unwrap();
        let body = call(format!("/rest/ping?apiKey={key}&v=1.16.1&c=test")).await;
        assert!(body.contains("status=\"ok\""), "{body}");
        let body = call("/rest/koanJoin?invite=forged&f=json".to_owned()).await;
        assert!(body.contains("not valid"), "{body}");

        // A key gives itself up.
        let body = call(format!("/rest/koanJoin?invite={token}&name=spare&f=json")).await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let spare = v["subsonic-response"]["join"]["apiKey"].as_str().unwrap();
        let body = call(format!("/rest/koanRevokeKey?apiKey={spare}&f=json")).await;
        assert!(body.contains("\"ok\""), "{body}");
        let body = call(format!("/rest/ping?apiKey={spare}&v=1.16.1&c=test")).await;
        assert!(body.contains("code=\"44\""), "{body}");
        let body = call(format!("/rest/koanRevokeKey?{owner}")).await;
        assert!(body.contains("sign in with the key"), "{body}");

        // Inviting again never reads the password back.
        let body = call(format!("/rest/koanInvite?username=sarita&{owner}")).await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(
            v["subsonic-response"]["invite"]["token"].is_string(),
            "{body}"
        );
        assert!(
            v["subsonic-response"]["invite"]["password"].is_null(),
            "{body}"
        );

        // A reset gives a new one, signs the invited device out, and withdraws
        // the link already sent.
        let body = call(format!(
            "/rest/koanInvite?username=sarita&reset=true&{owner}"
        ))
        .await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let reset = v["subsonic-response"]["invite"]["password"]
            .as_str()
            .unwrap();
        assert_ne!(reset, password);
        let body = call(format!("/rest/ping?apiKey={key}&v=1.16.1&c=test")).await;
        assert!(body.contains("code=\"44\""), "{body}");
        let body = call(format!("/rest/koanJoin?invite={token}&name=late&f=json")).await;
        assert!(body.contains("not valid"), "{body}");

        let body = call(format!(
            "/rest/koanSetUserRole?username=owner&role=user&{owner}"
        ))
        .await;
        assert!(body.contains("last admin"), "{body}");
        call(format!(
            "/rest/koanSetUserRole?username=sarita&role=user&{owner}"
        ))
        .await;
        let body = call(format!("/rest/koanUsers?{owner}")).await;
        assert!(
            body.contains("{\"role\":\"user\",\"username\":\"sarita\"}"),
            "{body}"
        );

        let body = call(format!("/rest/koanDeleteUser?username=owner&{owner}")).await;
        assert!(body.contains("cannot delete itself"), "{body}");
        call(format!("/rest/koanDeleteUser?username=sarita&{owner}")).await;
        let body = call(format!("/rest/koanUsers?{owner}")).await;
        assert!(!body.contains("sarita"), "{body}");
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

    /// A route that needs no database stays fast while slow Subsonic requests
    /// are in flight — argon2 on a wrong password, which is never remembered.
    /// With that work on the runtime's two workers, the first probe waited
    /// over a second; off them it takes milliseconds. The bound leaves room
    /// for a loaded CI machine and still fails the regression.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn trivial_route_under_load() {
        const SLOW: usize = 8;
        const PROBES: usize = 20;
        let (state, _dir) = test_state();
        let app = build_test_router(state);
        let slow_uri = "/rest/getAlbum?u=mate&p=wrong&v=1.16.1&c=test&id=1";
        let probe_uri = "/rest/getOpenSubsonicExtensions";

        let probe = app.clone();
        let idle = tokio::spawn(async move {
            let t = std::time::Instant::now();
            get_response(probe, probe_uri).await;
            t.elapsed()
        })
        .await
        .unwrap();

        let started = std::time::Instant::now();
        let slow: Vec<_> = (0..SLOW)
            .map(|_| {
                let app = app.clone();
                tokio::spawn(async move { get_response(app, slow_uri).await })
            })
            .collect();
        // The test body does not run on a worker, so each probe is spawned and
        // timed from the spawn, as an arriving request would be. A thread sleep
        // lets the slow requests take the workers; a timer would need one to fire.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let mut probes = Vec::with_capacity(PROBES);
        for _ in 0..PROBES {
            let t = std::time::Instant::now();
            let (status, _) = tokio::spawn(get_response(app.clone(), probe_uri))
                .await
                .unwrap();
            assert_eq!(status, StatusCode::OK);
            probes.push(t.elapsed());
        }
        for s in slow {
            s.await.unwrap();
        }
        let slow_total = started.elapsed();
        probes.sort();
        println!(
            "idle probe {idle:?}; under {SLOW} slow requests: median {:?}, max {:?}; \
             slow requests done in {slow_total:?}",
            probes[PROBES / 2],
            probes[PROBES - 1],
        );
        assert!(
            probes[PROBES - 1] < std::time::Duration::from_millis(750),
            "a trivial route waited {:?} behind slow Subsonic requests",
            probes[PROBES - 1]
        );
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

    /// Typed attributes must not change the XML wire format: every attribute is
    /// still a quoted string.
    #[tokio::test]
    async fn test_song_xml_is_unchanged_by_typed_attributes() {
        let (state, _dir) = test_state();
        seed_data(&state);

        let db = Database::open(state.pool.path()).unwrap();
        let track = queries::all_tracks(&db.conn).unwrap()[0].clone();
        let id = uid_of(&state, queries::UidKind::Track, track.id);
        let album = uid_of(&state, queries::UidKind::Album, track.album_id.unwrap());
        let artist = uid_of(&state, queries::UidKind::Artist, track.artist_id.unwrap());

        let app = build_test_router(state);
        let (_, body) = get_response(
            app,
            &format!("/rest/getSong?{}&id={}", auth_query(""), track.id),
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
                    r#"contentType="audio/flac" codec="FLAC" genre="Rock" albumId="{album}" artistId="{artist}" "#,
                    r#"parent="{album}" coverArt="{id}" type="music" isDir="false" "#,
                    r#"mediaType="song" bitDepth="16" samplingRate="44100" channelCount="2" "#,
                    r#"displayArtist="Test Artist" displayAlbumArtist="Test Artist" "#,
                    r#"musicBrainzId="">"#
                ),
                id = id,
                album = album,
                artist = artist,
            )
        );
    }

    /// Strictly-typed clients (Symfonium, Substreamer, Feishin) reject
    /// `"duration": "240"`, so numeric and boolean fields go out unquoted in JSON.
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

        let app = build_test_router(state.clone());
        let (_, body) = get_response(
            app,
            &format!("/rest/getSong?{}&id={}", auth_query("f=json"), track_id),
        )
        .await;

        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        let song: StrictSong =
            serde_json::from_value(parsed["subsonic-response"]["song"].clone()).unwrap();

        let uid = uid_of(&state, queries::UidKind::Track, track_id);
        assert_eq!(song.id, uid);
        assert_eq!(song.title, "Test Song");
        assert_eq!(song.duration, 240);
        assert_eq!(song.track, 1);
        assert_eq!(song.bit_rate, 1411);
        assert_eq!(song.disc_number, 1);
        assert!(!song.is_dir);
        assert_eq!(song.kind, "music");
        assert_eq!(song.cover_art, uid);
    }

    #[tokio::test]
    async fn test_album_json_field_types() {
        let (state, _dir) = test_state();
        seed_data(&state);

        let db = Database::open(state.pool.path()).unwrap();
        let album_id = queries::all_albums(&db.conn).unwrap()[0].id;

        let app = build_test_router(state.clone());
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
        assert_eq!(
            album["coverArt"],
            uid_of(&state, queries::UidKind::Album, album_id)
        );
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

    /// Three records by one artist, each with its own genre and year.
    fn seed_shelves(state: &AppState) -> Vec<i64> {
        let db = Database::open(state.pool.path()).unwrap();
        [
            ("Alpha", "Jazz", "1971"),
            ("Beta", "Rock", "1995"),
            ("Gamma", "jazz", "2001"),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (album, genre, date))| {
            let mut meta = track_meta(&format!("/music/s{i}.flac"), album, album, 1);
            meta.genre = Some(genre.into());
            meta.date = Some(date.into());
            queries::upsert_track(&db.conn, &meta).unwrap()
        })
        .collect()
    }

    fn names(list: &serde_json::Value) -> Vec<String> {
        list.as_array()
            .map(|a| {
                a.iter()
                    .map(|x| x["title"].as_str().unwrap().to_owned())
                    .collect()
            })
            .unwrap_or_default()
    }

    async fn album_list2(state: &Arc<AppState>, params: &str) -> Vec<String> {
        let v = json_of(
            build_test_router(state.clone()),
            &format!("/rest/getAlbumList2?{}&{params}", auth_query("f=json")),
        )
        .await;
        names(&v["albumList2"]["album"])
    }

    #[tokio::test]
    async fn album_lists_answer_every_type_from_sql() {
        let (state, _dir) = test_state();
        let [alpha, beta, gamma] = seed_shelves(&state)[..] else {
            unreachable!()
        };

        assert_eq!(
            album_list2(&state, "type=byGenre&genre=JAZZ").await,
            ["Alpha", "Gamma"]
        );
        assert_eq!(
            album_list2(&state, "type=byYear&fromYear=1990&toYear=2010").await,
            ["Beta", "Gamma"]
        );
        assert_eq!(
            album_list2(&state, "type=byYear&fromYear=2010&toYear=1990").await,
            ["Gamma", "Beta"],
            "a reversed range lists newest first"
        );
        assert_eq!(
            album_list2(&state, "type=alphabeticalByName&size=2&offset=1").await,
            ["Beta", "Gamma"]
        );
        assert_eq!(album_list2(&state, "type=random").await.len(), 3);

        assert!(album_list2(&state, "type=starred").await.is_empty());
        let beta_album = uid_of(
            &state,
            queries::UidKind::Album,
            queries::get_track_row(&Database::open(state.pool.path()).unwrap().conn, beta)
                .unwrap()
                .unwrap()
                .album_id
                .unwrap(),
        );
        get_response(
            build_test_router(state.clone()),
            &format!("/rest/star?{}&albumId={beta_album}", auth_query("")),
        )
        .await;
        assert_eq!(album_list2(&state, "type=starred").await, ["Beta"]);

        // Two plays of Alpha, the later of Gamma's one in between, sent as one
        // batched scrobble.
        let (_, body) = get_response(
            build_test_router(state.clone()),
            &format!(
                "/rest/scrobble?{}&id={alpha}&time=1000000&id={gamma}&time=3000000&id={alpha}&time=2000000",
                auth_query("")
            ),
        )
        .await;
        assert!(body.contains("status=\"ok\""), "{body}");
        assert_eq!(album_list2(&state, "type=recent").await, ["Gamma", "Alpha"]);
        assert_eq!(
            album_list2(&state, "type=frequent").await,
            ["Alpha", "Gamma"]
        );
        assert!(album_list2(&state, "type=highest").await.is_empty());

        let v = json_of(
            build_test_router(state.clone()),
            &format!("/rest/getAlbumList2?{}&type=byGenre", auth_query("f=json")),
        )
        .await;
        assert_eq!(v["error"]["code"], 10, "byGenre needs a genre");
        let v = json_of(
            build_test_router(state),
            &format!("/rest/getAlbumList2?{}&type=nonsense", auth_query("f=json")),
        )
        .await;
        assert_eq!(v["error"]["code"], 10, "an unknown type is refused");
    }

    #[tokio::test]
    async fn ratings_set_clear_and_order_highest() {
        let (state, _dir) = test_state();
        let [alpha, _, gamma] = seed_shelves(&state)[..] else {
            unreachable!()
        };
        let album_of = |track| {
            let db = Database::open(state.pool.path()).unwrap();
            let album = queries::get_track_row(&db.conn, track)
                .unwrap()
                .unwrap()
                .album_id
                .unwrap();
            uid_of(&state, queries::UidKind::Album, album)
        };
        let rate = |id: String, rating: &'static str| {
            let state = state.clone();
            async move {
                json_of(
                    build_test_router(state),
                    &format!(
                        "/rest/setRating?{}&id={id}&rating={rating}",
                        auth_query("f=json")
                    ),
                )
                .await
            }
        };

        let (alpha_album, gamma_album) = (album_of(alpha), album_of(gamma));
        rate(alpha_album.clone(), "3").await;
        rate(gamma_album.clone(), "5").await;
        assert_eq!(
            album_list2(&state, "type=highest").await,
            ["Gamma", "Alpha"]
        );
        let v = json_of(
            build_test_router(state.clone()),
            &format!("/rest/getAlbum?{}&id={alpha_album}", auth_query("f=json")),
        )
        .await;
        assert_eq!(v["album"]["userRating"], 3, "{v}");

        let song = uid_of(&state, queries::UidKind::Track, alpha);
        rate(song.clone(), "4").await;
        let song_rating = |state: Arc<AppState>, song: String| async move {
            json_of(
                build_test_router(state),
                &format!("/rest/getSong?{}&id={song}", auth_query("f=json")),
            )
            .await["song"]["userRating"]
                .clone()
        };
        assert_eq!(song_rating(state.clone(), song.clone()).await, 4);
        rate(song.clone(), "0").await;
        assert!(song_rating(state.clone(), song.clone()).await.is_null());

        let v = rate(song, "6").await;
        assert_eq!(v["error"]["code"], 10, "a rating above 5 is refused");

        rate(gamma_album, "0").await;
        assert_eq!(album_list2(&state, "type=highest").await, ["Alpha"]);
    }

    #[tokio::test]
    async fn bookmarks_save_replace_list_and_delete() {
        let (state, _dir) = test_state();
        let [alpha, beta, _] = seed_shelves(&state)[..] else {
            unreachable!()
        };
        let call = |path: String| {
            let state = state.clone();
            async move {
                json_of(
                    build_test_router(state),
                    &format!("/rest/{path}&{}", auth_query("f=json")),
                )
                .await
            }
        };
        let (alpha, beta) = (
            uid_of(&state, queries::UidKind::Track, alpha),
            uid_of(&state, queries::UidKind::Track, beta),
        );

        call(format!(
            "createBookmark?id={alpha}&position=1000&comment=intro"
        ))
        .await;
        call(format!("createBookmark?id={beta}&position=5000")).await;
        call(format!("createBookmark?id={alpha}&position=2500")).await;
        let v = call("getBookmarks?".into()).await;
        let marks = v["bookmarks"]["bookmark"].as_array().unwrap();
        assert_eq!(marks.len(), 2, "{v}");
        let mark = marks.iter().find(|m| m["entry"]["id"] == alpha).unwrap();
        assert_eq!(
            mark["position"], 2500,
            "a second bookmark replaces the first"
        );
        assert!(mark["comment"].is_null());
        assert_eq!(mark["username"], "testuser");
        assert!(mark["created"].is_string() && mark["changed"].is_string());

        let v = call(format!("createBookmark?id={alpha}")).await;
        assert_eq!(v["error"]["code"], 10, "position is required");

        call(format!("deleteBookmark?id={alpha}")).await;
        let v = call("getBookmarks?".into()).await;
        assert_eq!(v["bookmarks"]["bookmark"].as_array().unwrap().len(), 1);
        let v = call(format!("deleteBookmark?id={alpha}")).await;
        assert_eq!(v["status"], "ok", "deleting twice is not an error");

        let long = "x".repeat(MAX_BOOKMARK_COMMENT + 1);
        let v = call(format!(
            "createBookmark?id={beta}&position=1&comment={long}"
        ))
        .await;
        assert_eq!(v["error"]["code"], 10, "an overlong comment is refused");
        let v = call("getBookmarks?".into()).await;
        assert_eq!(v["bookmarks"]["bookmark"][0]["position"], 5000, "{v}");
    }

    #[tokio::test]
    async fn random_songs_filter_in_sql() {
        let (state, _dir) = test_state();
        seed_shelves(&state);
        let random = |params: &'static str| {
            let state = state.clone();
            async move {
                let v = json_of(
                    build_test_router(state),
                    &format!("/rest/getRandomSongs?{}&{params}", auth_query("f=json")),
                )
                .await;
                let mut got = names(&v["randomSongs"]["song"]);
                got.sort();
                got
            }
        };
        // A genre that is a small share of the library still fills the draw.
        assert_eq!(random("genre=jazz&size=10").await, ["Alpha", "Gamma"]);
        assert_eq!(random("fromYear=1990&toYear=1999").await, ["Beta"]);
        assert_eq!(random("size=4000000000").await.len(), 3);
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

    /// A malformed id comes back as a Subsonic error, not as axum's bare HTTP 400
    /// with a plain-text body that no client can parse.
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
            &format!(
                "/rest/getInternetRadioStations.view?{}",
                auth_query("f=json")
            ),
        )
        .await;
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["subsonic-response"]["error"]["code"], 70);
    }

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

    /// A client keeping a copy of the library asks whether it moved; a
    /// deletion has to move it, though it touches no file.
    #[tokio::test]
    async fn get_indexes_says_when_the_library_moved() {
        let (state, _dir) = test_state();
        seed_data(&state);
        let modified = |body: &str| -> i64 {
            let at = body.find("lastModified=\"").unwrap() + "lastModified=\"".len();
            body[at..].split('"').next().unwrap().parse().unwrap()
        };

        let app = build_test_router(state.clone());
        let (_, body) = get_response(app, &format!("/rest/getIndexes?{}", auth_query(""))).await;
        let first = modified(&body);

        let app = build_test_router(state.clone());
        let (_, body) = get_response(
            app,
            &format!(
                "/rest/getIndexes?{}&ifModifiedSince={first}",
                auth_query("")
            ),
        )
        .await;
        assert_eq!(modified(&body), first);
        assert!(!body.contains("<artist"), "unchanged: the timestamp alone");

        std::thread::sleep(std::time::Duration::from_millis(5));
        let db = Database::open(state.pool.path()).unwrap();
        db.conn
            .execute(
                "DELETE FROM tracks WHERE id = (SELECT MAX(id) FROM tracks)",
                [],
            )
            .unwrap();

        let app = build_test_router(state);
        let (_, body) = get_response(
            app,
            &format!(
                "/rest/getIndexes?{}&ifModifiedSince={first}",
                auth_query("")
            ),
        )
        .await;
        assert!(modified(&body) > first, "a deletion moves it");
    }

    #[tokio::test]
    async fn test_get_indexes_and_music_directory() {
        let (state, _dir) = test_state();
        seed_data(&state);

        let db = Database::open(state.pool.path()).unwrap();
        let artist_id = queries::all_artists(&db.conn).unwrap()[0].id;
        let album_id = queries::all_albums(&db.conn).unwrap()[0].id;

        let artist = uid_of(&state, queries::UidKind::Artist, artist_id);
        let album = uid_of(&state, queries::UidKind::Album, album_id);

        let app = build_test_router(state.clone());
        let (_, body) = get_response(app, &format!("/rest/getIndexes?{}", auth_query(""))).await;
        assert!(body.contains("<indexes"));
        assert!(body.contains(&format!("id=\"{artist}\"")));

        // Artist directory lists albums.
        let app = build_test_router(state.clone());
        let (_, body) = get_response(
            app,
            &format!("/rest/getMusicDirectory?{}&id={artist}", auth_query("")),
        )
        .await;
        assert!(body.contains(&format!("id=\"{album}\"")));
        assert!(body.contains("isDir=\"true\""));

        // Album directory lists songs, by the prefixed row id clients from
        // before uids hold.
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

    /// Seven tracks over three albums, for the paging tests.
    fn seed_library(state: &AppState) -> Vec<i64> {
        let db = Database::open(state.pool.path()).unwrap();
        (0..7)
            .map(|i| {
                queries::upsert_track(
                    &db.conn,
                    &track_meta(
                        &format!("/music/{i}.flac"),
                        &format!("Song {i}"),
                        &format!("Album {}", i / 3),
                        i % 3 + 1,
                    ),
                )
                .unwrap()
            })
            .collect()
    }

    async fn search3_json(state: &Arc<AppState>, params: &str) -> serde_json::Value {
        let app = build_test_router(state.clone());
        let (_, body) = get_response(
            app,
            &format!("/rest/search3?{}&{params}", auth_query("f=json")),
        )
        .await;
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        parsed["subsonic-response"]["searchResult3"].clone()
    }

    /// The rows a listing names, by the uids it publishes.
    fn ids(state: &AppState, list: &serde_json::Value) -> Vec<i64> {
        let db = Database::open(state.pool.path()).unwrap();
        list.as_array()
            .map(|a| {
                a.iter()
                    .map(|v| {
                        queries::find_uid(&db.conn, v["id"].as_str().unwrap())
                            .unwrap()
                            .unwrap()
                            .1
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// An empty query lists the whole library, as OpenSubsonic clients use it
    /// to — both as `query=` and as a pair of quotes.
    #[tokio::test]
    async fn test_search3_empty_query_lists_everything_in_id_order() {
        let (state, _dir) = test_state();
        let tracks = seed_library(&state);

        for query in ["query=", "query=%22%22"] {
            let r = search3_json(
                &state,
                &format!("{query}&songCount=100&albumCount=100&artistCount=100"),
            )
            .await;
            assert_eq!(ids(&state, &r["song"]), tracks, "{query}");
            assert_eq!(ids(&state, &r["album"]).len(), 3, "{query}");
            assert_eq!(ids(&state, &r["artist"]).len(), 1, "{query}");
        }
    }

    /// Pages walked by offset cover every track exactly once.
    #[tokio::test]
    async fn test_search3_song_offset_pages_without_gaps_or_repeats() {
        let (state, _dir) = test_state();
        let tracks = seed_library(&state);

        let mut walked = Vec::new();
        for offset in (0..).step_by(3) {
            let r = search3_json(
                &state,
                &format!("query=&songCount=3&songOffset={offset}&albumCount=0&artistCount=0"),
            )
            .await;
            assert!(ids(&state, &r["album"]).is_empty() && ids(&state, &r["artist"]).is_empty());
            let page = ids(&state, &r["song"]);
            if page.is_empty() {
                break;
            }
            walked.extend(page);
        }
        assert_eq!(walked, tracks);
    }

    #[tokio::test]
    async fn test_search3_album_and_artist_offsets_are_honoured() {
        let (state, _dir) = test_state();
        seed_library(&state);

        let all = ids(
            &state,
            &search3_json(&state, "query=&albumCount=10&songCount=0").await["album"],
        );
        let second = ids(
            &state,
            &search3_json(&state, "query=&albumCount=1&albumOffset=1&songCount=0").await["album"],
        );
        assert_eq!(second, vec![all[1]]);

        let past = search3_json(&state, "query=&artistCount=5&artistOffset=1&songCount=0").await;
        assert!(ids(&state, &past["artist"]).is_empty());

        // A text search pages its songs too.
        let first = ids(
            &state,
            &search3_json(&state, "query=Song&songCount=2").await["song"],
        );
        let next = ids(
            &state,
            &search3_json(&state, "query=Song&songCount=2&songOffset=2").await["song"],
        );
        assert_eq!(first.len(), 2);
        assert_eq!(next.len(), 2);
        assert!(first.iter().all(|id| !next.contains(id)));
    }

    #[tokio::test]
    async fn test_search3_song_count_is_capped() {
        let (state, _dir) = test_state();
        {
            let db = Database::open(state.pool.path()).unwrap();
            db.conn.execute_batch("BEGIN").unwrap();
            for i in 0..(SEARCH_PAGE_MAX + 5) {
                queries::upsert_track(
                    &db.conn,
                    &track_meta(&format!("/m/{i}.flac"), &format!("T{i}"), "A", 1),
                )
                .unwrap();
            }
            db.conn.execute_batch("COMMIT").unwrap();
        }
        let r = search3_json(&state, "query=&songCount=5000&albumCount=0&artistCount=0").await;
        assert_eq!(ids(&state, &r["song"]).len(), SEARCH_PAGE_MAX as usize);
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
        let row = queries::id_for_uid(&db.conn, queries::UidKind::Playlist, &id)
            .unwrap()
            .unwrap();
        let left = queries::playlist_track_ids(&db.conn, row).unwrap();
        assert_eq!(left, vec![c, a]);
    }

    /// A smart playlist is served like any other, marked read-only, and its
    /// contents refuse edits while its name does not.
    #[tokio::test]
    async fn smart_playlists_are_served_read_only() {
        let (state, _dir) = test_state();
        seed_data(&state);

        let db = Database::open(state.pool.path()).unwrap();
        let tracks = queries::all_tracks(&db.conn).unwrap();
        let rules = koan_core::smart::Rules::parse(r#"{"rules":[]}"#).unwrap();
        let row = queries::smart::create_smart_playlist(
            &db.conn,
            queries::LOCAL_USER,
            "Everything",
            None,
            &rules,
        )
        .unwrap();
        let id = queries::get_playlist(&db.conn, row).unwrap().unwrap().uid;
        drop(db);

        let app = build_test_router(state.clone());
        let (_, body) = get_response(app, &format!("/rest/getPlaylists?{}", auth_query(""))).await;
        assert!(body.contains("name=\"Everything\""), "{body}");
        assert!(body.contains("readonly=\"true\""), "{body}");

        let app = build_test_router(state.clone());
        let (_, body) = get_response(
            app,
            &format!("/rest/getPlaylist?{}&id={id}", auth_query("")),
        )
        .await;
        assert_eq!(body.matches("<entry ").count(), tracks.len(), "{body}");

        for edit in [
            format!(
                "updatePlaylist?playlistId={id}&songIdToAdd={}",
                tracks[0].id
            ),
            format!("updatePlaylist?playlistId={id}&songIndexToRemove=0"),
            format!("createPlaylist?playlistId={id}&songId={}", tracks[0].id),
        ] {
            let (path, query) = edit.split_once('?').unwrap();
            let app = build_test_router(state.clone());
            let (_, body) =
                get_response(app, &format!("/rest/{path}?{}&{query}", auth_query(""))).await;
            assert!(body.contains("status=\"failed\""), "{edit}: {body}");
        }

        let app = build_test_router(state.clone());
        let (_, body) = get_response(
            app,
            &format!(
                "/rest/updatePlaylist?{}&playlistId={id}&name=All",
                auth_query("")
            ),
        )
        .await;
        assert!(
            body.contains("status=\"ok\""),
            "a rename is allowed: {body}"
        );

        // One read from a file is named by it.
        let db = Database::open(state.pool.path()).unwrap();
        db.conn
            .execute("UPDATE playlists SET source_path = '/music/All.nsp'", [])
            .unwrap();
        drop(db);
        let app = build_test_router(state.clone());
        let (_, body) = get_response(
            app,
            &format!(
                "/rest/updatePlaylist?{}&playlistId={id}&name=Other",
                auth_query("")
            ),
        )
        .await;
        assert!(body.contains("status=\"failed\""), "{body}");
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
        assert_eq!(
            queries::play_count(&db.conn, koan_core::db::queries::LOCAL_USER, track_id).unwrap(),
            0
        );
    }

    /// A client that lost the answer to a batch sends it again; each play is
    /// recorded once.
    #[tokio::test]
    async fn a_scrobble_batch_sent_twice_records_each_play_once() {
        let (state, _dir) = test_state();
        seed_data(&state);
        let db = Database::open(state.pool.path()).unwrap();
        let track_id = queries::all_tracks(&db.conn).unwrap()[0].id;
        let path = format!(
            "/rest/scrobble?{}&id={track_id}&time=1000000&id={track_id}&time=2000000",
            auth_query("f=json")
        );
        for _ in 0..2 {
            let v = json_of(build_test_router(state.clone()), &path).await;
            assert_eq!(v["status"], "ok", "{v}");
        }
        assert_eq!(
            queries::play_count(&db.conn, koan_core::db::queries::LOCAL_USER, track_id).unwrap(),
            2
        );
    }

    /// A batch naming one track that does not exist records none of it, so a
    /// client retrying after fixing the batch does not double the rest.
    #[tokio::test]
    async fn a_scrobble_batch_with_a_missing_track_records_nothing() {
        let (state, _dir) = test_state();
        seed_data(&state);
        let db = Database::open(state.pool.path()).unwrap();
        let track_id = queries::all_tracks(&db.conn).unwrap()[0].id;

        let v = json_of(
            build_test_router(state),
            &format!(
                "/rest/scrobble?{}&id={track_id}&id=999999",
                auth_query("f=json")
            ),
        )
        .await;
        assert_eq!(v["error"]["code"], 70, "{v}");
        assert_eq!(
            queries::play_count(&db.conn, koan_core::db::queries::LOCAL_USER, track_id).unwrap(),
            0
        );
    }

    /// Plays scrobbled come back from `koanHistory` after the cursor; a play
    /// forgotten goes from the history and comes back as forgotten, and the
    /// next page from the cursor holds only what changed since.
    #[tokio::test]
    async fn history_pages_plays_and_forgettings_after_a_cursor() {
        let (state, _dir) = test_state();
        let shelves = seed_shelves(&state);
        let db = Database::open(state.pool.path()).unwrap();
        let (a, b) = (shelves[0], shelves[1]);
        let (a_uid, b_uid) = (
            uid_of(&state, queries::UidKind::Track, a),
            uid_of(&state, queries::UidKind::Track, b),
        );
        let get = |path: String| {
            let state = state.clone();
            async move { json_of(build_test_router(state), &path).await }
        };

        get(format!(
            "/rest/scrobble?{}&id={a_uid}&time=1000000&id={b_uid}&time=2000000",
            auth_query("f=json")
        ))
        .await;
        let v = get(format!("/rest/koanHistory?{}", auth_query("f=json"))).await;
        let page = &v["koanHistory"];
        let plays: Vec<(String, i64)> = page["play"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                (
                    p["id"].as_str().unwrap().to_owned(),
                    p["played"].as_i64().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            plays,
            [(a_uid.clone(), 1_000_000), (b_uid.clone(), 2_000_000)]
        );
        assert_eq!(page["more"], false);
        let cursor = page["cursor"].as_str().unwrap().to_owned();

        // Forgotten a second off: a play is named by when it started, and the
        // device that recorded it read its clock a moment apart.
        let v = get(format!(
            "/rest/koanForgetPlays?{}&id={a_uid}&time=1001000",
            auth_query("f=json")
        ))
        .await;
        assert_eq!(v["status"], "ok", "{v}");
        assert_eq!(
            queries::play_count(&db.conn, koan_core::db::queries::LOCAL_USER, a).unwrap(),
            0
        );
        let v = get(format!(
            "/rest/koanHistory?{}&since={cursor}",
            auth_query("f=json")
        ))
        .await;
        let page = &v["koanHistory"];
        assert!(page["play"].as_array().is_none_or(|p| p.is_empty()), "{v}");
        assert_eq!(page["forgotten"][0]["id"], a_uid.as_str());
        assert_eq!(page["forgotten"][0]["played"], 1_001_000);

        // Forgetting a play nobody has records nothing.
        let cursor = page["cursor"].as_str().unwrap().to_owned();
        get(format!(
            "/rest/koanForgetPlays?{}&id={a_uid}&time=9000000",
            auth_query("f=json")
        ))
        .await;
        let v = get(format!(
            "/rest/koanHistory?{}&since={cursor}",
            auth_query("f=json")
        ))
        .await;
        assert!(
            v["koanHistory"]["forgotten"]
                .as_array()
                .is_none_or(|f| f.is_empty()),
            "{v}"
        );

        // `through` forgets everything up to then, as one entry with no track.
        get(format!(
            "/rest/koanForgetPlays?{}&through=5000000",
            auth_query("f=json")
        ))
        .await;
        assert_eq!(
            queries::play_count(&db.conn, koan_core::db::queries::LOCAL_USER, b).unwrap(),
            0
        );
        let v = get(format!(
            "/rest/koanHistory?{}&since={cursor}",
            auth_query("f=json")
        ))
        .await;
        let forgotten = &v["koanHistory"]["forgotten"][0];
        assert!(forgotten.get("id").is_none_or(|id| id.is_null()), "{v}");
        assert_eq!(forgotten["played"], 5_000_000);
    }

    #[tokio::test]
    async fn test_star_album_by_prefixed_id() {
        let (state, _dir) = test_state();
        seed_data(&state);
        let db = Database::open(state.pool.path()).unwrap();
        let album_id = queries::all_tracks(&db.conn).unwrap()[0].album_id.unwrap();

        let app = build_test_router(state.clone());
        let (_, body) = get_response(
            app,
            &format!("/rest/star?{}&id=al-{}", auth_query(""), album_id),
        )
        .await;
        assert!(body.contains("status=\"ok\""), "{}", body);

        // The album, not one of its tracks.
        assert!(
            queries::load_favourites(&db.conn, koan_core::db::queries::LOCAL_USER)
                .unwrap()
                .is_empty()
        );
        assert!(
            queries::favourite_album_id_set(&db.conn, koan_core::db::queries::LOCAL_USER)
                .unwrap()
                .contains(&album_id)
        );
    }

    /// A uid names its row whatever endpoint takes it, and only a row of the
    /// kind the endpoint serves.
    #[tokio::test]
    async fn test_uids_name_one_row_of_one_kind() {
        let (state, _dir) = test_state();
        seed_data(&state);
        let db = Database::open(state.pool.path()).unwrap();
        let track = queries::all_tracks(&db.conn).unwrap()[0].clone();
        let album_id = track.album_id.unwrap();
        let song = uid_of(&state, queries::UidKind::Track, track.id);
        let album = uid_of(&state, queries::UidKind::Album, album_id);

        let get = |path: String| {
            let app = build_test_router(state.clone());
            async move { get_response(app, &path).await.1 }
        };
        let body = get(format!("/rest/getSong?{}&id={song}", auth_query(""))).await;
        assert!(body.contains("title=\"Test Song\""), "{body}");
        let body = get(format!("/rest/getSong?{}&id={album}", auth_query(""))).await;
        assert!(body.contains("status=\"failed\""), "{body}");
        let body = get(format!("/rest/getAlbum?{}&id={album}", auth_query(""))).await;
        assert!(body.contains("status=\"ok\""), "{body}");
        let body = get(format!(
            "/rest/getSong?{}&id=01a0ee12-0000-7000-8000-000000000000",
            auth_query("")
        ))
        .await;
        assert!(body.contains("status=\"failed\""), "{body}");

        // `star` takes any kind by `id`: the uid says which.
        let body = get(format!("/rest/star?{}&id={album}", auth_query(""))).await;
        assert!(body.contains("status=\"ok\""), "{body}");
        assert!(
            queries::load_favourites(&db.conn, queries::LOCAL_USER)
                .unwrap()
                .is_empty()
        );
        assert!(
            queries::favourite_album_id_set(&db.conn, queries::LOCAL_USER)
                .unwrap()
                .contains(&album_id)
        );
    }

    /// Clients star in bulk: `id`, `albumId` and `artistId` repeat, and all
    /// three can arrive in one request.
    #[tokio::test]
    async fn test_star_repeated_ids_albums_and_artists() {
        let (state, _dir) = test_state();
        let db = Database::open(state.pool.path()).unwrap();
        for (path, title, album) in [
            ("/music/a.flac", "Song A", "Album A"),
            ("/music/b.flac", "Song B", "Album B"),
        ] {
            queries::upsert_track(&db.conn, &track_meta(path, title, album, 1)).unwrap();
        }
        let tracks = queries::all_tracks(&db.conn).unwrap();
        let (a, b) = (&tracks[0], &tracks[1]);

        let app = build_test_router(state.clone());
        let (_, body) = get_response(
            app,
            &format!(
                "/rest/star?{}&id={}&id={}&albumId={}&artistId={}",
                auth_query(""),
                a.id,
                b.id,
                a.album_id.unwrap(),
                a.artist_id.unwrap()
            ),
        )
        .await;
        assert!(body.contains("status=\"ok\""), "{}", body);

        let app = build_test_router(state.clone());
        let (_, body) =
            get_response(app, &format!("/rest/getStarred2?{}&f=json", auth_query(""))).await;
        let starred: serde_json::Value = serde_json::from_str(&body).unwrap();
        let starred = &starred["subsonic-response"]["starred2"];
        assert_eq!(starred["song"].as_array().unwrap().len(), 2, "{}", body);
        assert_eq!(starred["album"].as_array().unwrap().len(), 1, "{}", body);
        assert_eq!(starred["artist"].as_array().unwrap().len(), 1, "{}", body);

        let app = build_test_router(state.clone());
        let (_, body) = get_response(
            app,
            &format!(
                "/rest/unstar?{}&albumId={}&artistId={}",
                auth_query(""),
                a.album_id.unwrap(),
                a.artist_id.unwrap()
            ),
        )
        .await;
        assert!(body.contains("status=\"ok\""), "{}", body);
        assert!(
            queries::favourite_album_id_set(&db.conn, koan_core::db::queries::LOCAL_USER)
                .unwrap()
                .is_empty()
        );
        assert!(
            queries::favourite_artist_id_set(&db.conn, koan_core::db::queries::LOCAL_USER)
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn test_star_without_an_id_is_an_error() {
        let (state, _dir) = test_state();
        let app = build_test_router(state);
        let (_, body) = get_response(app, &format!("/rest/star?{}", auth_query(""))).await;
        assert!(body.contains("status=\"failed\""), "{}", body);
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

    #[tokio::test]
    async fn test_download_serves_the_file() {
        let (state, dir) = test_state();
        let track_id = seed_local_file(&state, dir.path(), &[7u8; 1000]);

        let app = build_test_router(state);
        let (status, _, body) = get_with_range(
            app,
            &format!("/rest/download?{}&id={}", auth_query(""), track_id),
            "bytes=0-99",
        )
        .await;

        assert_eq!(status, StatusCode::PARTIAL_CONTENT);
        assert_eq!(body, vec![7u8; 100]);
    }

    // --- Cover art ---

    /// `getCoverArt` resolves an id by its kind: `id=5` meaning "album 5" must not
    /// serve track 5's art. Seeded so the two number spaces cross: album 2 holds
    /// track 3, so `al-2` and `mf-2` must land on different files.
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

        let first = |kind, id| {
            let groups = cover_tracks(&db, kind, id).unwrap();
            PathBuf::from(groups[0][0].path.clone().unwrap())
        };
        // `al-2` is Album Two — track 3's file, not track 2's.
        assert_eq!(
            first(Some(EntityKind::Album), album_two),
            dir.path().join("t3.flac")
        );
        // `mf-2` and a bare `2` are both track 2.
        assert_eq!(
            first(Some(EntityKind::Song), track2),
            dir.path().join("t2.flac")
        );
        assert_eq!(first(None, track2), dir.path().join("t2.flac"));
        // An artist resolves through their albums, one group each.
        let artist_id = queries::all_artists(&db.conn).unwrap()[0].id;
        assert_eq!(
            cover_tracks(&db, Some(EntityKind::Artist), artist_id)
                .unwrap()
                .len(),
            2
        );
    }

    /// A library that keeps its covers as `folder.jpg` beside the tracks, with
    /// nothing embedded, answers `getCoverArt` from the image.
    #[tokio::test]
    async fn test_cover_art_from_a_folder_image() {
        let (state, dir) = test_state();
        let album = dir.path().join("Album");
        std::fs::create_dir_all(&album).unwrap();
        let track = album.join("01.flac");
        std::fs::write(&track, b"no tags here").unwrap();
        let jpeg = {
            let mut out = Vec::new();
            image::codecs::jpeg::JpegEncoder::new(&mut out)
                .encode_image(&image::RgbImage::from_pixel(
                    64,
                    64,
                    image::Rgb([200, 30, 30]),
                ))
                .unwrap();
            out
        };
        std::fs::write(album.join("folder.jpg"), &jpeg).unwrap();
        let db = Database::open(state.pool.path()).unwrap();
        queries::upsert_track(
            &db.conn,
            &track_meta(track.to_str().unwrap(), "Song", "Album", 1),
        )
        .unwrap();
        let track_id = queries::track_id_by_path(&db.conn, track.to_str().unwrap())
            .unwrap()
            .unwrap();
        let album_id = queries::get_track_row(&db.conn, track_id)
            .unwrap()
            .unwrap()
            .album_id
            .unwrap();
        let app = build_test_router(state);
        for id in [format!("al-{album_id}"), format!("mf-{track_id}")] {
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/rest/getCoverArt?{}&id={id}", auth_query("")))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.headers()[header::CONTENT_TYPE], "image/jpeg");
            let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            assert_eq!(&body[..], &jpeg[..], "a small JPEG is served as it is");
        }
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
        assert_eq!(
            ids,
            [
                uid_of(&state, queries::UidKind::Track, first),
                uid_of(&state, queries::UidKind::Track, second)
            ]
        );
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

        // Accounts cannot use token auth, whatever the token: not even the
        // right one, after a password sign-in.
        let r = json_of(app(), "/rest/ping?u=mate&t=abc&s=def&f=json").await;
        assert_eq!(r["error"]["code"], 41);
        let r = json_of(app(), "/rest/ping?u=mate&p=hunter22&f=json").await;
        assert_eq!(r["status"], "ok");
        let right = format!("{:x}", md5::compute("hunter22salt"));
        let r = json_of(app(), &format!("/rest/ping?u=mate&t={right}&s=salt&f=json")).await;
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

        queries::record_play_at(
            &db.conn,
            koan_core::db::queries::LOCAL_USER,
            track.id,
            1_700_000_000,
            None,
            "local",
        )
        .unwrap();
        let r = json_of(
            app(),
            &format!("/rest/getSong?{}&id={}", auth_query("f=json"), track.id),
        )
        .await;
        assert_eq!(r["song"]["played"], "2023-11-14T22:13:20.000Z");
        let r = json_of(
            app(),
            &format!("/rest/getAlbum?{}&id={album_id}", auth_query("f=json")),
        )
        .await;
        assert_eq!(r["album"]["played"], "2023-11-14T22:13:20.000Z");

        // Another account's history is not this one's to see.
        let mate = "u=mate&p=hunter22&v=1.16.1&c=test&f=json";
        let r = json_of(app(), &format!("/rest/getSong?{mate}&id={}", track.id)).await;
        assert!(r["song"]["id"].is_string(), "{r}");
        assert!(r["song"].get("played").is_none(), "{r}");
        let r = json_of(app(), &format!("/rest/getAlbum?{mate}&id={album_id}")).await;
        assert!(r["album"]["id"].is_string(), "{r}");
        assert!(r["album"].get("played").is_none(), "{r}");

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
