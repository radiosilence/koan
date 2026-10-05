use std::sync::Arc;

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header};
use koan_core::auth::{self, Role};
use koan_core::db::connection::Database;
use koan_core::db::queries::{self, TrackMeta};
use tower::ServiceExt;

use crate::auth::routes::{AuthRouteState, RateLimiter};

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
    setup_at(auth_enabled, None)
}

/// `setup`, with `sharing.public_url` set when OAuth needs one.
fn setup_at(auth_enabled: bool, public_url: Option<&str>) -> Fixture {
    setup_full(auth_enabled, public_url, None)
}

fn setup_full(
    auth_enabled: bool,
    public_url: Option<&str>,
    proxy_auth: Option<super::ProxyAuth>,
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
    };
    Fixture {
        app: super::router(
            pool,
            state.clone(),
            auth_enabled,
            Arc::new(crate::covers::Covers::new(dir.path().join("covers"))),
            public_url.map(str::to_owned),
            Vec::new(),
            proxy_auth,
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
async fn every_class_on_every_page_has_a_rule() {
    let f = setup(true);
    let db = Database::open(f.state.pool.path()).unwrap();
    let boss = queries::auth::create_user(&db.conn, "boss", "sesame", Role::Admin).unwrap();
    let artist = queries::tracks_by_ids(&db.conn, &[f.track_id]).unwrap()[0]
        .artist_id
        .unwrap();
    let access =
        auth::mint_access_token(&f.state.private_pem, boss, "boss", Role::Admin, 900).unwrap();
    let page = |uri: &str| {
        get(uri)
            .header(header::COOKIE, format!("koan_access={access}"))
            .body(Body::empty())
            .unwrap()
    };
    let post = |uri: &str, body: &str| {
        Request::post(uri)
            .header(header::HOST, HOST)
            .header(header::COOKIE, format!("koan_access={access}"))
            .header(header::CONTENT_TYPE, "application/json")
            .header("datastar-request", "true")
            .body(Body::from(body.to_owned()))
            .unwrap()
    };
    let mut rendered = Vec::new();
    for uri in [
        "/albums".to_owned(),
        "/albums?lossless=1&codec=MP3".to_owned(),
        format!("/album/{}", f.album_id),
        "/artists".to_owned(),
        format!("/artist/{artist}"),
        "/playlists".to_owned(),
        "/search?q=Wet".to_owned(),
        "/search?q=nothing-here".to_owned(),
        "/queue".to_owned(),
        "/keys".to_owned(),
        "/users".to_owned(),
        "/connect".to_owned(),
    ] {
        let r = send(&f.app, page(&uri)).await;
        assert_eq!(r.status, StatusCode::OK, "{uri}");
        rendered.push((uri, r.body));
    }
    for (uri, body) in [
        ("/keys", r#"{"keyname":"phone"}"#),
        ("/users/1/password/form", "{}"),
    ] {
        let r = send(&f.app, post(uri, body)).await;
        assert_eq!(r.status, StatusCode::OK, "{uri}");
        rendered.push((uri.to_owned(), r.body));
    }
    let r = send(&f.app, get("/login").body(Body::empty()).unwrap()).await;
    rendered.push(("/login".into(), r.body));
    for (uri, html) in rendered {
        let missing = crate::share::unstyled_classes(
            &html,
            super::UI_CSS,
            &["page", "browse", "toolbar", "search", "group"],
        );
        assert!(
            missing.is_empty(),
            "{uri}: no rule in ui.css for {missing:?}"
        );
    }
}

#[tokio::test]
async fn pages_render_whole_or_as_content_escaped_and_without_inline_script() {
    let f = setup(true);
    let uri = format!("/album/{}", f.album_id);
    let full = send(&f.app, authed(&f.state, &uri).body(Body::empty()).unwrap()).await;
    assert_eq!(full.status, StatusCode::OK);
    assert!(full.body.starts_with("<!doctype html>"));
    assert!(full.body.contains("rel=icon"), "pages carry the favicon");
    assert!(full.body.contains("aria-label=Library>"));
    assert!(full.body.contains("Wet &lt;Moss&gt; &amp; Stone"));
    assert!(full.body.contains("Hymn &lt;to&gt; Moisture"));
    assert!(!full.body.contains("<Moss>") && !full.body.contains("<to>"));
    assert!(!full.body.contains("<script>") && !full.body.contains(" style="));
    let csp = full.headers[header::CONTENT_SECURITY_POLICY]
        .to_str()
        .unwrap();
    assert!(csp.contains("script-src 'self' 'unsafe-eval';") && !csp.contains("unsafe-inline"));
    // The stylesheet's URL changes with its contents, so an upgrade never pairs
    // new markup with a cached stylesheet.
    let css = &super::ASSETS.css;
    assert!(css.starts_with("/ui/assets/ui.css?v=") && full.body.contains(css.as_str()));
    let served = send(&f.app, get(css).body(Body::empty()).unwrap()).await;
    assert_eq!(served.status, StatusCode::OK);
    assert!(
        served.headers[header::CACHE_CONTROL]
            .to_str()
            .unwrap()
            .contains("immutable")
    );
    // Every page and patch names it too, so an open tab can catch up.
    assert_eq!(full.headers["x-koan-css"], super::ASSETS.css_hash.as_str());

    let partial = send(
        &f.app,
        authed(&f.state, &uri)
            .header("x-koan-partial", "1")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert!(partial.body.starts_with("<section class=\"page\" "));
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
async fn pages_link_versioned_covers_and_a_missing_cover_is_remembered_and_drawn() {
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
        assert_eq!(r.status, StatusCode::OK, "the fake file has no art");
        assert_eq!(r.headers[header::CONTENT_TYPE], "image/svg+xml");
        assert!(r.body.contains("<svg"), "the apps' placeholder, drawn");
        assert!(
            !r.headers[header::CACHE_CONTROL]
                .to_str()
                .unwrap()
                .contains("immutable"),
            "art added later must replace it"
        );
    }
    let r = send(
        &f.app,
        authed(&f.state, "/ui/cover/999999?size=300")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND, "no such album");
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
    assert!(r.body.contains(">1</span></summary>"));

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
async fn playlists_list_the_callers_own_and_play_like_albums() {
    let f = setup(true);
    let (mine, theirs) = {
        let db = Database::open(&f.dir.path().join("koan.db")).unwrap();
        let bob = queries::auth::create_user(&db.conn, "bob", "hunter3", Role::User).unwrap();
        let mine = queries::create_playlist(&db.conn, 1, "Damp <Mix>", None).unwrap();
        queries::add_tracks(&db.conn, mine, &[f.track_id]).unwrap();
        let theirs = queries::create_playlist(&db.conn, bob, "Bob's Secret", None).unwrap();
        (mine, theirs)
    };
    let r = send(
        &f.app,
        authed(&f.state, "/playlists").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.body.contains("Damp &lt;Mix&gt;") && r.body.contains("1 track"));
    assert!(
        !r.body.contains("Bob"),
        "another account's private playlist is not listed"
    );

    let uri = format!("/playlist/{mine}");
    let r = send(&f.app, authed(&f.state, &uri).body(Body::empty()).unwrap()).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.body.contains("data-act=queue") && r.body.contains("data-context=album"));
    assert!(r.body.contains(&format!("data-id={}", f.track_id)));

    let uri = format!("/playlist/{theirs}");
    let r = send(&f.app, authed(&f.state, &uri).body(Body::empty()).unwrap()).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
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

#[tokio::test]
async fn admins_create_invite_and_remove_accounts() {
    static CONFIG: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    koan_core::config::set_config_dir(CONFIG.get_or_init(|| tempfile::tempdir().unwrap()).path());
    let f = setup(true);
    let db = Database::open(f.state.pool.path()).unwrap();
    let boss = queries::auth::create_user(&db.conn, "boss", "sesame", Role::Admin).unwrap();
    let token = |id: i64, name: &str, role: Role| {
        auth::mint_access_token(&f.state.private_pem, id, name, role, 900).unwrap()
    };
    let admin = token(boss, "boss", Role::Admin);
    let post = |uri: &str, body: &str, access: &str| {
        Request::post(uri)
            .header(header::HOST, HOST)
            .header(header::COOKIE, format!("koan_access={access}"))
            .header(header::CONTENT_TYPE, "application/json")
            .header("datastar-request", "true")
            .body(Body::from(body.to_owned()))
            .unwrap()
    };
    let page = |access: &str| {
        get("/users")
            .header(header::COOKIE, format!("koan_access={access}"))
            .body(Body::empty())
            .unwrap()
    };

    let r = send(&f.app, page(&access_token(&f.state))).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = send(
        &f.app,
        post("/users", r#"{"newuser":"x"}"#, &access_token(&f.state)),
    )
    .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    // A token minted while alice was an admin: the account's role now governs.
    let r = send(&f.app, page(&token(1, "alice", Role::Admin))).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);

    let r = send(&f.app, page(&admin)).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(
        r.body.contains("alice") && r.body.contains("boss"),
        "{}",
        r.body
    );

    let r = send(
        &f.app,
        post(
            "/users",
            r#"{"newuser":"sarita","newrole":"readonly"}"#,
            &admin,
        ),
    )
    .await;
    let link = r
        .body
        .split("id=invite-link readonly value=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap_or_else(|| panic!("no link in {}", r.body))
        .replace("&amp;", "&");
    let invite = koan_core::invite::Invite::parse(&link).unwrap();
    assert_eq!(invite.server, format!("http://{HOST}"));
    assert_eq!(invite.username, "sarita");
    assert_eq!(invite.password, None);
    let password = r
        .body
        .split("<dt>Password</dt><dd><code>")
        .nth(1)
        .and_then(|rest| rest.split('<').next())
        .unwrap_or_else(|| panic!("no password in {}", r.body))
        .to_owned();
    let row = queries::auth::get_user_by_username(&db.conn, "sarita")
        .unwrap()
        .unwrap();
    assert_eq!(row.role, Role::Readonly);
    auth::verify_password(&password, &row.password_hash).unwrap();

    // The link's token is signed with this server's key and redeems for a key.
    let joined = koan_core::invite::redeem(
        &db.conn,
        &crate::auth::signing_keys().unwrap().1,
        invite.token.as_deref().unwrap(),
        "phone",
    )
    .unwrap();
    assert_eq!(joined.username, "sarita");

    // Inviting again shows no password: the server cannot read one back.
    let r = send(
        &f.app,
        post(&format!("/users/{}/invite", row.id), "{}", &admin),
    )
    .await;
    assert!(r.body.contains("id=invite-link"), "{}", r.body);
    assert!(!r.body.contains("<dt>Password</dt>"), "{}", r.body);

    // A reset does show one, and signs the account's devices out.
    let alice_link = open_link("alice");
    let r = send(&f.app, post("/users/1/invite?reset=true", "{}", &admin)).await;
    assert!(r.body.contains("<dt>Password</dt>"), "{}", r.body);
    assert_closed(alice_link);

    // So does a password the admin chooses. The row's button asks for the
    // form, and a value typed into another account's form and never
    // submitted, which Datastar posts along with it, changes nothing.
    let before = queries::auth::get_user_by_id(&db.conn, 1)
        .unwrap()
        .unwrap()
        .password_hash;
    let r = send(
        &f.app,
        post(
            "/users/1/password/form",
            r#"{"setpassword":"typed for someone else"}"#,
            &admin,
        ),
    )
    .await;
    assert!(r.body.contains("data-bind:setpassword"), "{}", r.body);
    assert!(r.body.contains(r#"{"setpassword":""}"#), "{}", r.body);
    let after = queries::auth::get_user_by_id(&db.conn, 1)
        .unwrap()
        .unwrap()
        .password_hash;
    assert_eq!(before, after);
    let r = send(
        &f.app,
        post("/users/1/password", r#"{"setpassword":"short"}"#, &admin),
    )
    .await;
    assert!(r.body.contains("at least 8"), "{}", r.body);
    let r = send(
        &f.app,
        post(
            "/users/1/password",
            r#"{"setpassword":"correct horse"}"#,
            &admin,
        ),
    )
    .await;
    assert!(r.body.contains("password is changed"), "{}", r.body);
    let alice = queries::auth::get_user_by_id(&db.conn, 1).unwrap().unwrap();
    auth::verify_password("correct horse", &alice.password_hash).unwrap();
    assert!(
        queries::api_keys::authenticate_api_key(&db.conn, &joined.api_key)
            .unwrap()
            .is_some()
    );

    let r = send(
        &f.app,
        post(&format!("/users/{}/role?role=user", row.id), "{}", &admin),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    let r = send(
        &f.app,
        post(&format!("/users/{boss}/role?role=user",), "{}", &admin),
    )
    .await;
    assert!(r.body.contains("last admin"), "{}", r.body);
    assert_eq!(
        queries::auth::get_user_by_username(&db.conn, "sarita")
            .unwrap()
            .unwrap()
            .role,
        Role::User
    );

    let r = send(&f.app, post(&format!("/users/{boss}/delete"), "{}", &admin)).await;
    assert!(r.body.contains("own account"), "{}", r.body);
    let sarita_link = open_link("sarita");
    send(
        &f.app,
        post(&format!("/users/{}/delete", row.id), "{}", &admin),
    )
    .await;
    assert!(
        queries::auth::get_user_by_username(&db.conn, "sarita")
            .unwrap()
            .is_none()
    );
    assert_closed(sarita_link);
}

/// An open koanLink for `username`, as the link route registers one.
fn open_link(
    username: &str,
) -> tokio::sync::mpsc::UnboundedReceiver<koan_core::remote::link::LinkCommand> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    crate::clients::registry().register(username, "phone", "ios", "ui-tests", tx, false);
    rx
}

/// The link's session would see its channel close, which is what ends it.
fn assert_closed(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<koan_core::remote::link::LinkCommand>,
) {
    while rx.try_recv().is_ok() {}
    assert!(matches!(
        rx.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Disconnected)
    ));
}

mod oauth {
    use super::*;
    use base64::Engine as _;
    use sha2::{Digest, Sha256};

    const CALLBACK: &str = "https://claude.ai/api/mcp/auth_callback";
    const VERIFIER: &str = "a-verifier-of-at-least-forty-three-characters-long";

    fn challenge() -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(VERIFIER))
    }

    fn query(pairs: &[(&str, &str)]) -> String {
        form_urlencoded::Serializer::new(String::new())
            .extend_pairs(pairs)
            .finish()
    }

    fn json(r: &Reply) -> serde_json::Value {
        serde_json::from_str(&r.body).unwrap_or_else(|_| panic!("not JSON: {}", r.body))
    }

    async fn register(f: &Fixture, redirect: &str) -> Reply {
        send(
            &f.app,
            Request::post("/oauth/register")
                .header(header::HOST, HOST)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({ "redirect_uris": [redirect], "client_name": "Claude" })
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
    }

    fn authorize_params<'a>(client_id: &'a str, challenge: &'a str) -> Vec<(&'a str, &'a str)> {
        vec![
            ("response_type", "code"),
            ("client_id", client_id),
            ("redirect_uri", CALLBACK),
            ("code_challenge", challenge),
            ("code_challenge_method", "S256"),
            ("state", "xyz"),
            ("resource", "https://koan.test/mcp"),
        ]
    }

    /// Approve as alice; the reply is the redirect to the client.
    async fn approve(f: &Fixture, params: &[(&str, &str)], origin: &str) -> Reply {
        let mut params = params.to_vec();
        params.push(("decision", "allow"));
        send(
            &f.app,
            Request::post("/oauth/authorize")
                .header(header::HOST, HOST)
                .header(header::ORIGIN, origin)
                .header(
                    header::COOKIE,
                    format!("koan_access={}", access_token(&f.state)),
                )
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(query(&params)))
                .unwrap(),
        )
        .await
    }

    fn code_of(location: &str) -> String {
        let url = reqwest::Url::parse(location).unwrap();
        let get = |k: &str| {
            url.query_pairs()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v.into_owned())
        };
        assert_eq!(get("state").as_deref(), Some("xyz"));
        assert_eq!(get("iss").as_deref(), Some(ORIGIN));
        get("code").expect("a code")
    }

    async fn token(f: &Fixture, pairs: &[(&str, &str)]) -> Reply {
        let mut req = form("/oauth/token", &query(pairs));
        req.headers_mut().remove(header::ORIGIN);
        send(&f.app, req).await
    }

    #[tokio::test]
    async fn an_mcp_client_signs_in_and_refreshes() {
        let f = setup_at(true, Some("https://koan.test/"));
        let meta = send(
            &f.app,
            get("/.well-known/oauth-authorization-server")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(json(&meta)["issuer"], ORIGIN);
        let resource = send(
            &f.app,
            get(crate::ui::RESOURCE_METADATA)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(json(&resource)["resource"], "https://koan.test/mcp");

        let reg = register(&f, CALLBACK).await;
        assert_eq!(reg.status, StatusCode::CREATED);
        let client_id = json(&reg)["client_id"].as_str().unwrap().to_owned();
        let challenge = challenge();
        let params = authorize_params(&client_id, &challenge);
        let uri = format!("/oauth/authorize?{}", query(&params));

        // Signed out: through sign-in and back.
        let r = send(&f.app, get(&uri).body(Body::empty()).unwrap()).await;
        assert_eq!(r.status, StatusCode::SEE_OTHER);
        assert!(
            r.location()
                .starts_with("/auth/resume?next=%2Foauth%2Fauthorize")
        );

        let r = send(&f.app, authed(&f.state, &uri).body(Body::empty()).unwrap()).await;
        assert_eq!(r.status, StatusCode::OK);
        assert!(r.body.contains("Connect kōan to claude.ai?"), "{}", r.body);
        let csp = r.headers[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
        assert!(csp.contains("form-action 'self' https://claude.ai"));
        assert!(csp.contains("frame-ancestors 'none'"));

        let r = approve(&f, &params, ORIGIN).await;
        assert_eq!(r.status, StatusCode::SEE_OTHER);
        assert!(r.location().starts_with(CALLBACK));
        let code = code_of(r.location());

        let exchange = [
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", CALLBACK),
            ("client_id", client_id.as_str()),
            ("code_verifier", VERIFIER),
            ("resource", "https://koan.test/mcp"),
        ];
        let r = token(&f, &exchange).await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.body);
        let t = json(&r);
        // Good at /mcp and nowhere else.
        let access = t["access_token"].as_str().unwrap();
        let claims =
            auth::validate_scoped_token(&f.state.public_pem, access, Some(auth::MCP_SCOPE))
                .unwrap();
        assert_eq!(claims.username, "alice");
        assert!(auth::validate_access_token(&f.state.public_pem, access).is_err());
        let r = send(
            &f.app,
            get("/albums")
                .header(header::COOKIE, format!("koan_access={access}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_ne!(r.status, StatusCode::OK);
        let refresh = t["refresh_token"].as_str().unwrap().to_owned();

        let r = token(
            &f,
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh.as_str()),
                ("client_id", client_id.as_str()),
            ],
        )
        .await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.body);
        let refreshed = json(&r)["access_token"].as_str().unwrap().to_owned();
        assert!(auth::validate_access_token(&f.state.public_pem, &refreshed).is_err());
        let rotated = json(&r)["refresh_token"].as_str().unwrap().to_owned();

        // The code again: refused, and the grant it made is revoked.
        let r = token(&f, &exchange).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST);
        let r = token(
            &f,
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", rotated.as_str()),
            ],
        )
        .await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn a_redirect_uri_is_matched_as_registered_and_may_be_left_out() {
        let f = setup_at(true, Some(ORIGIN));
        // Re-serialised, this gains a trailing slash.
        let registered = "http://localhost:33418";
        let reg = register(&f, registered).await;
        let client_id = json(&reg)["client_id"].as_str().unwrap().to_owned();
        let challenge = challenge();
        for given in ["", registered] {
            let mut params = authorize_params(&client_id, &challenge);
            params[2] = ("redirect_uri", given);
            let uri = format!("/oauth/authorize?{}", query(&params));
            let r = send(&f.app, authed(&f.state, &uri).body(Body::empty()).unwrap()).await;
            assert_eq!(r.status, StatusCode::OK, "{}", r.body);
            let r = approve(&f, &params, ORIGIN).await;
            assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
            let code = code_of(r.location());
            let r = token(
                &f,
                &[
                    ("grant_type", "authorization_code"),
                    ("code", code.as_str()),
                    ("redirect_uri", given),
                    ("client_id", client_id.as_str()),
                    ("code_verifier", VERIFIER),
                ],
            )
            .await;
            assert_eq!(r.status, StatusCode::OK, "{given:?}: {}", r.body);
        }
    }

    #[tokio::test]
    async fn requests_that_do_not_hold_are_refused() {
        let f = setup_at(true, Some(ORIGIN));
        assert_eq!(
            register(&f, "http://evil.example/cb").await.status,
            StatusCode::BAD_REQUEST
        );
        let client_id = json(&register(&f, CALLBACK).await)["client_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let challenge = challenge();

        // Not this client's redirect, or not a client: a page, never a redirect.
        let mut params = authorize_params(&client_id, &challenge);
        params[2] = ("redirect_uri", "https://evil.example/cb");
        let uri = format!("/oauth/authorize?{}", query(&params));
        let r = send(&f.app, authed(&f.state, &uri).body(Body::empty()).unwrap()).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST);
        assert!(!r.headers.contains_key(header::LOCATION));
        let uri = format!(
            "/oauth/authorize?{}",
            query(&authorize_params("forged", &challenge))
        );
        let r = send(&f.app, authed(&f.state, &uri).body(Body::empty()).unwrap()).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST);

        // No PKCE: back to the client with an error.
        let mut params = authorize_params(&client_id, &challenge);
        params[4] = ("code_challenge_method", "plain");
        let uri = format!("/oauth/authorize?{}", query(&params));
        let r = send(&f.app, authed(&f.state, &uri).body(Body::empty()).unwrap()).await;
        assert!(r.location().contains("error=invalid_request"));

        // Another site's form.
        let params = authorize_params(&client_id, &challenge);
        let r = approve(&f, &params, "https://evil.example").await;
        assert_eq!(r.status, StatusCode::FORBIDDEN);

        // A wrong verifier.
        let code = code_of(approve(&f, &params, ORIGIN).await.location());
        let r = token(
            &f,
            &[
                ("grant_type", "authorization_code"),
                ("code", code.as_str()),
                ("redirect_uri", CALLBACK),
                ("client_id", client_id.as_str()),
                ("code_verifier", "not-the-verifier"),
            ],
        )
        .await;
        assert_eq!(json(&r)["error"], "invalid_grant");

        // An unknown client is told to register again.
        let r = token(
            &f,
            &[
                ("grant_type", "authorization_code"),
                ("code", code.as_str()),
                ("client_id", "forged"),
            ],
        )
        .await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED);
        assert_eq!(json(&r)["error"], "invalid_client");
    }

    #[tokio::test]
    async fn the_assistants_page_gives_the_address_once_there_is_one() {
        let f = setup_at(true, Some(ORIGIN));
        let r = send(
            &f.app,
            authed(&f.state, "/connect").body(Body::empty()).unwrap(),
        )
        .await;
        assert_eq!(r.status, StatusCode::OK);
        assert!(
            r.body.contains("value=\"https://koan.test/mcp\""),
            "{}",
            r.body
        );
        let f = setup(true);
        let r = send(
            &f.app,
            authed(&f.state, "/connect").body(Body::empty()).unwrap(),
        )
        .await;
        assert!(
            r.body.contains("needs to know its own address"),
            "{}",
            r.body
        );
    }

    #[tokio::test]
    async fn without_a_public_url_there_is_no_oauth() {
        let f = setup(true);
        for uri in [
            "/.well-known/oauth-authorization-server",
            crate::ui::RESOURCE_METADATA,
        ] {
            let r = send(&f.app, get(uri).body(Body::empty()).unwrap()).await;
            assert_eq!(r.status, StatusCode::NOT_FOUND);
        }
        assert_eq!(register(&f, CALLBACK).await.status, StatusCode::NOT_FOUND);
    }
}

#[tokio::test]
async fn the_icon_is_where_favicon_fetchers_look() {
    let f = setup(true);
    for uri in ["/favicon.ico", "/apple-touch-icon.png"] {
        let r = send(&f.app, get(uri).body(Body::empty()).unwrap()).await;
        assert_eq!(r.status, StatusCode::OK, "{uri}");
        assert_eq!(r.headers[header::CONTENT_TYPE], "image/png");
    }
}

/// Behind an authenticating proxy at 10.0.0.1 that names the account in
/// `Remote-User`.
fn setup_behind_proxy() -> Fixture {
    let proxy = super::ProxyAuth::from_config("Remote-User", &["10.0.0.1".into()]);
    setup_full(true, None, proxy)
}

fn from_peer(builder: axum::http::request::Builder, peer: &str) -> axum::http::request::Builder {
    let addr: std::net::SocketAddr = format!("{peer}:50000").parse().unwrap();
    builder.extension(axum::extract::ConnectInfo(addr))
}

#[tokio::test]
async fn the_proxy_signs_in_the_account_it_names() {
    let f = setup_behind_proxy();
    let r = send(
        &f.app,
        from_peer(get("/login?next=/artists"), "10.0.0.1")
            .header("remote-user", "alice")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(r.location(), "/ui/resume?next=%2Fartists");

    let r = send(
        &f.app,
        from_peer(get("/ui/resume?next=/artists"), "10.0.0.1")
            .header("remote-user", "alice")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(r.location(), "/artists");
    let access = r.cookie("koan_access");
    let claims = auth::validate_access_token(&f.state.public_pem, &access).unwrap();
    assert_eq!(claims.username, "alice");
}

#[tokio::test]
async fn page_loads_behind_the_proxy_resume_through_it() {
    let f = setup_behind_proxy();
    let r = send(
        &f.app,
        get("/oauth/authorize?x=y").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(r.location(), "/ui/resume?next=%2Foauth%2Fauthorize%3Fx%3Dy");

    // Without the header it falls back to the refresh cookie.
    let r = send(
        &f.app,
        get("/ui/resume?next=/queue").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(r.location(), "/auth/resume?next=%2Fqueue");
    assert!(r.cookies().is_empty());
}

#[tokio::test]
async fn the_header_counts_only_from_the_proxy_with_one_value() {
    let f = setup_behind_proxy();
    let stranger = from_peer(get("/ui/resume"), "203.0.113.9")
        .header("remote-user", "alice")
        .body(Body::empty())
        .unwrap();
    let doubled = from_peer(get("/ui/resume"), "10.0.0.1")
        .header("remote-user", "mallory")
        .header("remote-user", "alice")
        .body(Body::empty())
        .unwrap();
    let merged = from_peer(get("/ui/resume"), "10.0.0.1")
        .header("remote-user", "alice, mallory")
        .body(Body::empty())
        .unwrap();
    let unknown_peer = get("/ui/resume")
        .header("remote-user", "alice")
        .body(Body::empty())
        .unwrap();
    for req in [stranger, doubled, merged, unknown_peer] {
        let r = send(&f.app, req).await;
        assert_eq!(r.status, StatusCode::SEE_OTHER);
        assert_eq!(r.location(), "/auth/resume?next=%2F");
        assert!(r.cookies().is_empty());
    }
    let r = send(
        &f.app,
        from_peer(get("/login"), "203.0.113.9")
            .header("remote-user", "alice")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
}

/// The paths the docs tell operators to exempt from the proxy, where a
/// client's own header arrives from the proxy's address unchecked, sign in
/// no one.
#[tokio::test]
async fn paths_exempt_from_the_proxy_ignore_the_header() {
    let f = setup_behind_proxy();
    let app = f
        .app
        .clone()
        .merge(crate::auth::routes::auth_router(f.state.clone()));
    let json = |uri: &str, body: &str| {
        from_peer(Request::post(uri), "10.0.0.1")
            .header(header::HOST, HOST)
            .header(header::ORIGIN, ORIGIN)
            .header(header::CONTENT_TYPE, "application/json")
            .header("remote-user", "alice")
            .body(Body::from(body.to_owned()))
            .unwrap()
    };
    let form = |uri: &str, body: &str| {
        from_peer(Request::post(uri), "10.0.0.1")
            .header(header::HOST, HOST)
            .header(header::ORIGIN, ORIGIN)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header("remote-user", "alice")
            .body(Body::from(body.to_owned()))
            .unwrap()
    };
    let page = |uri: &str| {
        from_peer(get(uri), "10.0.0.1")
            .header("remote-user", "alice")
            .body(Body::empty())
            .unwrap()
    };
    for req in [
        json("/auth/login", r#"{"username":"alice","password":"wrong"}"#),
        json("/auth/refresh", r#"{"refresh_token":"nope"}"#),
        json("/auth/logout", r#"{"refresh_token":"nope"}"#),
        form(
            "/oauth/token",
            "grant_type=refresh_token&refresh_token=nope",
        ),
        json(
            "/oauth/register",
            r#"{"redirect_uris":["https://claude.ai/api/mcp/auth_callback"]}"#,
        ),
        page("/auth/resume"),
        form("/auth/renew", ""),
        page("/.well-known/oauth-authorization-server"),
    ] {
        let uri = req.uri().to_string();
        let r = send(&app, req).await;
        let minted = r
            .cookies()
            .iter()
            .any(|c| c.starts_with("koan_access=") && !c.starts_with("koan_access=;"));
        assert!(!minted, "{uri} signed in: {:?}", r.cookies());
        assert!(!r.body.contains("\"access_token\":\""), "{uri}: {}", r.body);
    }
}

#[tokio::test]
async fn an_account_the_server_lacks_is_refused() {
    let f = setup_behind_proxy();
    let r = send(
        &f.app,
        from_peer(get("/ui/resume"), "10.0.0.1")
            .header("remote-user", "bob")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert!(r.cookies().is_empty());
}

#[tokio::test]
async fn a_session_for_another_account_than_the_proxy_names_is_resumed() {
    let f = setup_behind_proxy();
    let r = send(
        &f.app,
        from_peer(authed(&f.state, "/albums"), "10.0.0.1")
            .header("remote-user", "bob")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(r.location(), "/ui/resume?next=%2Falbums");

    let r = send(
        &f.app,
        from_peer(authed(&f.state, "/albums"), "10.0.0.1")
            .header("remote-user", "alice")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
}

#[test]
fn proxy_auth_needs_a_header_and_a_proxy() {
    use super::ProxyAuth;
    assert!(ProxyAuth::from_config("", &["10.0.0.1".into()]).is_none());
    assert!(ProxyAuth::from_config("Remote-User", &[]).is_none());
    assert!(ProxyAuth::from_config("Remote-User", &["not an address".into()]).is_none());
    assert!(ProxyAuth::from_config("Remote User", &["10.0.0.1".into()]).is_none());

    let proxy =
        ProxyAuth::from_config("Remote-User", &["junk".into(), "172.18.0.0/16".into()]).unwrap();
    let mut headers = HeaderMap::new();
    headers.insert("remote-user", "alice".parse().unwrap());
    let at = |peer: &str| {
        let mut ext = axum::http::Extensions::new();
        ext.insert(axum::extract::ConnectInfo(
            peer.parse::<std::net::SocketAddr>().unwrap(),
        ));
        ext
    };
    assert_eq!(proxy.user(&headers, &at("172.18.0.5:1")), Some("alice"));
    assert_eq!(
        proxy.user(&headers, &at("[::ffff:172.18.0.5]:1")),
        Some("alice")
    );
    assert_eq!(proxy.user(&headers, &at("172.19.0.5:1")), None);
    headers.insert("remote-user", "alice,admin".parse().unwrap());
    assert_eq!(proxy.user(&headers, &at("172.18.0.5:1")), None);
}
