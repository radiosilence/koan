//! Subsonic-compatible REST server.
//!
//! Implements a subset of the Subsonic API backed by the local koan database.
//! Auth uses MD5+salt tokens (standard Subsonic scheme). Responses are JSON
//! wrapped in the `subsonic-response` envelope.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::IntoResponse;
use axum::routing::get;
use koan_core::config::Config;
use koan_core::db::connection::Database;
use koan_core::db::queries;
use serde::Serialize;

use super::open_db;

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

struct AppState {
    db_path: PathBuf,
    username: String,
    password: String,
}

impl AppState {
    fn open_db(&self) -> Result<Database, String> {
        Database::open(&self.db_path).map_err(|e| format!("Database error: {e}"))
    }
}

type SharedState = Arc<AppState>;

// ---------------------------------------------------------------------------
// Subsonic response envelope
// ---------------------------------------------------------------------------

const API_VERSION: &str = "1.16.1";
const SERVER_NAME: &str = "koan";

#[derive(Serialize)]
struct SubsonicEnvelope<T: Serialize> {
    #[serde(rename = "subsonic-response")]
    subsonic_response: SubsonicResponseBody<T>,
}

#[derive(Serialize)]
struct SubsonicResponseBody<T: Serialize> {
    status: &'static str,
    version: &'static str,
    #[serde(rename = "type")]
    server_type: &'static str,
    #[serde(flatten)]
    payload: T,
}

#[derive(Serialize)]
struct ErrorPayload {
    error: SubsonicError,
}

#[derive(Serialize)]
struct SubsonicError {
    code: i32,
    message: String,
}

#[derive(Serialize)]
struct Empty {}

fn ok_response<T: Serialize>(payload: T) -> axum::Json<SubsonicEnvelope<T>> {
    axum::Json(SubsonicEnvelope {
        subsonic_response: SubsonicResponseBody {
            status: "ok",
            version: API_VERSION,
            server_type: SERVER_NAME,
            payload,
        },
    })
}

fn error_response(
    code: i32,
    message: impl Into<String>,
) -> axum::Json<SubsonicEnvelope<ErrorPayload>> {
    axum::Json(SubsonicEnvelope {
        subsonic_response: SubsonicResponseBody {
            status: "failed",
            version: API_VERSION,
            server_type: SERVER_NAME,
            payload: ErrorPayload {
                error: SubsonicError {
                    code,
                    message: message.into(),
                },
            },
        },
    })
}

// ---------------------------------------------------------------------------
// Auth
// ---------------------------------------------------------------------------

/// Verify Subsonic token auth: t = md5(password + s).
fn verify_auth(params: &HashMap<String, String>, state: &AppState) -> Result<(), (i32, String)> {
    let user = params.get("u").ok_or((10, "Missing parameter: u".into()))?;
    let token = params.get("t").ok_or((10, "Missing parameter: t".into()))?;
    let salt = params.get("s").ok_or((10, "Missing parameter: s".into()))?;

    if user != &state.username {
        return Err((40, "Wrong username or password".into()));
    }

    let expected = format!("{:x}", md5::compute(format!("{}{}", state.password, salt)));
    if token != &expected {
        return Err((40, "Wrong username or password".into()));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Subsonic song type
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct SubsonicSong {
    id: String,
    title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    album: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    artist: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    track: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    disc_number: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    genre: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bit_rate: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    suffix: Option<String>,
}

fn track_to_song(t: &queries::TrackRow) -> SubsonicSong {
    SubsonicSong {
        id: t.id.to_string(),
        title: t.title.clone(),
        album: if t.album_title.is_empty() {
            None
        } else {
            Some(t.album_title.clone())
        },
        artist: if t.artist_name.is_empty() {
            None
        } else {
            Some(t.artist_name.clone())
        },
        track: t.track_number,
        disc_number: t.disc,
        duration: t.duration_ms.map(|ms| ms / 1000),
        genre: t.genre.clone(),
        bit_rate: t.bitrate,
        suffix: t.codec.as_ref().map(|c| c.to_lowercase()),
    }
}

// ---------------------------------------------------------------------------
// Endpoint handlers
// ---------------------------------------------------------------------------

async fn ping(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if let Err((code, msg)) = verify_auth(&params, &state) {
        return error_response(code, msg).into_response();
    }
    ok_response(Empty {}).into_response()
}

/// Resolve a track ID to its favourites path (prefers local, falls back to cached).
fn track_fav_path(t: &queries::TrackRow) -> &str {
    t.path.as_deref().or(t.cached_path.as_deref()).unwrap_or("")
}

/// Shared logic for star/unstar — looks up the track, then calls `op` on its path.
/// Shared logic for star/unstar — looks up the track, then calls `op` on its path.
fn toggle_star(
    state: &AppState,
    params: &HashMap<String, String>,
    op: fn(&rusqlite::Connection, &std::path::Path) -> rusqlite::Result<()>,
) -> axum::response::Response {
    let Some(id_str) = params.get("id") else {
        return error_response(10, "Missing parameter: id").into_response();
    };
    let Ok(track_id) = id_str.parse::<i64>() else {
        return error_response(0, "Invalid id").into_response();
    };

    let db = match state.open_db() {
        Ok(db) => db,
        Err(e) => return error_response(0, e).into_response(),
    };

    let track = match queries::get_track_row(&db.conn, track_id) {
        Ok(Some(t)) => t,
        Ok(None) => return error_response(70, "Track not found").into_response(),
        Err(e) => return error_response(0, format!("Database error: {e}")).into_response(),
    };

    if let Err(e) = op(&db.conn, std::path::Path::new(track_fav_path(&track))) {
        return error_response(0, format!("Database error: {e}")).into_response();
    }

    ok_response(Empty {}).into_response()
}

async fn star(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if let Err((code, msg)) = verify_auth(&params, &state) {
        return error_response(code, msg).into_response();
    }
    toggle_star(&state, &params, queries::add_favourite)
}

async fn unstar(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if let Err((code, msg)) = verify_auth(&params, &state) {
        return error_response(code, msg).into_response();
    }
    toggle_star(&state, &params, queries::remove_favourite)
}

#[derive(Serialize)]
struct Starred2Payload {
    starred2: Starred2Songs,
}

#[derive(Serialize)]
struct Starred2Songs {
    song: Vec<SubsonicSong>,
}

async fn get_starred2(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if let Err((code, msg)) = verify_auth(&params, &state) {
        return error_response(code, msg).into_response();
    }

    let db = match state.open_db() {
        Ok(db) => db,
        Err(e) => return error_response(0, e).into_response(),
    };

    let favourites = match queries::load_favourites(&db.conn) {
        Ok(f) => f,
        Err(e) => return error_response(0, format!("Database error: {e}")).into_response(),
    };

    let mut songs = Vec::new();
    for fav_path in &favourites {
        let path_str = fav_path.to_string_lossy();
        if let Ok(Some(track_id)) = queries::track_id_by_path(&db.conn, &path_str)
            && let Ok(Some(track)) = queries::get_track_row(&db.conn, track_id)
        {
            songs.push(track_to_song(&track));
        }
    }

    ok_response(Starred2Payload {
        starred2: Starred2Songs { song: songs },
    })
    .into_response()
}

async fn scrobble(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if let Err((code, msg)) = verify_auth(&params, &state) {
        return error_response(code, msg).into_response();
    }

    let Some(id_str) = params.get("id") else {
        return error_response(10, "Missing parameter: id").into_response();
    };
    let Ok(track_id) = id_str.parse::<i64>() else {
        return error_response(0, "Invalid id").into_response();
    };

    let db = match state.open_db() {
        Ok(db) => db,
        Err(e) => return error_response(0, e).into_response(),
    };

    match queries::get_track_row(&db.conn, track_id) {
        Ok(Some(_)) => {}
        Ok(None) => return error_response(70, "Track not found").into_response(),
        Err(e) => return error_response(0, format!("Database error: {e}")).into_response(),
    }

    let result =
        if let Some(time_ms) = params.get("time").and_then(|t| t.parse::<i64>().ok()) {
            // Caller-provided timestamp (ms epoch) — convert to seconds for play_history.
            db.conn.execute(
            "INSERT INTO play_history (track_id, played_at, duration_ms) VALUES (?1, ?2, NULL)",
            rusqlite::params![track_id, time_ms / 1000],
        ).map(|_| ())
         .map_err(|e| e.into())
        } else {
            queries::record_play(&db.conn, track_id, None)
        };

    if let Err(e) = result {
        return error_response(0, format!("Database error: {e}")).into_response();
    }

    ok_response(Empty {}).into_response()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RandomSongsPayload {
    random_songs: RandomSongsList,
}

#[derive(Serialize)]
struct RandomSongsList {
    song: Vec<SubsonicSong>,
}

async fn get_random_songs(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if let Err((code, msg)) = verify_auth(&params, &state) {
        return error_response(code, msg).into_response();
    }

    let size: u32 = params
        .get("size")
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);

    let genre = params.get("genre").cloned();

    // Oversample when genre-filtering so we're likely to fill the requested size.
    let fetch_count = if genre.is_some() { size * 5 } else { size };

    let db = match state.open_db() {
        Ok(db) => db,
        Err(e) => return error_response(0, e).into_response(),
    };

    let tracks = match queries::random_tracks(&db.conn, fetch_count, None) {
        Ok(t) => t,
        Err(e) => return error_response(0, format!("Database error: {e}")).into_response(),
    };

    let songs: Vec<SubsonicSong> = tracks
        .iter()
        .filter(|t| {
            genre
                .as_deref()
                .is_none_or(|g| t.genre.as_deref() == Some(g))
        })
        .take(size as usize)
        .map(track_to_song)
        .collect();

    ok_response(RandomSongsPayload {
        random_songs: RandomSongsList { song: songs },
    })
    .into_response()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SimilarSongs2Payload {
    similar_songs2: SimilarSongsList,
}

#[derive(Serialize)]
struct SimilarSongsList {
    song: Vec<SubsonicSong>,
}

async fn get_similar_songs2(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if let Err((code, msg)) = verify_auth(&params, &state) {
        return error_response(code, msg).into_response();
    }

    let Some(id_str) = params.get("id") else {
        return error_response(10, "Missing parameter: id").into_response();
    };
    let Ok(track_id) = id_str.parse::<i64>() else {
        return error_response(0, "Invalid id").into_response();
    };

    let count: usize = params
        .get("count")
        .and_then(|c| c.parse().ok())
        .unwrap_or(50);

    let db = match state.open_db() {
        Ok(db) => db,
        Err(e) => return error_response(0, e).into_response(),
    };

    let track = match queries::get_track_row(&db.conn, track_id) {
        Ok(Some(t)) => t,
        Ok(None) => return error_response(70, "Track not found").into_response(),
        Err(e) => return error_response(0, format!("Database error: {e}")).into_response(),
    };

    let Some(artist_id) = track.artist_id else {
        return ok_response(SimilarSongs2Payload {
            similar_songs2: SimilarSongsList { song: vec![] },
        })
        .into_response();
    };

    let similar = match queries::get_similar_artists(&db.conn, artist_id) {
        Ok(s) => s,
        Err(e) => return error_response(0, format!("Database error: {e}")).into_response(),
    };

    let mut songs = Vec::new();
    for (artist_row, _score) in &similar {
        if songs.len() >= count {
            break;
        }
        let tracks = match queries::tracks_for_artist(&db.conn, artist_row.id) {
            Ok(t) => t,
            Err(_) => continue,
        };
        for t in &tracks {
            if songs.len() >= count {
                break;
            }
            songs.push(track_to_song(t));
        }
    }

    ok_response(SimilarSongs2Payload {
        similar_songs2: SimilarSongsList { song: songs },
    })
    .into_response()
}

// ---------------------------------------------------------------------------
// Router + entry point
// ---------------------------------------------------------------------------

fn build_router(state: SharedState) -> axum::Router {
    axum::Router::new()
        .route("/rest/ping", get(ping))
        .route("/rest/ping.view", get(ping))
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

pub fn cmd_serve(port: Option<u16>) {
    let _db = open_db();
    let db_path = koan_core::config::db_path();
    let cfg = Config::load().unwrap_or_default();

    let port = port.unwrap_or(4040);

    let password = super::get_remote_password(&cfg);

    let state = Arc::new(AppState {
        db_path,
        username: cfg.remote.username.clone(),
        password,
    });

    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    rt.block_on(async {
        let app = build_router(state);
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
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn make_auth_params(user: &str, pass: &str) -> HashMap<String, String> {
        let salt = "testsalt";
        let token = format!("{:x}", md5::compute(format!("{}{}", pass, salt)));
        let mut params = HashMap::new();
        params.insert("u".into(), user.into());
        params.insert("t".into(), token);
        params.insert("s".into(), salt.into());
        params.insert("v".into(), API_VERSION.into());
        params.insert("c".into(), "test".into());
        params.insert("f".into(), "json".into());
        params
    }

    fn test_db() -> Database {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        koan_core::db::schema::create_tables(&conn).unwrap();
        Database { conn }
    }

    fn test_state() -> AppState {
        // Auth tests don't actually open the DB, so a dummy path is fine.
        AppState {
            db_path: PathBuf::from("/dev/null"),
            username: "admin".into(),
            password: "secret".into(),
        }
    }

    #[test]
    fn test_verify_auth_success() {
        let state = test_state();
        let params = make_auth_params("admin", "secret");
        assert!(verify_auth(&params, &state).is_ok());
    }

    #[test]
    fn test_verify_auth_wrong_password() {
        let state = test_state();
        let params = make_auth_params("admin", "wrong");
        let err = verify_auth(&params, &state).unwrap_err();
        assert_eq!(err.0, 40);
    }

    #[test]
    fn test_verify_auth_wrong_user() {
        let state = test_state();
        let params = make_auth_params("nobody", "secret");
        let err = verify_auth(&params, &state).unwrap_err();
        assert_eq!(err.0, 40);
    }

    #[test]
    fn test_verify_auth_missing_params() {
        let state = test_state();
        let params = HashMap::new();
        let err = verify_auth(&params, &state).unwrap_err();
        assert_eq!(err.0, 10);
    }

    #[test]
    fn test_track_to_song_conversion() {
        let track = queries::TrackRow {
            id: 42,
            album_id: Some(1),
            artist_id: Some(1),
            artist_name: "Boards of Canada".into(),
            album_artist_name: "Boards of Canada".into(),
            album_title: "Music Has the Right to Children".into(),
            disc: Some(1),
            track_number: Some(3),
            title: "Turquoise Hexagon Sun".into(),
            duration_ms: Some(330_000),
            path: Some("/music/boc/ths.flac".into()),
            codec: Some("FLAC".into()),
            sample_rate: Some(44100),
            bit_depth: Some(16),
            channels: Some(2),
            bitrate: Some(1000),
            genre: Some("IDM".into()),
            source: "local".into(),
            remote_id: None,
            cached_path: None,
        };

        let song = track_to_song(&track);
        assert_eq!(song.id, "42");
        assert_eq!(song.title, "Turquoise Hexagon Sun");
        assert_eq!(
            song.album.as_deref(),
            Some("Music Has the Right to Children")
        );
        assert_eq!(song.artist.as_deref(), Some("Boards of Canada"));
        assert_eq!(song.track, Some(3));
        assert_eq!(song.duration, Some(330)); // seconds
        assert_eq!(song.genre.as_deref(), Some("IDM"));
        assert_eq!(song.suffix.as_deref(), Some("flac"));
    }

    #[test]
    fn test_track_to_song_empty_strings_become_none() {
        let track = queries::TrackRow {
            id: 1,
            album_id: None,
            artist_id: None,
            artist_name: String::new(),
            album_artist_name: String::new(),
            album_title: String::new(),
            disc: None,
            track_number: None,
            title: "Untitled".into(),
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

        let song = track_to_song(&track);
        assert!(song.album.is_none());
        assert!(song.artist.is_none());
        assert!(song.duration.is_none());
        assert!(song.suffix.is_none());
    }

    #[test]
    fn test_ok_response_shape() {
        let resp = ok_response(Empty {});
        let body = resp.0;
        assert_eq!(body.subsonic_response.status, "ok");
        assert_eq!(body.subsonic_response.version, API_VERSION);
        assert_eq!(body.subsonic_response.server_type, SERVER_NAME);
    }

    #[test]
    fn test_error_response_shape() {
        let resp = error_response(40, "Auth failed");
        let body = resp.0;
        assert_eq!(body.subsonic_response.status, "failed");
        assert_eq!(body.subsonic_response.payload.error.code, 40);
        assert_eq!(body.subsonic_response.payload.error.message, "Auth failed");
    }

    // --- Integration-style tests using in-memory DB ---

    fn setup_db_with_tracks() -> Database {
        let db = test_db();

        db.conn
            .execute("INSERT INTO artists (id, name) VALUES (1, 'Autechre')", [])
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO albums (id, title, artist_id) VALUES (1, 'Tri Repetae', 1)",
                [],
            )
            .unwrap();
        for i in 1..=5 {
            db.conn
                .execute(
                    "INSERT INTO tracks (id, title, artist_id, album_id, source, path, genre, duration_ms, codec)
                     VALUES (?1, ?2, 1, 1, 'local', ?3, 'IDM', 300000, 'FLAC')",
                    rusqlite::params![i, format!("Track {}", i), format!("/music/tr/track{}.flac", i)],
                )
                .unwrap();
        }

        db
    }

    #[test]
    fn test_star_and_unstar_roundtrip() {
        let db = setup_db_with_tracks();

        // Star track 1.
        queries::add_favourite(&db.conn, std::path::Path::new("/music/tr/track1.flac")).unwrap();
        let favs = queries::load_favourites(&db.conn).unwrap();
        assert!(favs.contains(std::path::Path::new("/music/tr/track1.flac")));

        // Unstar.
        queries::remove_favourite(&db.conn, std::path::Path::new("/music/tr/track1.flac")).unwrap();
        let favs = queries::load_favourites(&db.conn).unwrap();
        assert!(!favs.contains(std::path::Path::new("/music/tr/track1.flac")));
    }

    #[test]
    fn test_scrobble_inserts_play_history() {
        let db = setup_db_with_tracks();

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        db.conn
            .execute(
                "INSERT INTO play_history (track_id, played_at, duration_ms) VALUES (?1, ?2, NULL)",
                rusqlite::params![1, now],
            )
            .unwrap();

        let count: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM play_history WHERE track_id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_random_songs_returns_correct_count() {
        let db = setup_db_with_tracks();
        let tracks = queries::random_tracks(&db.conn, 3, None).unwrap();
        assert_eq!(tracks.len(), 3);
    }

    #[test]
    fn test_random_songs_respects_max() {
        let db = setup_db_with_tracks();
        // Ask for more than exists.
        let tracks = queries::random_tracks(&db.conn, 100, None).unwrap();
        assert_eq!(tracks.len(), 5);
    }

    #[test]
    fn test_similar_songs_empty_when_no_relations() {
        let db = setup_db_with_tracks();
        let similar = queries::get_similar_artists(&db.conn, 1).unwrap();
        assert!(similar.is_empty());
    }

    #[test]
    fn test_similar_songs_finds_related_tracks() {
        let db = setup_db_with_tracks();

        // Add a second artist with tracks.
        db.conn
            .execute(
                "INSERT INTO artists (id, name) VALUES (2, 'Squarepusher')",
                [],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO albums (id, title, artist_id) VALUES (2, 'Feed Me Weird Things', 2)",
                [],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO tracks (id, title, artist_id, album_id, source, path)
                 VALUES (10, 'Squarepusher Theme', 2, 2, 'local', '/music/sq/theme.flac')",
                [],
            )
            .unwrap();

        // Link artists as similar.
        queries::save_similar_artists(&db.conn, 1, &[(2, 0.9)], "subsonic").unwrap();

        let similar = queries::get_similar_artists(&db.conn, 1).unwrap();
        assert_eq!(similar.len(), 1);
        assert_eq!(similar[0].0.name, "Squarepusher");

        // Get tracks for the similar artist.
        let tracks = queries::tracks_for_artist(&db.conn, similar[0].0.id).unwrap();
        assert!(!tracks.is_empty());
        assert_eq!(tracks[0].title, "Squarepusher Theme");
    }
}
