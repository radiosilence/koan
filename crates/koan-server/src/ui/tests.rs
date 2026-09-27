use std::sync::Arc;

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header};
use koan_core::auth::{self, Role};
use koan_core::db::connection::Database;
use koan_core::db::queries::{self, TrackMeta};
use tower::ServiceExt;

use crate::auth::routes::{AuthRouteState, LoginRateLimiter};

const HOST: &str = "koan.test";
const ORIGIN: &str = "https://koan.test";

fn meta(path: &std::path::Path, title: &str, n: i32) -> TrackMeta {
    TrackMeta {
        title: title.into(),
        artist: "Rrose".into(),
        album: "Hymn <to> Moisture".into(),
        album_artist: Some("Rrose".into()),
        track_number: Some(n),
        disc: Some(1),
        duration_ms: Some(125_000),
        codec: Some("FLAC".into()),
        sample_rate: Some(44100),
        bit_depth: Some(16),
        channels: Some(2),
        bitrate: Some(1000),
        genre: None,
        path: Some(path.to_string_lossy().into_owned()),
        date: Some("2019".into()),
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

struct Fixture {
    dir: tempfile::TempDir,
    app: axum::Router,
    state: AuthRouteState,
    album_id: i64,
    track_id: i64,
}

/// A library of one album, a user `alice` with password `hunter2`, and the UI
/// with auth on or off.
fn setup(auth_enabled: bool) -> Fixture {
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
        login_limiter: Arc::new(LoginRateLimiter::default()),
    };
    Fixture {
        app: super::router(
            pool,
            state.clone(),
            auth_enabled,
            Arc::new(crate::covers::Covers::new(dir.path().join("covers"))),
        ),
        dir,
        state,
        album_id,
        track_id,
    }
}

struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: String,
}

impl Reply {
    fn location(&self) -> &str {
        self.headers[header::LOCATION].to_str().unwrap()
    }

    fn cookies(&self) -> Vec<&str> {
        self.headers
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|v| v.to_str().unwrap())
            .collect()
    }

    /// The value of a cookie this reply set.
    fn cookie(&self, name: &str) -> String {
        self.cookies()
            .iter()
            .find_map(|c| c.strip_prefix(&format!("{name}=")))
            .and_then(|c| c.split(';').next())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| panic!("no {name} cookie in {:?}", self.cookies()))
            .to_owned()
    }
}

async fn send(app: &axum::Router, req: Request<Body>) -> Reply {
    let resp = app.clone().oneshot(req).await.unwrap();
    let (parts, body) = resp.into_parts();
    let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    Reply {
        status: parts.status,
        headers: parts.headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    }
}

fn get(uri: &str) -> axum::http::request::Builder {
    Request::get(uri).header(header::HOST, HOST)
}

fn form(uri: &str, body: &str) -> Request<Body> {
    Request::post(uri)
        .header(header::HOST, HOST)
        .header(header::ORIGIN, ORIGIN)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body.to_owned()))
        .unwrap()
}

fn access_token(state: &AuthRouteState) -> String {
    auth::mint_access_token(&state.private_pem, 1, "alice", Role::User, 900).unwrap()
}

fn authed(state: &AuthRouteState, uri: &str) -> axum::http::request::Builder {
    get(uri).header(
        header::COOKIE,
        format!("koan_access={}", access_token(state)),
    )
}

#[tokio::test]
async fn page_loads_without_a_session_resume_it_and_script_requests_are_refused() {
    let f = setup(true);
    let r = send(&f.app, get("/album/1?x=y").body(Body::empty()).unwrap()).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(r.location(), "/auth/resume?next=%2Falbum%2F1%3Fx%3Dy");

    for req in [
        get("/albums").header("x-koan-partial", "1"),
        get("/search/results").header("datastar-request", "true"),
        get(&format!("/ui/stream/{}", f.track_id)),
        get("/albums").header(header::COOKIE, "koan_access=forged"),
    ] {
        let r = send(&f.app, req.body(Body::empty()).unwrap()).await;
        assert!(
            matches!(r.status, StatusCode::UNAUTHORIZED | StatusCode::SEE_OTHER),
            "{}",
            r.status
        );
        assert_ne!(r.status, StatusCode::OK);
    }
    let r = send(
        &f.app,
        get("/albums")
            .header("x-koan-partial", "1")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn signing_in_sets_http_only_cookies_and_goes_only_to_local_paths() {
    let f = setup(true);
    let r = send(
        &f.app,
        form("/login", "username=alice&password=hunter2&next=%2Fartists"),
    )
    .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(r.location(), "/artists");
    assert!(!r.body.contains(&r.cookie("koan_access")));
    let cookies = r.cookies();
    assert_eq!(cookies.len(), 3);
    for c in &cookies {
        assert!(c.contains("HttpOnly") && c.contains("SameSite=Lax") && c.contains("Secure"));
    }
    assert!(
        cookies
            .iter()
            .any(|c| c.starts_with("koan_access=") && c.contains("Path=/;"))
    );
    assert!(
        cookies
            .iter()
            .any(|c| c.starts_with("koan_refresh=") && c.contains("Path=/auth;"))
    );

    for next in [
        "%2F%2Fevil.example",
        "https%3A%2F%2Fevil.example",
        "%2F%5Cevil.example",
        "%2Fauth%2Fresume",
    ] {
        let r = send(
            &f.app,
            form(
                "/login",
                &format!("username=alice&password=hunter2&next={next}"),
            ),
        )
        .await;
        assert_eq!(
            (r.status, r.location()),
            (StatusCode::SEE_OTHER, "/"),
            "{next}"
        );
    }
}

#[tokio::test]
async fn wrong_passwords_and_cross_site_forms_do_not_sign_in() {
    let f = setup(true);
    let r = send(
        &f.app,
        form("/login", "username=alice&password=nope&next=%2F"),
    )
    .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert!(r.body.contains("Wrong username or password."));
    assert!(r.cookies().is_empty());

    let req = Request::post("/login")
        .header(header::HOST, HOST)
        .header(header::ORIGIN, "https://evil.example")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from("username=alice&password=hunter2"))
        .unwrap();
    let r = send(&f.app, req).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert!(r.cookies().is_empty());
}

#[tokio::test]
async fn resume_spends_the_refresh_cookie_once() {
    let f = setup(true);
    let signed_in = send(&f.app, form("/login", "username=alice&password=hunter2")).await;
    let refresh = signed_in.cookie("koan_refresh");
    let resume = || {
        get("/auth/resume?next=%2Fqueue")
            .header(header::COOKIE, format!("koan_refresh={refresh}"))
            .body(Body::empty())
            .unwrap()
    };

    let r = send(&f.app, resume()).await;
    assert_eq!((r.status, r.location()), (StatusCode::SEE_OTHER, "/queue"));
    let access = r.cookie("koan_access");
    assert_ne!(r.cookie("koan_refresh"), refresh, "rotated");
    let page = send(
        &f.app,
        get("/queue")
            .header(header::COOKIE, format!("koan_access={access}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(page.status, StatusCode::OK);

    // Spent: the same cookie again goes to the sign-in form.
    let r = send(&f.app, resume()).await;
    assert_eq!(
        (r.status, r.location()),
        (StatusCode::SEE_OTHER, "/login?next=%2Fqueue")
    );
    let r = send(&f.app, get("/auth/resume").body(Body::empty()).unwrap()).await;
    assert_eq!(r.location(), "/login?next=%2F");
}

#[tokio::test]
async fn renew_answers_with_cookies_and_no_tokens() {
    let f = setup(true);
    let signed_in = send(&f.app, form("/login", "username=alice&password=hunter2")).await;
    let refresh = signed_in.cookie("koan_refresh");
    let renew = |origin: &str| {
        Request::post("/auth/renew")
            .header(header::HOST, HOST)
            .header(header::ORIGIN, origin)
            .header(header::COOKIE, format!("koan_refresh={refresh}"))
            .body(Body::empty())
            .unwrap()
    };
    assert_eq!(
        send(&f.app, renew("https://evil.example")).await.status,
        StatusCode::FORBIDDEN
    );
    let r = send(&f.app, renew(ORIGIN)).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert!(r.body.is_empty());
    assert!(!r.cookie("koan_access").is_empty());
}

#[tokio::test]
async fn signing_out_revokes_the_refresh_token() {
    let f = setup(true);
    let signed_in = send(&f.app, form("/login", "username=alice&password=hunter2")).await;
    let refresh = signed_in.cookie("koan_refresh");
    let mut req = form("/auth/signout", "");
    req.headers_mut().insert(
        header::COOKIE,
        format!("koan_refresh={refresh}").parse().unwrap(),
    );
    let r = send(&f.app, req).await;
    assert_eq!((r.status, r.location()), (StatusCode::SEE_OTHER, "/login"));
    assert!(r.cookies().iter().all(|c| c.contains("Max-Age=0")));

    let r = send(
        &f.app,
        get("/auth/resume")
            .header(header::COOKIE, format!("koan_refresh={refresh}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert!(r.location().starts_with("/login"));
}

#[tokio::test]
async fn streams_serve_ranges_to_a_signed_in_user() {
    let f = setup(true);
    let uri = format!("/ui/stream/{}", f.track_id);
    let r = send(
        &f.app,
        authed(&f.state, &uri)
            .header(header::RANGE, "bytes=2-4")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(
        (r.status, r.body.as_str()),
        (StatusCode::PARTIAL_CONTENT, "234")
    );
    let r = send(&f.app, get(&uri).body(Body::empty()).unwrap()).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let r = send(
        &f.app,
        authed(&f.state, "/ui/stream/999")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn pages_render_whole_or_as_content_escaped_and_without_inline_script() {
    let f = setup(true);
    let uri = format!("/album/{}", f.album_id);
    let full = send(&f.app, authed(&f.state, &uri).body(Body::empty()).unwrap()).await;
    assert_eq!(full.status, StatusCode::OK);
    assert!(full.body.starts_with("<!doctype html>"));
    assert!(full.body.contains("rel=icon"), "pages carry the favicon");
    assert!(full.body.contains("<nav class=side"));
    assert!(full.body.contains("Wet &lt;Moss&gt; &amp; Stone"));
    assert!(full.body.contains("Hymn &lt;to&gt; Moisture"));
    assert!(!full.body.contains("<Moss>") && !full.body.contains("<to>"));
    assert!(!full.body.contains("<script>") && !full.body.contains(" style="));
    let csp = full.headers[header::CONTENT_SECURITY_POLICY]
        .to_str()
        .unwrap();
    assert!(csp.contains("script-src 'self' 'unsafe-eval';") && !csp.contains("unsafe-inline"));

    let partial = send(
        &f.app,
        authed(&f.state, &uri)
            .header("x-koan-partial", "1")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert!(partial.body.starts_with("<section class=\"page album\""));
    assert!(
        partial
            .body
            .contains("data-title=\"Hymn &lt;to&gt; Moisture\"")
    );
    assert!(!partial.body.contains("<nav"));

    for page in [
        "/",
        "/albums",
        "/artists",
        "/search?q=moss",
        "/queue",
        "/artist/1",
    ] {
        let r = send(&f.app, authed(&f.state, page).body(Body::empty()).unwrap()).await;
        assert_eq!(r.status, StatusCode::OK, "{page}");
    }
    let r = send(
        &f.app,
        authed(&f.state, "/search?q=moss")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert!(r.body.contains("Wet &lt;Moss&gt; &amp; Stone"));
}

#[tokio::test]
async fn live_fragments_are_datastar_events_and_posts_need_datastar() {
    let f = setup(true);
    let r = send(
        &f.app,
        authed(
            &f.state,
            "/search/results?datastar=%7B%22q%22%3A%22moss%22%7D",
        )
        .header("datastar-request", "true")
        .body(Body::empty())
        .unwrap(),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.body.starts_with("event: datastar-patch-elements\n"));
    assert!(r.body.contains("data: elements <div id=results>"));

    let r = send(
        &f.app,
        authed(&f.state, "/albums/more?offset=0")
            .header("datastar-request", "true")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert!(
        r.body
            .contains("data: selector #albums\ndata: mode append\n")
    );

    let share = format!("/album/{}/share", f.album_id);
    let bare = Request::post(&share)
        .header(header::HOST, HOST)
        .header(
            header::COOKIE,
            format!("koan_access={}", access_token(&f.state)),
        )
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&f.app, bare).await.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn pages_link_versioned_covers_and_a_missing_cover_is_remembered() {
    let f = setup(true);
    let page = send(
        &f.app,
        authed(&f.state, "/albums").body(Body::empty()).unwrap(),
    )
    .await;
    let src = format!("src=\"/ui/cover/{}?size=400&amp;v=", f.album_id);
    let src_raw = format!("src=\"/ui/cover/{}?size=400&v=", f.album_id);
    assert!(
        page.body.contains(&src) || page.body.contains(&src_raw),
        "{}",
        page.body
    );
    assert!(
        page.body
            .contains("loading=lazy decoding=async width=400 height=400")
    );

    let uri = format!("/ui/cover/{}?size=300&v=1", f.album_id);
    for _ in 0..2 {
        let r = send(&f.app, authed(&f.state, &uri).body(Body::empty()).unwrap()).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "the fake file has no art");
    }
    let kept: Vec<_> = std::fs::read_dir(f.dir.path().join("covers"))
        .unwrap()
        .map(|e| e.unwrap())
        .collect();
    assert_eq!(kept.len(), 1, "one remembered miss");
    assert_eq!(kept[0].metadata().unwrap().len(), 0);
    assert!(kept[0].file_name().to_string_lossy().ends_with("-400.jpg"));
}

#[tokio::test]
async fn sorting_and_filtering_live_in_the_query_string() {
    let f = setup(true);
    let page = |uri: &str| {
        let req = authed(&f.state, uri).body(Body::empty()).unwrap();
        let app = f.app.clone();
        async move { send(&app, req).await }
    };
    let r = page("/albums?sort=title&lossless=1&genre=").await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(
        r.body.contains("Hymn &lt;to&gt; Moisture"),
        "FLAC is lossless"
    );
    assert!(r.body.contains("<option value=\"title\" selected>"));
    assert!(r.body.contains("name=lossless value=1 checked"));
    assert!(r.body.contains("<span class=badge>1</span>"));

    let r = page("/albums?codec=MP3").await;
    assert!(r.body.contains("No albums match."));
    let r = page("/albums?from=2020").await;
    assert!(r.body.contains("No albums match."), "released 2019");
    let r = page("/albums?from=2015&to=2019").await;
    assert!(r.body.contains("Hymn &lt;to&gt; Moisture"));

    // A value from a link, not among those offered, is still shown and kept.
    let r = page("/albums?genre=%22%3E%3Cscript%3E").await;
    assert!(
        r.body
            .contains("<option value=\"&quot;&gt;&lt;script&gt;\" selected>")
    );
    assert!(!r.body.contains("\"><script>"));

    for uri in [
        "/artists?sort=albums&fav=1",
        "/artists?sort=recent&lossless=1",
        "/albums?sort=random",
    ] {
        assert_eq!(page(uri).await.status, StatusCode::OK, "{uri}");
    }
    let r = page("/albums?sort=random").await;
    assert!(
        r.body.contains("<input type=hidden name=seed value="),
        "the shuffle is pinned"
    );
}

#[tokio::test]
async fn with_auth_off_everything_is_open() {
    let f = setup(false);
    let r = send(&f.app, get("/albums").body(Body::empty()).unwrap()).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(!r.body.contains("Sign out"));
    assert!(
        r.body
            .contains(concat!("kōan ", env!("CARGO_PKG_VERSION"), "</a>")),
        "the version shows without an account too"
    );
    let r = send(
        &f.app,
        get("/login?next=%2Fqueue").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(r.location(), "/queue");
}

#[tokio::test]
async fn api_keys_are_shown_once_listed_and_revoked() {
    let f = setup(true);
    let post = |uri: &str, body: &str, datastar: bool| {
        let mut req = Request::post(uri)
            .header(header::HOST, HOST)
            .header(
                header::COOKIE,
                format!("koan_access={}", access_token(&f.state)),
            )
            .header(header::CONTENT_TYPE, "application/json");
        if datastar {
            req = req.header("datastar-request", "true");
        }
        req.body(Body::from(body.to_owned())).unwrap()
    };

    let r = send(
        &f.app,
        authed(&f.state, "/keys").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.body.contains("No keys yet."), "{}", r.body);

    let r = send(&f.app, post("/keys", r#"{"keyname":"phone"}"#, false)).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);

    let r = send(&f.app, post("/keys", r#"{"keyname":"phone"}"#, true)).await;
    let key = r
        .body
        .split("id=new-key readonly value=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap_or_else(|| panic!("no key in {}", r.body))
        .to_owned();
    let db = Database::open(f.state.pool.path()).unwrap();
    let user = queries::api_keys::authenticate_api_key(&db.conn, &key)
        .unwrap()
        .unwrap();
    assert_eq!(user.username, "alice");

    let r = send(
        &f.app,
        authed(&f.state, "/keys").body(Body::empty()).unwrap(),
    )
    .await;
    assert!(
        r.body.contains("phone") && r.body.contains("last used"),
        "{}",
        r.body
    );
    assert!(!r.body.contains(&key));

    let id = queries::api_keys::list_api_keys(&db.conn, Some(user.id)).unwrap()[0].id;
    let r = send(&f.app, post(&format!("/keys/{id}/revoke"), "{}", true)).await;
    assert!(r.body.contains("No keys yet."), "{}", r.body);
    assert!(
        queries::api_keys::authenticate_api_key(&db.conn, &key)
            .unwrap()
            .is_none()
    );
}
