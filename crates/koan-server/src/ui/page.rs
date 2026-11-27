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
