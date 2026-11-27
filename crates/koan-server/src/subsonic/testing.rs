//! What the Subsonic tests share: a server on a throwaway database, a
//! seeded library and requests against it.

use super::*;
pub(super) use axum::body::Body;
use axum::http::Request;
pub(super) use koan_core::db::queries::TrackMeta;
pub(super) use tower::ServiceExt;

pub(super) fn test_state() -> (Arc<AppState>, tempfile::TempDir) {
    // Playlist writes read config to push upstream.
    koan_core::config::isolate_config_for_tests();
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let db = Database::open(&db_path).unwrap();
    koan_core::db::schema::create_tables(&db.conn).unwrap();

    koan_core::db::queries::auth::create_user(&db.conn, "mate", "hunter22", Role::Readonly)
        .unwrap();
    koan_core::db::queries::auth::create_user(&db.conn, "owner", "sesame", Role::Admin).unwrap();

    let pool = Arc::new(Pool::new(db_path));
    let state = Arc::new(AppState {
        users: Arc::new(PasswordVerifier::new(pool.clone())),
        pool,
        username: "testuser".into(),
        password: Some("testpass".into()),
        app_key: Some(koan_core::auth::app_password_key(b"test signing key")),
        totp_key: Some(koan_auth::second_factor::totp_key(b"test signing key")),
        upstream: None,
        http: reqwest::Client::new(),
        covers: Arc::new(crate::covers::Covers::new(dir.path().join("covers"))),
        last_modified: Default::default(),
        transcoder: None,
        push: false,
        server_url: crate::ServerUrl::Fixed(None),
        stop: Default::default(),
        links: Default::default(),
        host_key: None,
    });
    (state, dir)
}

pub(super) fn build_test_router(state: Arc<AppState>) -> axum::Router {
    let stop = axum::Extension(state.stop.clone());
    register_subsonic_routes(axum::Router::new())
        .with_state(state)
        .layer(stop)
        .layer(axum::middleware::from_fn(form_post))
}

pub(super) fn auth_query(extra: &str) -> String {
    let salt = "abc123";
    let token = format!("{:x}", md5::compute(format!("testpass{}", salt)));
    let base = format!("u=testuser&t={}&s={}&v=1.16.1&c=test", token, salt);
    if extra.is_empty() {
        base
    } else {
        format!("{}&{}", base, extra)
    }
}

pub(super) fn track_meta(path: &str, title: &str, album: &str, track_number: i32) -> TrackMeta {
    TrackMeta {
        title: title.into(),
        artist: "Test Artist".into(),
        album: album.into(),
        album_artist: Some("Test Artist".into()),
        track_number: Some(track_number),
        disc: Some(1),
        disc_total: None,
        duration_ms: Some(240_000),
        codec: Some("FLAC".into()),
        sample_rate: Some(44100),
        bit_depth: Some(16),
        channels: Some(2),
        bitrate: Some(1411),
        genre: Some("Rock".into()),
        path: Some(path.into()),
        server_path: None,
        date: Some("2020".into()),
        label: None,
        release_types: None,
        original_date: None,
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
        credits: Default::default(),
    }
}

/// What the API publishes as this row's id.
pub(super) fn uid_of(state: &AppState, kind: queries::UidKind, id: i64) -> String {
    let db = Database::open(state.pool.path()).unwrap();
    queries::uids_for(&db.conn, kind, [id]).unwrap()[&id].clone()
}

pub(super) fn seed_data(state: &AppState) {
    let db = Database::open(state.pool.path()).unwrap();
    queries::upsert_track(
        &db.conn,
        &track_meta("/music/test.flac", "Test Song", "Test Album", 1),
    )
    .unwrap();
}

/// Seed a track backed by a file that actually exists, for the paths that
/// read bytes off disk.
pub(super) fn seed_local_file(state: &AppState, dir: &std::path::Path, bytes: &[u8]) -> i64 {
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

pub(super) async fn get_response(app: axum::Router, uri: &str) -> (StatusCode, String) {
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

/// Three records by one artist, each with its own genre and year.
pub(super) fn seed_shelves(state: &AppState) -> Vec<i64> {
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

pub(super) fn names(list: &serde_json::Value) -> Vec<String> {
    list.as_array()
        .map(|a| {
            a.iter()
                .map(|x| x["title"].as_str().unwrap().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

pub(super) async fn album_list2(state: &Arc<AppState>, params: &str) -> Vec<String> {
    let v = json_of(
        build_test_router(state.clone()),
        &format!("/rest/getAlbumList2?{}&{params}", auth_query("f=json")),
    )
    .await;
    names(&v["albumList2"]["album"])
}

/// The rows a listing names, by the uids it publishes.
pub(super) fn ids(state: &AppState, list: &serde_json::Value) -> Vec<i64> {
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

pub(super) async fn json_of(app: axum::Router, uri: &str) -> serde_json::Value {
    let (_, body) = get_response(app, uri).await;
    let parsed: serde_json::Value = serde_json::from_str(&body).expect(&body);
    parsed["subsonic-response"].clone()
}

pub(super) async fn post_form(app: axum::Router, uri: &str, form: &str) -> String {
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

pub(super) fn api_key(state: &AppState, username: &str) -> String {
    let db = Database::open(state.pool.path()).unwrap();
    let user = koan_core::db::queries::auth::get_user_by_username(&db.conn, username)
        .unwrap()
        .unwrap();
    queries::api_keys::create_api_key(&db.conn, user.id, "test")
        .unwrap()
        .1
}

pub(super) fn assert_envelope(r: &serde_json::Value, status: &str) {
    assert_eq!(r["status"], status);
    assert_eq!(r["version"], SUBSONIC_API_VERSION);
    assert_eq!(r["type"], "koan");
    assert_eq!(r["serverVersion"], env!("CARGO_PKG_VERSION"));
    assert_eq!(r["openSubsonic"], true);
}

/// The router on a real socket, so requests go through hyper's own
/// parsing, URI limits included.
pub(super) async fn serve(state: Arc<AppState>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, build_test_router(state)).await });
    format!("http://{addr}")
}

pub(super) async fn post_text(http: &reqwest::Client, url: &str, form: &str) -> String {
    http.post(url)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(form.to_owned())
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap()
}

pub(super) const MATE: &str = "u=mate&p=hunter22&v=1.16.1&c=test&f=json";

pub(super) fn json(body: &str) -> serde_json::Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("{e}: {body}"))
}
