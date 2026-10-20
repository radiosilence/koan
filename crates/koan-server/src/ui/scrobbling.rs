//! The signed-in user's scrobbling: connect a ListenBrainz account with its
//! user token, or a Last.fm account through Last.fm's own sign-in, see what is
//! still waiting to be sent, disconnect.

use axum::Extension;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::response::sse::Event;
use koan_core::db::queries::scrobbling::{self, LASTFM, LISTENBRAINZ, ScrobbleService};
use koan_core::scrobbling::lastfm;

use super::pages::{COPY_ERROR, Failed, respond};
use super::{UiState, events, open, patch, see_other};
use crate::auth::AuthUser;
use crate::html::{Html, Render};
use crate::share::blocking;
use askama::Template;

pub(super) const NO_ACCOUNTS: &str =
    "Scrobbling belongs to accounts, and this server runs without sign-in.";

pub(super) fn day(secs: i64) -> String {
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|t| t.format("%-d %b %Y").to_string())
        .unwrap_or_default()
}

pub(super) fn plays(n: i64) -> String {
    if n == 1 {
        "1 play".to_owned()
    } else {
        format!("{n} plays")
    }
}

#[derive(Template)]
#[template(path = "scrobbling/listenbrainz.html")]
struct ListenBrainz<'a> {
    service: Option<&'a ScrobbleService>,
    message: Option<&'a str>,
}

fn section(service: Option<&ScrobbleService>, message: Option<&str>) -> Html {
    ListenBrainz { service, message }.html()
}

#[derive(Template)]
#[template(path = "scrobbling/lastfm.html")]
struct Lastfm<'a> {
    service: Option<&'a ScrobbleService>,
    offered: bool,
    message: Option<&'a str>,
}

fn lastfm_section(service: Option<&ScrobbleService>, offered: bool, message: Option<&str>) -> Html {
    Lastfm {
        service,
        offered,
        message,
    }
    .html()
}

/// The scrobbling page. `sections` is the rendered ListenBrainz and Last.fm
/// sections, absent when the server runs without sign-in.
#[derive(Template)]
#[template(path = "pages/scrobbling.html")]
struct ScrobblingPage {
    sections: Option<(Html, Html)>,
}

fn connection(s: &UiState, user_id: i64, service: &str) -> Option<Option<ScrobbleService>> {
    let db = open(&s.pool)?;
    scrobbling::service(&db.conn, user_id, service).ok()
}

pub(super) async fn page(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
) -> Response {
    render(&s, &user, &headers, None).await
}

/// The page, with `lastfm_message` under the Last.fm section.
async fn render(
    s: &UiState,
    user: &AuthUser,
    headers: &HeaderMap,
    lastfm_message: Option<&str>,
) -> Response {
    let sections = if !s.auth_enabled {
        None
    } else {
        let st = s.clone();
        let user_id = user.user_id;
        let (brainz, fm) = blocking(move || {
            Some((
                connection(&st, user_id, LISTENBRAINZ),
                connection(&st, user_id, LASTFM),
            ))
        })
        .await
        .unwrap_or((None, None));
        // Without the address Last.fm sends a person back to, it cannot be offered.
        let offered = lastfm::configured() && s.server_url.is_some();
        let message = match lastfm_message {
            None if lastfm::configured() && s.server_url.is_none() => {
                Some(lastfm::LastfmError::NoServerUrl.to_string())
            }
            other => other.map(str::to_owned),
        };
        Some((
            section(brainz.flatten().as_ref(), None),
            lastfm_section(fm.flatten().as_ref(), offered, message.as_deref()),
        ))
    };
    let inner = ScrobblingPage { sections }.html();
    respond(s, headers, user, "Scrobbling", &inner)
}

/// Send the person to Last.fm to approve kōan.
pub(super) async fn lastfm_connect(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
) -> Response {
    let Some(public) = s.server_url.as_deref().filter(|_| s.auth_enabled) else {
        return render(&s, &user, &headers, Some(NO_ADDRESS)).await;
    };
    let public = public.trim_end_matches('/').to_owned();
    let sent = lastfm::begin(user.user_id)
        .and_then(|state| lastfm::auth_url(&format!("{public}/scrobbling/lastfm/return/{state}")));
    match sent {
        Ok(url) => see_other(&url),
        Err(e) => render(&s, &user, &headers, Some(&e.to_string())).await,
    }
}

const NO_ADDRESS: &str = "This server does not know where it is reached from outside, which Last.fm needs to send you back. Its admin needs to set server.url.";

/// Where Last.fm sends the person back, with the `token` to exchange.
pub(super) async fn lastfm_return(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Path(state): Path<String>,
    Query(query): Query<std::collections::HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    if !s.auth_enabled {
        return render(&s, &user, &headers, None).await;
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
        Ok(_) => see_other("/scrobbling"),
        Err(e) => render(&s, &user, &headers, Some(&e.to_string())).await,
    }
}

pub(super) async fn lastfm_disconnect(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
) -> Response {
    let done = blocking(move || {
        let db = open(&s.pool)?;
        scrobbling::disconnect(&db.conn, user.user_id, LASTFM).ok()
    })
    .await;
    if done.is_none() {
        return failed_in("lastfm-result", "The connection could not be removed.");
    }
    events(vec![patch(
        &lastfm_section(None, true, Some("Disconnected.")),
        None,
    )])
}

fn failed(message: &str) -> Response {
    failed_in("scrobble-result", message)
}

fn failed_in(id: &str, message: &str) -> Response {
    let html = Failed {
        id,
        class: COPY_ERROR,
        message,
    }
    .html();
    events(vec![patch(&html, None)])
}

/// Datastar posts its signals as JSON; the token is `lbtoken`.
pub(super) async fn connect(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    body: Bytes,
) -> Response {
    if !s.auth_enabled {
        return failed(NO_ACCOUNTS);
    }
    let pasted = serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v.get("lbtoken")?.as_str().map(str::to_owned))
        .unwrap_or_default();
    let checked = tokio::task::spawn_blocking(move || {
        koan_core::scrobbling::check_listenbrainz_token(&pasted)
    })
    .await;
    let (token, name) = match checked {
        Ok(Ok(checked)) => checked,
        Ok(Err(e)) => return failed(&e.to_string()),
        Err(_) => return failed("The token could not be checked."),
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
        Some(Err(e)) => return failed(&e.to_string()),
        None => return failed("The connection could not be saved."),
    };
    let message = if queued > 0 {
        format!(
            "Connected. {} from your history are on their way.",
            plays(queued as i64)
        )
    } else {
        "Connected.".to_owned()
    };
    events(vec![
        patch(&section(Some(&service), Some(&message)), None),
        Event::default()
            .event("datastar-patch-signals")
            .data("signals {\"lbtoken\":\"\"}"),
    ])
}

pub(super) async fn disconnect(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
) -> Response {
    let done = blocking(move || {
        let db = open(&s.pool)?;
        scrobbling::disconnect(&db.conn, user.user_id, LISTENBRAINZ).ok()
    })
    .await;
    if done.is_none() {
        return failed("The connection could not be removed.");
    }
    events(vec![patch(&section(None, Some("Disconnected.")), None)])
}
