//! The pages, one module per page, named as the nav names it. Every page has
//! the same shape ([`Page`]): the nav entry it is shown under, what it reads
//! from the request, and its body. [`serve`] draws any of them: a browser
//! asking for the address gets the shell around the body; the page itself
//! asking (a link or form that loads in place) gets the body alone, morphed
//! into `main`, with the address it now shows, so the shell and the player in
//! it stay.

use std::future::Future;

use askama::Template;
use axum::Extension;
use axum::extract::{FromRequestParts, OriginalUri, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::Event;
use axum::response::{IntoResponse, Response};
use koan_kit_web as kit;

use super::layout::{Layout, Main, Nav, Title};
use super::{UiState, document, patch};
use crate::auth::AuthUser;
use crate::html::{Html, Render};

pub(super) mod account;
pub(super) mod album;
pub(super) mod albums;
pub(super) mod artist;
pub(super) mod artists;
pub(super) mod assistants;
pub(super) mod favourites;
pub(super) mod history;
pub(super) mod library;
pub(super) mod playlist;
pub(super) mod playlists;
pub(super) mod properties;
pub(super) mod queue;
pub(super) mod recently_played;
pub(super) mod scrobbling;
pub(super) mod search;
pub(super) mod tracks;
pub(super) mod users;

/// An entry in the nav: the sidebar on a wide screen, the tab bar on a phone.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Section {
    Albums,
    Artists,
    Tracks,
    Playlists,
    Library,
    Search,
    Queue,
    RecentlyPlayed,
    Favourites,
    History,
    Account,
}

impl Section {
    /// Under the phone's Library tab, which stands for the lists the sidebar
    /// names one by one.
    pub(super) fn in_library(self) -> bool {
        matches!(
            self,
            Section::Library
                | Section::Playlists
                | Section::RecentlyPlayed
                | Section::Favourites
                | Section::History
                | Section::Tracks
        )
    }
}

/// A page: the nav entry it is shown under, what it reads from the request,
/// and how its body is drawn for the account signed in.
pub(super) trait Page: Send + 'static {
    const SECTION: Section;
    type Args: FromRequestParts<UiState, Rejection: IntoResponse> + Send;
    fn body(
        s: &UiState,
        user: &AuthUser,
        args: Self::Args,
    ) -> impl Future<Output = Result<Body, Refusal>> + Send;
}

/// Why a page has nothing of its own to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Refusal {
    /// What the address names is not there, or not this account's to see.
    NotFound,
    /// The library could not be read.
    Unavailable,
}

impl Refusal {
    fn status(self) -> StatusCode {
        match self {
            Refusal::NotFound => StatusCode::NOT_FOUND,
            Refusal::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        }
    }

    fn body(self) -> Body {
        let view = match self {
            Refusal::NotFound => Refused {
                title: "Not here",
                detail: "Nothing in the library has this address: it may have been removed, or the link is wrong.",
            },
            Refusal::Unavailable => Refused {
                title: "The library is unavailable",
                detail: "The server could not read its library. Try again in a moment.",
            },
        };
        Body::of(view.title, &view).status(self.status())
    }
}

/// A page's place drawn when it has nothing of its own to show.
#[derive(Template)]
#[template(path = "layout/refused.html")]
struct Refused {
    title: &'static str,
    detail: &'static str,
}

/// A page's body as its module draws it, with what the shell needs to know of
/// it.
pub(super) struct Body {
    html: Html,
    status: StatusCode,
    /// What the document's title names.
    title: String,
    /// Signals of the page's own, beside those every page has.
    signals: serde_json::Map<String, serde_json::Value>,
}

impl Body {
    pub(super) fn of(title: impl Into<String>, view: &impl Template) -> Self {
        Self {
            html: view.html(),
            status: StatusCode::OK,
            title: title.into(),
            signals: serde_json::Map::new(),
        }
    }

    pub(super) fn status(mut self, status: StatusCode) -> Self {
        self.status = status;
        self
    }

    pub(super) fn signal(mut self, name: &str, value: impl Into<serde_json::Value>) -> Self {
        self.signals.insert(name.to_owned(), value.into());
        self
    }
}

/// Page `P`'s body, and what its live stream carries on from, read before the
/// body was drawn so that nothing that moves meanwhile is missed.
async fn draw<P: Page>(s: &UiState, user: &AuthUser, args: P::Args) -> (Body, super::stream::Seen) {
    let seen = super::stream::Seen::now(s);
    let body = P::body(s, user, args).await.unwrap_or_else(Refusal::body);
    (body, seen)
}

/// The signals a page starts with: its own, the address it shows, and what
/// its stream carries on from.
fn page_signals(body: &mut Body, url: &str, seen: &super::stream::Seen) -> serde_json::Value {
    let mut all = std::mem::take(&mut body.signals);
    all.insert("url".into(), url.into());
    all.insert("seen".into(), seen.signal());
    serde_json::Value::Object(all)
}

/// Page `P` shown in place, as a link or form that loads in place is
/// answered: its body morphed into `main`, the nav marking it, the title,
/// any flash cleared, and its signals. An action that takes the page
/// somewhere answers with these too.
pub(super) async fn shown<P: Page>(
    s: &UiState,
    user: &AuthUser,
    args: P::Args,
    url: &str,
) -> Vec<Event> {
    let (mut body, seen) = draw::<P>(s, user, args).await;
    let signals = page_signals(&mut body, url, &seen);
    vec![
        patch(&Html::from_iter([
            Main { body: body.html }.html(),
            Title { name: &body.title }.html(),
            Nav::new(s, user, P::SECTION).html(),
            kit::Flash::region().html(),
        ])),
        super::signals(signals),
    ]
}

/// Draw page `P`: whole for a browser, or in place for the page itself.
pub(super) async fn serve<P: Page>(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    args: P::Args,
) -> Response {
    let url = s.absolute(&askama_datastar::address(&uri));
    if askama_datastar::is_datastar(&headers) {
        return askama_datastar::events(shown::<P>(&s, &user, args, &url).await);
    }
    let (body, seen) = draw::<P>(&s, &user, args).await;
    whole(&s, &user, P::SECTION, body, &url, &seen)
}

/// A page drawn by a handler of its own rather than from its address alone,
/// such as one carrying what an action just said: whole for a browser, in
/// place for the page.
pub(super) fn respond(
    s: &UiState,
    user: &AuthUser,
    headers: &HeaderMap,
    uri: &axum::http::Uri,
    section: Section,
    mut body: Body,
) -> Response {
    let url = s.absolute(&askama_datastar::address(uri));
    let seen = super::stream::Seen::now(s);
    if askama_datastar::is_datastar(headers) {
        let signals = page_signals(&mut body, &url, &seen);
        return askama_datastar::events([
            patch(&Html::from_iter([
                Main { body: body.html }.html(),
                Title { name: &body.title }.html(),
                Nav::new(s, user, section).html(),
                kit::Flash::region().html(),
            ])),
            super::signals(signals),
        ]);
    }
    whole(s, user, section, body, &url, &seen)
}

fn whole(
    s: &UiState,
    user: &AuthUser,
    section: Section,
    mut body: Body,
    url: &str,
    seen: &super::stream::Seen,
) -> Response {
    let signals = page_signals(&mut body, url, seen);
    let layout = Layout {
        base: s.base(),
        title: Title { name: &body.title }.html(),
        nav: Nav::new(s, user, section).html(),
        main: Main { body: body.html }.html(),
        signals: signals.to_string(),
        live: s.absolute(super::paths::LIVE),
        proxied: s.proxy_auth.is_some(),
    };
    document(body.status, &layout.html())
}

#[cfg(test)]
mod tests {
    use crate::ui::fixture::*;

    #[tokio::test]
    async fn scripts_are_compressed_for_a_browser_that_accepts_it() {
        let f = setup(true);
        let r = send(
            &f.app,
            get("/ui/assets/datastar.js")
                .header(header::ACCEPT_ENCODING, "br, gzip")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(r.status, StatusCode::OK);
        assert_eq!(r.headers[header::CONTENT_ENCODING], "br");
    }

    #[tokio::test]
    async fn streams_serve_ranges_to_a_signed_in_user() {
        let f = setup(true);
        let uri = format!("/ui/stream/{}", f.track_id);
        let r = send(
            &f.app,
            authed(&f.state, &uri)
                .header(header::RANGE, "bytes=2-4")
                .header(header::ACCEPT_ENCODING, "br")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(
            (r.status, r.body.as_str()),
            (StatusCode::PARTIAL_CONTENT, "234")
        );
        assert!(!r.headers.contains_key(header::CONTENT_ENCODING));
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
            "/library".to_owned(),
            "/favourites".to_owned(),
            "/history".to_owned(),
            "/recent".to_owned(),
            "/tracks".to_owned(),
            "/tracks?fav=1&recent=1&sort=played".to_owned(),
            "/albums?recent=1".to_owned(),
            "/account".to_owned(),
            "/scrobbling".to_owned(),
            "/users".to_owned(),
            "/connect".to_owned(),
            format!("/track/{}/properties", f.track_id),
        ] {
            let r = send(&f.app, page(&uri)).await;
            assert_eq!(r.status, StatusCode::OK, "{uri}");
            rendered.push((uri, r.body));
        }
        for (uri, body) in [
            ("/account/keys", r#"{"keyname":"phone"}"#),
            ("/account/app-passwords", r#"{"appname":"Arpeggi"}"#),
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
        assert!(
            full.body.contains("rel=\"icon\""),
            "pages carry the favicon"
        );
        assert!(full.body.contains("aria-label=\"Library\">"));
        assert!(full.body.contains("Wet &#60;Moss&#62; &#38; Stone"));
        assert!(full.body.contains("Hymn &#60;to&#62; Moisture"));
        assert!(!full.body.contains("<Moss>") && !full.body.contains("<to>"));
        // kit: raw, the assertion that no page carries a style attribute names one.
        assert!(!full.body.contains("<script>") && !full.body.contains(" style="));
        let csp = full.headers[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap();
        assert!(
            csp.contains("script-src 'self' 'unsafe-eval' 'sha256-")
                && !csp.contains("unsafe-inline")
        );
        assert!(crate::share::import_map_allowed(&full.body, csp), "{csp}");
        // The stylesheet's URL changes with its contents, so an upgrade never pairs
        // new markup with a cached stylesheet.
        let css = &super::ASSETS.css;
        assert!(css.starts_with("./ui/assets/ui.css?v=") && full.body.contains(css.as_str()));
        let served = send(&f.app, get(&css[1..]).body(Body::empty()).unwrap()).await;
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
                .contains("data-title=\"Hymn &#60;to&#62; Moisture\"")
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
        assert!(r.body.contains("Wet &#60;Moss&#62; &#38; Stone"));
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
        assert!(r.body.contains("data: elements <div id=\"results\">"));

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
        let src = format!("src=\"ui/cover/{}?size=600&#38;v=", f.album_id);
        let src_raw = format!("src=\"ui/cover/{}?size=600&v=", f.album_id);
        assert!(
            page.body.contains(&src) || page.body.contains(&src_raw),
            "{}",
            page.body
        );
        assert!(
            page.body
                .contains("loading=\"lazy\" decoding=\"async\" width=\"600\" height=\"600\"")
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
        assert!(kept[0].file_name().to_string_lossy().ends_with("-300.jpg"));
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
    async fn under_a_path_pages_resolve_their_links_from_it() {
        let f = setup_at(true, Some("https://koan.test/koan/"));
        let r = send(&f.app, get("/login").body(Body::empty()).unwrap()).await;
        assert!(r.body.contains("<base href=\"/koan/\">"), "{}", r.body);
        let r = send(
            &f.app,
            authed(&f.state, "/albums").body(Body::empty()).unwrap(),
        )
        .await;
        assert!(r.body.contains("<base href=\"/koan/\">"));
        assert!(
            r.body.contains("href=\"albums\""),
            "links are relative to it"
        );
        let r = send(&setup(true).app, get("/login").body(Body::empty()).unwrap()).await;
        assert!(r.body.contains("<base href=\"/\">"));
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
}
