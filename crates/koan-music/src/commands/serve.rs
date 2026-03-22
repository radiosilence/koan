use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use koan_core::db::connection::Database;
use koan_core::db::queries;
use serde::Deserialize;

use super::open_db;

const SUBSONIC_API_VERSION: &str = "1.16.1";
const SUBSONIC_XMLNS: &str = "http://subsonic.org/restapi";

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct AppState {
    db_path: std::path::PathBuf,
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
}

// Convenience: allow `?` on strings
impl From<String> for SubsonicError {
    fn from(s: String) -> Self {
        Self {
            code: SubsonicErrorCode::Generic,
            message: s,
        }
    }
}

// ---------------------------------------------------------------------------
// Query params (common to all endpoints)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct SubsonicParams {
    /// Username
    u: Option<String>,
    /// Auth token: md5(password + salt)
    t: Option<String>,
    /// Salt
    s: Option<String>,
    /// Plaintext password (legacy auth, also supported)
    p: Option<String>,
    /// API version
    #[allow(dead_code)]
    v: Option<String>,
    /// Client name
    #[allow(dead_code)]
    c: Option<String>,
    /// Response format: xml (default) or json
    f: Option<String>,
}

impl SubsonicParams {
    fn wants_json(&self) -> bool {
        self.f.as_deref() == Some("json")
    }
}

// ---------------------------------------------------------------------------
// Response builder
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
                [("content-type", "application/json; charset=utf-8")],
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
                [("content-type", "application/xml; charset=utf-8")],
                xml,
            )
                .into_response()
        }
    }
}

// ---------------------------------------------------------------------------
// XML builder — lightweight, no deps
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
    /// For JSON: should this node's children be rendered as an array?
    is_array: bool,
    /// Tag name of array children (for JSON grouping)
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

    /// Mark this node as containing an array of children with the given tag.
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

        // Attributes become properties
        for (k, v) in &self.attrs {
            obj.insert(k.clone(), serde_json::Value::String(v.clone()));
        }

        if self.is_array {
            // Group children by tag into arrays
            let child_tag = self.array_child_tag.as_deref().unwrap_or("item");
            let arr: Vec<serde_json::Value> =
                self.children.iter().map(|c| c.to_json_value()).collect();
            obj.insert(child_tag.into(), serde_json::Value::Array(arr));
        } else {
            // Group children by tag — multiple same-tag children become arrays
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
                [("content-type", "application/json; charset=utf-8")],
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
                [("content-type", "application/xml; charset=utf-8")],
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

fn validate_auth(params: &SubsonicParams, state: &AppState) -> Result<(), SubsonicError> {
    let username = params
        .u
        .as_deref()
        .ok_or_else(|| SubsonicError::missing_param("u"))?;

    if username != state.username {
        return Err(SubsonicError::wrong_auth());
    }

    // Token-based auth: t = md5(password + s)
    if let (Some(token), Some(salt)) = (params.t.as_deref(), params.s.as_deref()) {
        let expected = format!("{:x}", md5::compute(format!("{}{}", state.password, salt)));
        if token == expected {
            return Ok(());
        }
        return Err(SubsonicError::wrong_auth());
    }

    // Legacy plaintext password auth (with optional enc: prefix)
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
        .filter_map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

// ---------------------------------------------------------------------------
// Endpoint: ping
// ---------------------------------------------------------------------------

async fn ping(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    if let Err(e) = validate_auth(&params, &state) {
        return SubsonicResponse::error(params.wants_json(), &e);
    }
    SubsonicResponse::ok(params.wants_json()).build()
}

// ---------------------------------------------------------------------------
// Endpoint: getLicense
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Endpoint: getArtists
// ---------------------------------------------------------------------------

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

    // Group by first letter
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
                    .map(|ch| ch.is_ascii_alphabetic())
                    .unwrap_or(false)
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

fn album_counts_by_artist(db: &Database) -> BTreeMap<i64, i64> {
    let albums = queries::all_albums(&db.conn).unwrap_or_default();
    let mut map: BTreeMap<i64, i64> = BTreeMap::new();
    for album in albums {
        *map.entry(album.artist_id).or_insert(0) += 1;
    }
    map
}

// ---------------------------------------------------------------------------
// Endpoint: getArtist
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct IdParam {
    id: Option<String>,
    // Auth params are also present but handled by SubsonicParams
    #[serde(flatten)]
    auth: SubsonicParams,
}

async fn get_artist(State(state): State<Arc<AppState>>, Query(params): Query<IdParam>) -> Response {
    if let Err(e) = validate_auth(&params.auth, &state) {
        return SubsonicResponse::error(params.auth.wants_json(), &e);
    }
    let json = params.auth.wants_json();

    let id_str = match params.id.as_deref() {
        Some(id) => id,
        None => return SubsonicResponse::error(json, &SubsonicError::missing_param("id")),
    };
    let artist_id: i64 = match id_str.parse() {
        Ok(id) => id,
        Err(_) => return SubsonicResponse::error(json, &SubsonicError::not_found("Artist")),
    };

    let db = match state.open_db() {
        Ok(db) => db,
        Err(e) => return SubsonicResponse::error(json, &e),
    };

    // Get artist by ID — no dedicated query, so scan all_artists.
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

// ---------------------------------------------------------------------------
// Endpoint: getAlbum
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Endpoint: getAlbumList2
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct AlbumListParams {
    #[serde(rename = "type")]
    list_type: Option<String>,
    size: Option<i64>,
    offset: Option<i64>,
    #[serde(flatten)]
    auth: SubsonicParams,
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

    // Sort based on type
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
            // Deterministic-ish shuffle based on current second
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
        _ => {} // default order from DB
    }

    // Paginate
    let page: Vec<_> = albums.into_iter().skip(offset).take(size).collect();

    let mut list_node = XmlNode::new("albumList2").array_of("album");
    for album in &page {
        list_node = list_node.child(album_to_xml_node(album, album.total_tracks));
    }

    SubsonicResponse::ok(json).child(list_node).build()
}

// ---------------------------------------------------------------------------
// Endpoint: getSong
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Track → XmlNode helper
// ---------------------------------------------------------------------------

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

fn track_to_xml_node(track: &queries::TrackRow) -> XmlNode {
    let duration_secs = track.duration_ms.map(|ms| ms / 1000);
    XmlNode::new("song")
        .attr("id", &track.id.to_string())
        .attr("title", &track.title)
        .attr("album", &track.album_title)
        .attr("artist", &track.artist_name)
        .attr_opt_i32("track", track.track_number)
        .attr_opt_i32("discNumber", track.disc)
        .attr_opt_i64("duration", duration_secs)
        .attr_opt_i32("bitRate", track.bitrate)
        .attr_opt(
            "suffix",
            track.codec.as_deref().map(|c| c.to_lowercase()).as_deref(),
        )
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

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

fn build_router(state: Arc<AppState>) -> axum::Router {
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
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

pub fn cmd_serve(port: Option<u16>, username: &str, password: &str) {
    let _db = open_db(); // ensure DB exists
    let db_path = koan_core::config::db_path();

    let port = port.unwrap_or(4040);

    let state = Arc::new(AppState {
        db_path,
        username: username.to_string(),
        password: password.to_string(),
    });

    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    rt.block_on(async {
        let app = build_router(state);
        let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
        eprintln!("koan subsonic server listening on http://0.0.0.0:{}", port);
        eprintln!("  endpoints: /rest/ping, /rest/getArtists, /rest/getArtist, /rest/getAlbum, /rest/getAlbumList2, /rest/getSong");

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
    use axum::body::Body;
    use axum::http::Request;
    use koan_core::db::connection::Database;
    use koan_core::db::queries::{self, TrackMeta};
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

    #[tokio::test]
    async fn test_ping_ok() {
        let (state, _dir) = test_state();
        let app = build_router(state);
        let (status, body) = get_response(app, &format!("/rest/ping?{}", auth_query(""))).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("status=\"ok\""));
    }

    #[tokio::test]
    async fn test_ping_json() {
        let (state, _dir) = test_state();
        let app = build_router(state);
        let (status, body) =
            get_response(app, &format!("/rest/ping?{}", auth_query("f=json"))).await;
        assert_eq!(status, StatusCode::OK);
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["subsonic-response"]["status"], "ok");
    }

    #[tokio::test]
    async fn test_ping_wrong_password() {
        let (state, _dir) = test_state();
        let app = build_router(state);
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
        let app = build_router(state);
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
    async fn test_get_license() {
        let (state, _dir) = test_state();
        let app = build_router(state);
        let (_, body) = get_response(app, &format!("/rest/getLicense?{}", auth_query(""))).await;
        assert!(body.contains("license"));
        assert!(body.contains("valid=\"true\""));
    }

    #[tokio::test]
    async fn test_get_artists_empty() {
        let (state, _dir) = test_state();
        let app = build_router(state);
        let (_, body) = get_response(app, &format!("/rest/getArtists?{}", auth_query(""))).await;
        assert!(body.contains("status=\"ok\""));
        assert!(body.contains("<artists"));
    }

    #[tokio::test]
    async fn test_get_artists_with_data() {
        let (state, _dir) = test_state();
        seed_data(&state);
        let app = build_router(state);
        let (_, body) = get_response(app, &format!("/rest/getArtists?{}", auth_query(""))).await;
        assert!(body.contains("Test Artist"));
    }

    #[tokio::test]
    async fn test_get_artist_by_id() {
        let (state, _dir) = test_state();
        seed_data(&state);

        // Find the artist ID
        let db = Database::open(&state.db_path).unwrap();
        let artists = queries::all_artists(&db.conn).unwrap();
        let artist = &artists[0];

        let app = build_router(state);
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

        let app = build_router(state);
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

        let app = build_router(state);
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

        let app = build_router(state);
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
        let app = build_router(state);
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

        let app = build_router(state);
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
        let app = build_router(state);
        let (status, body) =
            get_response(app, &format!("/rest/ping.view?{}", auth_query(""))).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("status=\"ok\""));
    }

    #[tokio::test]
    async fn test_legacy_password_auth() {
        let (state, _dir) = test_state();
        let app = build_router(state);
        let (_, body) = get_response(app, "/rest/ping?u=testuser&p=testpass&v=1.16.1&c=test").await;
        assert!(body.contains("status=\"ok\""));
    }

    #[tokio::test]
    async fn test_xml_escaping() {
        // Verify our xml_escape function handles special chars
        assert_eq!(xml_escape("A&B"), "A&amp;B");
        assert_eq!(xml_escape("a<b>c"), "a&lt;b&gt;c");
        assert_eq!(xml_escape(r#"say "hi""#), "say &quot;hi&quot;");
    }
}
