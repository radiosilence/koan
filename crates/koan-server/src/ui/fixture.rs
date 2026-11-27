//! A throwaway library, accounts and the UI's router over them, for the
//! pages' tests and the escaping guard. Each page's tests sit beside its
//! module and take what they need from here.

#![allow(dead_code, unused_imports)]

pub(in crate::ui) use std::sync::Arc;

pub(in crate::ui) use axum::body::Body;
pub(in crate::ui) use axum::http::{HeaderMap, Request, StatusCode, header};
pub(in crate::ui) use koan_core::auth::{self, Role};
pub(in crate::ui) use koan_core::db::connection::Database;
pub(in crate::ui) use koan_core::db::queries::{self, TrackMeta};
pub(in crate::ui) use tower::ServiceExt;

pub(in crate::ui) use crate::auth::routes::{AuthRouteState, RateLimiter};

pub(in crate::ui) const HOST: &str = "koan.test";

pub(in crate::ui) const ORIGIN: &str = "https://koan.test";

pub(in crate::ui) fn meta(path: &std::path::Path, title: &str, n: i32) -> TrackMeta {
    TrackMeta {
        title: title.into(),
        artist: "Rrose".into(),
        album: "Hymn <to> Moisture".into(),
        album_artist: Some("Rrose".into()),
        track_number: Some(n),
        disc: Some(1),
        disc_total: None,
        duration_ms: Some(125_000),
        codec: Some("FLAC".into()),
        sample_rate: Some(44100),
        bit_depth: Some(16),
        channels: Some(2),
        bitrate: Some(1000),
        genre: None,
        path: Some(path.to_string_lossy().into_owned()),
        server_path: None,
        date: Some("2019".into()),
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

pub(in crate::ui) struct Fixture {
    pub(in crate::ui) dir: tempfile::TempDir,
    pub(in crate::ui) app: axum::Router,
    pub(in crate::ui) state: AuthRouteState,
    pub(in crate::ui) jobs: crate::graphql::jobs::JobRegistry,
    pub(in crate::ui) album_id: i64,
    pub(in crate::ui) track_id: i64,
}

/// A library of one album, a user `alice` with password `hunter2`, and the UI
/// with auth on or off.
pub(in crate::ui) fn setup(auth_enabled: bool) -> Fixture {
    setup_at(auth_enabled, None)
}

/// `setup`, with `server.url` set when OAuth needs one.
pub(in crate::ui) fn setup_at(auth_enabled: bool, server_url: Option<&str>) -> Fixture {
    setup_full(auth_enabled, server_url, None)
}

pub(in crate::ui) fn setup_full(
    auth_enabled: bool,
    server_url: Option<&str>,
    proxy_auth: Option<crate::ui::ProxyAuth>,
) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("koan.db");
    let db = Database::open(&db_path).unwrap();
    let file = dir.path().join("a.flac");
    std::fs::write(&file, b"0123456789").unwrap();
    let track_id = queries::upsert_track(&db.conn, &meta(&file, "Wet <Moss> & Stone", 1)).unwrap();
    let album_id = queries::tracks_by_ids(&db.conn, &[track_id]).unwrap()[0]
        .album_id
        .unwrap();
    queries::auth::create_user(&db.conn, "alice", "hunter2", Role::User).unwrap();
    let (private_pem, public_pem) = auth::generate_keypair_pem().unwrap();
    let pool = Arc::new(koan_core::db::pool::Pool::new(db_path));
    let state = AuthRouteState {
        pool: pool.clone(),
        private_pem: Arc::new(private_pem.into_bytes()),
        public_pem: Arc::new(public_pem.into_bytes()),
        access_ttl_secs: 900,
        refresh_ttl_secs: 3600,
        cookie_secure: true,
        login_limiter: Arc::new(RateLimiter::default()),
        users: Arc::new(crate::auth::password::PasswordVerifier::new(pool.clone())),
    };
    let jobs = crate::graphql::jobs::JobRegistry::default();
    Fixture {
        app: crate::ui::router(
            pool,
            state.clone(),
            auth_enabled,
            auth_enabled.then(crate::ui::SetupGate::default),
            Arc::new(crate::covers::Covers::new(dir.path().join("covers"))),
            server_url.map(str::to_owned),
            Vec::new(),
            proxy_auth,
            jobs.clone(),
        ),
        dir,
        state,
        jobs,
        album_id,
        track_id,
    }
}

pub(in crate::ui) struct Reply {
    pub(in crate::ui) status: StatusCode,
    pub(in crate::ui) headers: HeaderMap,
    pub(in crate::ui) body: String,
}

impl Reply {
    pub(in crate::ui) fn location(&self) -> &str {
        self.headers[header::LOCATION].to_str().unwrap()
    }

    pub(in crate::ui) fn cookies(&self) -> Vec<&str> {
        self.headers
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|v| v.to_str().unwrap())
            .collect()
    }

    /// The value of a cookie this reply set.
    pub(in crate::ui) fn cookie(&self, name: &str) -> String {
        self.cookies()
            .iter()
            .find_map(|c| c.strip_prefix(&format!("{name}=")))
            .and_then(|c| c.split(';').next())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| panic!("no {name} cookie in {:?}", self.cookies()))
            .to_owned()
    }
}

pub(in crate::ui) async fn send(app: &axum::Router, req: Request<Body>) -> Reply {
    let resp = app.clone().oneshot(req).await.unwrap();
    let (parts, body) = resp.into_parts();
    let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    Reply {
        status: parts.status,
        headers: parts.headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    }
}

pub(in crate::ui) fn get(uri: &str) -> axum::http::request::Builder {
    Request::get(uri).header(header::HOST, HOST)
}

pub(in crate::ui) fn form(uri: &str, body: &str) -> Request<Body> {
    Request::post(uri)
        .header(header::HOST, HOST)
        .header(header::ORIGIN, ORIGIN)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body.to_owned()))
        .unwrap()
}

pub(in crate::ui) fn access_token(state: &AuthRouteState) -> String {
    auth::mint_access_token(&state.private_pem, 1, "alice", Role::User, 900).unwrap()
}

pub(in crate::ui) fn authed(state: &AuthRouteState, uri: &str) -> axum::http::request::Builder {
    get(uri).header(
        header::COOKIE,
        format!("koan_access={}", access_token(state)),
    )
}

/// The `data-id`s of the track rows on a page, in order.
pub(in crate::ui) fn row_ids(body: &str) -> Vec<String> {
    body.split("<li tabindex=\"0\" data-id=\"")
        .skip(1)
        .map(|r| r.split(' ').next().unwrap().to_owned())
        .collect()
}

/// An open koanLink for `username`, as the link route registers one.
pub(in crate::ui) fn open_link(username: &str) -> tokio::sync::mpsc::Receiver<koan_core::remote::acks::Envelope> {
    let (tx, rx) = crate::clients::LinkTx::channel();
    crate::clients::registry().register(username, "phone", "ios", "ui-tests", tx, false, false);
    rx
}

/// The link's session would see its channel close, which is what ends it.
pub(in crate::ui) fn assert_closed(mut rx: tokio::sync::mpsc::Receiver<koan_core::remote::acks::Envelope>) {
    while rx.try_recv().is_ok() {}
    assert!(matches!(
        rx.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Disconnected)
    ));
}

/// Behind an authenticating proxy at 10.0.0.1 that names the account in
/// `Remote-User`.
pub(in crate::ui) fn setup_behind_proxy() -> Fixture {
    let proxy = crate::ui::ProxyAuth::from_config("Remote-User", &["10.0.0.1".into()]).unwrap();
    setup_full(true, None, proxy)
}

pub(in crate::ui) fn from_peer(builder: axum::http::request::Builder, peer: &str) -> axum::http::request::Builder {
    let addr: std::net::SocketAddr = format!("{peer}:50000").parse().unwrap();
    builder.extension(axum::extract::ConnectInfo(addr))
}

/// The UI over a server with no accounts, its setup page on or off, and the
/// setup code the server logged when on.
pub(in crate::ui) fn unset(setup_wizard: bool) -> (tempfile::TempDir, axum::Router, Option<String>) {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("koan.db");
    Database::open(&db_path).unwrap();
    let (private_pem, public_pem) = auth::generate_keypair_pem().unwrap();
    let pool = Arc::new(koan_core::db::pool::Pool::new(db_path));
    let state = AuthRouteState {
        pool: pool.clone(),
        private_pem: Arc::new(private_pem.into_bytes()),
        public_pem: Arc::new(public_pem.into_bytes()),
        access_ttl_secs: 900,
        refresh_ttl_secs: 3600,
        cookie_secure: true,
        login_limiter: Arc::new(RateLimiter::default()),
        users: Arc::new(crate::auth::password::PasswordVerifier::new(pool.clone())),
    };
    let setup = setup_wizard.then(crate::ui::SetupGate::default);
    let code = setup
        .as_ref()
        .and_then(crate::ui::SetupGate::code)
        .map(|c| c.as_str().to_owned());
    let app = crate::ui::router(
        pool,
        state,
        true,
        setup,
        Arc::new(crate::covers::Covers::new(dir.path().join("covers"))),
        None,
        Vec::new(),
        None,
        Default::default(),
    );
    (dir, app, code)
}

/// Last.fm, as far as turning a token into a session goes: every
/// `auth.getSession` is answered with the session of `fmuser`.
pub(in crate::ui) fn fake_lastfm() {
    use std::io::{BufRead, BufReader, Read, Write};
    static STARTED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    STARTED.get_or_init(|| {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        koan_core::scrobbling::lastfm::use_api(&format!(
            "http://{}/2.0/",
            listener.local_addr().unwrap()
        ));
        koan_core::scrobbling::lastfm::use_credentials("test-key", "test-secret");
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut length = 0;
                let mut line = String::new();
                while reader.read_line(&mut line).is_ok_and(|n| n > 0) && line != "\r\n" {
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap_or(0);
                    }
                    line.clear();
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let body = String::from_utf8(body).unwrap();
                let answer = if body.contains("token=good-token") {
                    r#"{"session":{"name":"fmuser","key":"session-key-9","subscriber":0}}"#
                } else {
                    r#"{"error":4,"message":"Unauthorized Token"}"#
                };
                let _ = write!(
                    &stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
                    answer.len()
                );
            }
        });
    });
}
