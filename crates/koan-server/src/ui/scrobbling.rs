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

use super::pages::{COPY_ERROR, EMPTY, ERROR, SUB, respond};
use super::{UiState, events, open, patch, see_other};
use crate::auth::AuthUser;
use crate::share::{blocking, escape};

const DISCONNECT_CONFIRM: &str = "Stop sending your plays to ListenBrainz?";

const LASTFM_DISCONNECT_CONFIRM: &str = "Stop sending your plays to Last.fm?";

const NO_ACCOUNTS: &str = "Scrobbling belongs to accounts, and this server runs without sign-in.";

fn day(secs: i64) -> String {
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|t| t.format("%-d %b %Y").to_string())
        .unwrap_or_default()
}

fn plays(n: i64) -> String {
    if n == 1 {
        "1 play".to_owned()
    } else {
        format!("{n} plays")
    }
}

/// The ListenBrainz section: the form to connect, or the connection.
fn section(service: Option<&ScrobbleService>, message: Option<&str>) -> String {
    let message = message.map_or_else(String::new, |m| format!("<p>{}</p>", escape(m)));
    let Some(s) = service else {
        return format!(
            "<div id=listenbrainz><p>Paste your user token from \
<a href=\"https://listenbrainz.org/settings/\">your ListenBrainz settings</a>. \
The plays already in your history here are sent when you connect.</p>\
<form class=\"mb-2 flex max-w-form gap-2\" data-on:submit__prevent=\"@post('/scrobbling/listenbrainz')\">\
<input class=\"flex-1\" type=password name=token data-bind:lbtoken placeholder=\"User token\" required \
autocomplete=off aria-label=\"ListenBrainz user token\"><button class=\"primary\">Connect</button></form>\
{message}<div id=scrobble-result></div></div>"
        );
    };
    let state = match &s.error {
        Some(e) => format!("<p class=\"{ERROR}\" role=alert>{}</p>", escape(e)),
        None if s.pending > 0 => format!(
            "<p class=\"{EMPTY}\">{} waiting to be sent.</p>",
            plays(s.pending)
        ),
        None => format!("<p class=\"{EMPTY}\">Every play has been sent.</p>"),
    };
    let reconnect = if s.error.is_some() {
        "<p>Disconnect, then connect again with a current token. Plays recorded meanwhile are kept and sent.</p>"
    } else {
        ""
    };
    format!(
        "<div id=listenbrainz><p>Connected as <strong>{name}</strong> since {since}.</p>{message}{state}{reconnect}\
<button class=\"quiet bad\" data-on:click=\"confirm('{DISCONNECT_CONFIRM}') && \
@post('/scrobbling/listenbrainz/disconnect')\">Disconnect</button>\
<div id=scrobble-result></div></div>",
        name = escape(&s.account_name),
        since = day(s.connected_at),
    )
}

/// The Last.fm section: what the server lacks to offer it, the link that
/// sends the person to Last.fm, or the connection.
fn lastfm_section(
    service: Option<&ScrobbleService>,
    offered: bool,
    message: Option<&str>,
) -> String {
    let message = message.map_or_else(String::new, |m| {
        format!("<p class=\"{ERROR}\" role=alert>{}</p>", escape(m))
    });
    let Some(s) = service else {
        if !offered {
            return format!(
                "<div id=lastfm><p class=\"{EMPTY}\">Last.fm is not set up on this server. \
Its admin needs to add a Last.fm API key and secret.</p>{message}</div>"
            );
        }
        return format!(
            "<div id=lastfm><p>You will be sent to Last.fm to approve kōan, then back here. \
The plays in your history from the last two weeks are sent when you connect: Last.fm takes no older ones.</p>\
<a class=\"primary\" href=\"/scrobbling/lastfm/connect\">Connect Last.fm</a>{message}</div>"
        );
    };
    let state = match &s.error {
        Some(e) => format!("<p class=\"{ERROR}\" role=alert>{}</p>", escape(e)),
        None if s.pending > 0 => format!(
            "<p class=\"{EMPTY}\">{} waiting to be sent.</p>",
            plays(s.pending)
        ),
        None => format!("<p class=\"{EMPTY}\">Every play has been sent.</p>"),
    };
    let reconnect = if s.error.is_some() {
        "<p>Disconnect, then connect again. Plays recorded meanwhile are kept and sent.</p>"
    } else {
        ""
    };
    format!(
        "<div id=lastfm><p>Connected as <strong>{name}</strong> since {since}.</p>{message}{state}{reconnect}\
<button class=\"quiet bad\" data-on:click=\"confirm('{LASTFM_DISCONNECT_CONFIRM}') && \
@post('/scrobbling/lastfm/disconnect')\">Disconnect</button>\
<div id=lastfm-result></div></div>",
        name = escape(&s.account_name),
        since = day(s.connected_at),
    )
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
    let intro = format!(
        "<h1>Scrobbling</h1><p class=\"{SUB}\">Send what you play to ListenBrainz or Last.fm: every play kōan's \
apps and other Subsonic clients signed in as you report to this server, as they report it.</p>"
    );
    let inner = if !s.auth_enabled {
        format!("{intro}<p class=\"{EMPTY}\">{NO_ACCOUNTS}</p>")
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
        let offered = lastfm::configured() && s.public_url.is_some();
        let message = match lastfm_message {
            None if lastfm::configured() && s.public_url.is_none() => {
                Some(lastfm::LastfmError::NoPublicUrl.to_string())
            }
            other => other.map(str::to_owned),
        };
        format!(
            "{intro}<h2>ListenBrainz</h2>{}<h2>Last.fm</h2>{}",
            section(brainz.flatten().as_ref(), None),
            lastfm_section(fm.flatten().as_ref(), offered, message.as_deref()),
        )
    };
    respond(s, headers, user, "Scrobbling", &inner)
}

/// Send the person to Last.fm to approve kōan.
pub(super) async fn lastfm_connect(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
) -> Response {
    let Some(public) = s.public_url.as_deref().filter(|_| s.auth_enabled) else {
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

const NO_ADDRESS: &str = "This server does not know where it is reached from outside, which Last.fm needs to send you back. Its admin needs to set sharing.public_url.";

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
    events(vec![patch(
        &format!(
            "<div id={id} class=\"{COPY_ERROR}\" role=alert>{}</div>",
            escape(message)
        ),
        None,
    )])
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
