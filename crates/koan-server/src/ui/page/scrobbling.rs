//! Scrobbling: the account's plays sent on to ListenBrainz, connected with a
//! user token, or to Last.fm, connected through Last.fm's own sign-in; what
//! is still waiting to be sent; disconnecting.

use askama::Template;
use axum::Extension;
use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::HeaderMap;
use axum::response::Response;
use koan_core::db::queries::scrobbling::{self, LASTFM, LISTENBRAINZ, ScrobbleService};
use koan_core::scrobbling::lastfm;
use koan_kit_web as kit;

use super::{Body, Page, Refusal, Section};
use crate::auth::AuthUser;
use crate::html::{Html, Render};
use crate::share::blocking;
use crate::ui::{Logged, UiState, flash_bad, open, patch, paths, see_other};

const NO_ACCOUNTS: &str = "Scrobbling belongs to accounts, and this server runs without sign-in.";

const NO_ADDRESS: &str = "This server does not know where it is reached from outside, which Last.fm needs to send you back. Its admin needs to set server.url.";

/// A day as the account pages write it: "5 Oct 2026".
pub(in crate::ui) fn day(secs: i64) -> String {
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|t| t.format("%-d %b %Y").to_string())
        .unwrap_or_default()
}

fn plays(n: i64) -> String {
    crate::ui::parts::count(n.max(0) as u64, "play", "plays")
}

/// A connection's state, in its tone: its last error, or how many plays are
/// still to send.
fn state(s: &ScrobbleService) -> kit::Status<'static> {
    match &s.error {
        Some(e) => kit::Status::bad(e.clone()),
        None if s.pending > 0 => {
            kit::Status::wait(format!("{} waiting to be sent", plays(s.pending)))
        }
        None => kit::Status::ok("Every play has been sent"),
    }
}

/// The ListenBrainz section: the form to connect, or the connection.
#[derive(Template)]
#[template(path = "scrobbling/listenbrainz.html")]
struct ListenBrainz<'a> {
    service: Option<&'a ScrobbleService>,
    message: Option<&'a str>,
}

impl ListenBrainz<'_> {
    fn id(&self) -> &'static str {
        "listenbrainz"
    }

    fn form(&self) -> kit::Form<'static> {
        kit::Form::post(paths::listenbrainz())
    }

    fn token(&self) -> kit::Input<'static> {
        kit::Input::password("lbtoken")
            .placeholder("User token")
            .label("ListenBrainz user token")
            .autocomplete("off")
            .required()
            .class("flex-1")
    }

    fn disconnect(&self) -> kit::Button<'static> {
        kit::Button::quiet("Disconnect")
            .bad()
            .post(paths::listenbrainz_disconnect())
            .confirm("Stop sending your plays to ListenBrainz?", "Disconnect")
    }

    fn state(&self, s: &ScrobbleService) -> kit::Status<'static> {
        state(s)
    }

    fn since(&self, s: &ScrobbleService) -> String {
        day(s.connected_at)
    }
}

/// The Last.fm section: what the server lacks to offer it, the link that
/// sends the person to Last.fm, or the connection.
#[derive(Template)]
#[template(path = "scrobbling/lastfm.html")]
struct Lastfm<'a> {
    service: Option<&'a ScrobbleService>,
    offered: bool,
    message: Option<&'a str>,
}

impl Lastfm<'_> {
    fn id(&self) -> &'static str {
        "lastfm"
    }

    fn connect(&self) -> &'static str {
        paths::lastfm_connect()
    }

    fn disconnect(&self) -> kit::Button<'static> {
        kit::Button::quiet("Disconnect")
            .bad()
            .post(paths::lastfm_disconnect())
            .confirm("Stop sending your plays to Last.fm?", "Disconnect")
    }

    fn state(&self, s: &ScrobbleService) -> kit::Status<'static> {
        state(s)
    }

    fn since(&self, s: &ScrobbleService) -> String {
        day(s.connected_at)
    }
}

/// The page. `sections` is the rendered ListenBrainz and Last.fm sections,
/// absent when the server runs without sign-in.
#[derive(Template)]
#[template(path = "scrobbling/page.html")]
struct ScrobblingPage {
    sections: Option<(Html, Html)>,
}

impl ScrobblingPage {
    fn no_accounts(&self) -> &'static str {
        NO_ACCOUNTS
    }
}

pub(in crate::ui) struct Scrobbling;

impl Page for Scrobbling {
    const SECTION: Section = Section::Account;
    type Args = ();

    async fn body(s: &UiState, user: &AuthUser, (): ()) -> Result<Body, Refusal> {
        body(s, user, None).await
    }
}

fn connection(s: &UiState, user_id: i64, service: &str) -> Option<Option<ScrobbleService>> {
    let db = open(&s.pool)?;
    scrobbling::service(&db.conn, user_id, service).logged()
}

/// Whether Last.fm can be offered: it needs the address it sends a person
/// back to.
fn offered(s: &UiState) -> bool {
    lastfm::configured() && s.server_url.is_some()
}

/// The page, with `lastfm_message` under the Last.fm section.
async fn body(s: &UiState, user: &AuthUser, lastfm_message: Option<&str>) -> Result<Body, Refusal> {
    let sections = if s.auth_enabled {
        let (st, user_id) = (s.clone(), user.user_id);
        let (brainz, fm) = blocking(move || {
            Some((
                connection(&st, user_id, LISTENBRAINZ)?,
                connection(&st, user_id, LASTFM)?,
            ))
        })
        .await
        .ok_or(Refusal::Unavailable)?;
        let message = match lastfm_message {
            None if lastfm::configured() && s.server_url.is_none() => {
                Some(lastfm::LastfmError::NoServerUrl.to_string())
            }
            other => other.map(str::to_owned),
        };
        Some((
            ListenBrainz {
                service: brainz.as_ref(),
                message: None,
            }
            .html(),
            Lastfm {
                service: fm.as_ref(),
                offered: offered(s),
                message: message.as_deref(),
            }
            .html(),
        ))
    } else {
        None
    };
    Ok(Body::of("Scrobbling", &ScrobblingPage { sections }))
}

/// The page with `message` under the Last.fm section, for the address it was
/// asked at.
async fn with_message(
    s: &UiState,
    user: &AuthUser,
    headers: &HeaderMap,
    uri: &axum::http::Uri,
    message: &str,
) -> Response {
    let body = body(s, user, Some(message))
        .await
        .unwrap_or_else(super::Refusal::body);
    super::respond(s, user, headers, uri, Section::Account, body)
}

/// Send the person to Last.fm to approve kōan.
pub(in crate::ui) async fn lastfm_connect(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Response {
    let Some(public) = s.server_url.as_deref().filter(|_| s.auth_enabled) else {
        return with_message(&s, &user, &headers, &uri, NO_ADDRESS).await;
    };
    let public = public.trim_end_matches('/').to_owned();
    let sent = lastfm::begin(user.user_id)
        .and_then(|state| lastfm::auth_url(&format!("{public}/{}", paths::lastfm_return(&state))));
    match sent {
        Ok(url) => see_other(&url),
        Err(e) => with_message(&s, &user, &headers, &uri, &e.to_string()).await,
    }
}

/// Where Last.fm sends the person back, with the `token` to exchange.
pub(in crate::ui) async fn lastfm_return(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Path(state): Path<String>,
    Query(query): Query<std::collections::HashMap<String, String>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Response {
    if !s.auth_enabled {
        return see_other(paths::SCROBBLING);
    }
    let token = query.get("token").cloned().unwrap_or_default();
    let user_id = user.user_id;
    let exchanged = tokio::task::spawn_blocking(move || lastfm::session(user_id, &state, &token))
        .await
        .unwrap_or(Err(lastfm::LastfmError::Unreachable));
    let connected = match exchanged {
        Ok((key, name)) => {
            let st = s.clone();
            blocking(move || {
                let db = open(&st.pool)?;
                Some(lastfm::connect(&db.conn, user_id, &key, &name))
            })
            .await
            .unwrap_or(Err(lastfm::LastfmError::Saving))
        }
        Err(e) => Err(e),
    };
    match connected {
        // A page of its own address would be reloaded with a spent state.
        Ok(_) => see_other(paths::SCROBBLING),
        Err(e) => with_message(&s, &user, &headers, &uri, &e.to_string()).await,
    }
}

pub(in crate::ui) async fn lastfm_disconnect(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
) -> Response {
    let offered = offered(&s);
    let done = blocking(move || {
        let db = open(&s.pool)?;
        scrobbling::disconnect(&db.conn, user.user_id, LASTFM).logged()
    })
    .await;
    if done.is_none() {
        return flash_bad("The connection could not be removed.");
    }
    let html = Lastfm {
        service: None,
        offered,
        message: Some("Disconnected."),
    }
    .html();
    askama_datastar::events([patch(&html), kit::Confirm::close().into()])
}

#[derive(serde::Deserialize)]
pub(in crate::ui) struct Token {
    lbtoken: String,
}

/// Connect ListenBrainz with the user token the form posts.
pub(in crate::ui) async fn connect(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    axum::Form(Token { lbtoken }): axum::Form<Token>,
) -> Response {
    if !s.auth_enabled {
        return flash_bad(NO_ACCOUNTS);
    }
    let checked = tokio::task::spawn_blocking(move || {
        koan_core::scrobbling::check_listenbrainz_token(&lbtoken)
    })
    .await;
    let (token, name) = match checked {
        Ok(Ok(checked)) => checked,
        Ok(Err(e)) => return flash_bad(&e.to_string()),
        Err(_) => return flash_bad("The token could not be checked."),
    };
    let connected = blocking(move || {
        let db = open(&s.pool)?;
        Some(koan_core::scrobbling::connect_listenbrainz(
            &db.conn,
            user.user_id,
            &token,
            &name,
        ))
    })
    .await;
    let (queued, service) = match connected {
        Some(Ok(connected)) => connected,
        Some(Err(e)) => return flash_bad(&e.to_string()),
        None => return flash_bad("The connection could not be saved."),
    };
    let message = if queued > 0 {
        format!(
            "Connected. {} from your history are on their way.",
            plays(queued as i64)
        )
    } else {
        "Connected.".to_owned()
    };
    // Replaced rather than morphed, so the token typed into the form goes.
    let html = ListenBrainz {
        service: Some(&service),
        message: Some(&message),
    }
    .html();
    askama_datastar::events([html
        .patch()
        .mode(askama_datastar::ElementPatchMode::Replace)
        .into()])
}

pub(in crate::ui) async fn disconnect(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
) -> Response {
    let done = blocking(move || {
        let db = open(&s.pool)?;
        scrobbling::disconnect(&db.conn, user.user_id, LISTENBRAINZ).logged()
    })
    .await;
    if done.is_none() {
        return flash_bad("The connection could not be removed.");
    }
    let html = ListenBrainz {
        service: None,
        message: Some("Disconnected."),
    }
    .html();
    askama_datastar::events([patch(&html), kit::Confirm::close().into()])
}
