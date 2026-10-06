//! The signed-in user's scrobbling: connect a ListenBrainz account with its
//! user token, see what is still waiting to be sent, disconnect.

use axum::Extension;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use axum::response::sse::Event;
use koan_core::db::queries::scrobbling::{self, LISTENBRAINZ, ScrobbleService};

use super::pages::{COPY_ERROR, EMPTY, ERROR, SUB, respond};
use super::{UiState, events, open, patch};
use crate::auth::AuthUser;
use crate::share::{blocking, escape};

const DISCONNECT_CONFIRM: &str = "Stop sending your plays to ListenBrainz?";

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
<button class=\"quiet\" data-on:click=\"confirm('{DISCONNECT_CONFIRM}') && \
@post('/scrobbling/listenbrainz/disconnect')\">Disconnect</button>\
<div id=scrobble-result></div></div>",
        name = escape(&s.account_name),
        since = day(s.connected_at),
    )
}

fn listenbrainz(s: &UiState, user_id: i64) -> Option<Option<ScrobbleService>> {
    let db = open(&s.pool)?;
    scrobbling::service(&db.conn, user_id, LISTENBRAINZ).ok()
}

pub(super) async fn page(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
) -> Response {
    let intro = format!(
        "<h1>Scrobbling</h1><p class=\"{SUB}\">Send what you play to ListenBrainz: every play kōan's \
apps and other Subsonic clients signed in as you report to this server, as they report it.</p>"
    );
    let inner = if !s.auth_enabled {
        format!("{intro}<p class=\"{EMPTY}\">{NO_ACCOUNTS}</p>")
    } else {
        let st = s.clone();
        let service = blocking(move || listenbrainz(&st, user.user_id))
            .await
            .flatten();
        format!(
            "{intro}<h2>ListenBrainz</h2>{}",
            section(service.as_ref(), None)
        )
    };
    respond(&s, &headers, &user, "Scrobbling", &inner)
}

fn failed(message: &str) -> Response {
    events(vec![patch(
        &format!(
            "<div id=scrobble-result class=\"{COPY_ERROR}\" role=alert>{}</div>",
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
