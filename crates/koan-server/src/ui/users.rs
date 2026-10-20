//! Accounts, for admins: create one, change its role or password, delete it,
//! and produce an invite for it — the link and the email to send it in. The
//! server sends nothing itself; the admin sends the email from their own
//! client.

use askama::Template;
use axum::Extension;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::Event;
use axum::response::{IntoResponse, Response};
use koan_core::auth::Role;
use koan_core::db::queries::auth::{self as users, UserRow};
use koan_core::invite::{self, AccountError, Invite};

use super::pages::respond;
use super::{UiState, events, open, patch};
use crate::auth::AuthUser;
use crate::html::{Html, NOTHING, Render};
use crate::share::blocking;

/// A one-line form: the field, an option or two, the button.
pub(super) const FORM: &str = "mb-2 flex max-w-form gap-2";

pub(super) const ROLES: [(Role, &str); 3] = [
    (Role::Readonly, "Listen only"),
    (Role::User, "Listen and edit"),
    (Role::Admin, "Admin"),
];

#[derive(Template)]
#[template(path = "users/list.html")]
struct UserList<'a> {
    rows: &'a [UserRow],
    me: i64,
}

fn user_list(rows: &[UserRow], me: i64) -> Html {
    UserList { rows, me }.html()
}

/// What the result area under the form shows: rendered markup, or a message,
/// which is an error when the flag is set.
#[derive(Template)]
#[template(path = "users/result.html")]
struct UserResult<'a> {
    html: &'a Html,
    message: Option<(&'a str, bool)>,
}

fn result(html: &Html) -> Event {
    let html = UserResult {
        html,
        message: None,
    };
    patch(&html.html(), None)
}

fn message(text: &str, error: bool) -> Event {
    let html = UserResult {
        html: &NOTHING,
        message: Some((text, error)),
    };
    patch(&html.html(), None)
}

fn failure(text: &str) -> Event {
    message(text, true)
}

/// The invite link as a QR code, for a phone to scan off the screen. Dark on
/// white whatever the theme, since scanners expect that contrast; each module
/// a whole 4px, so edges stay crisp. The code carries the invite's token, as
/// the link beside it does.
fn invite_qr(link: &str) -> Option<Html> {
    use qrcode::render::svg;
    let code = match qrcode::QrCode::new(link) {
        Ok(code) => code,
        Err(e) => {
            log::warn!("web UI: invite link too long for a QR code: {e}");
            return None;
        }
    };
    let svg = code
        .render::<svg::Color>()
        .module_dimensions(4, 4)
        .dark_color(svg::Color("#000"))
        .light_color(svg::Color("#fff"))
        .build();
    svg.find("<svg").map(|at| Html::trusted(&svg[at..]))
}

#[derive(Template)]
#[template(path = "users/invite.html")]
struct InvitePanel<'a> {
    invite: &'a Invite,
    link: String,
    qr: Option<Html>,
}

fn invite_panel(invite: &Invite) -> Html {
    let link = invite.link();
    InvitePanel {
        invite,
        qr: invite_qr(&link),
        link,
    }
    .html()
}

/// An invite carrying a token signed with this server's key.
fn token_invite(
    s: &UiState,
    server: &str,
    id: i64,
    username: &str,
    password: Option<&str>,
) -> Result<Invite, AccountError> {
    let keys = crate::auth::signing_keys().map_err(|e| AccountError::Other(Box::new(e)))?;
    let db = open(&s.pool).ok_or_else(|| AccountError::Other("no database".into()))?;
    let token = invite::mint_token(&db.conn, &keys.0, id)?;
    Ok(Invite::with_token(server, username, &token, password))
}

fn forbidden() -> Response {
    (StatusCode::FORBIDDEN, "admins only").into_response()
}

fn usable(s: &UiState, user: &AuthUser) -> bool {
    s.auth_enabled && user.role == Role::Admin
}

fn list(s: &UiState) -> Vec<UserRow> {
    open(&s.pool)
        .and_then(|db| users::list_users(&db.conn).ok())
        .unwrap_or_default()
}

/// The users page. `list` is the rendered account list, absent when the
/// server runs without sign-in.
#[derive(Template)]
#[template(path = "pages/users.html")]
struct UsersPage {
    list: Option<Html>,
}

pub(super) async fn page(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
) -> Response {
    let list = if s.auth_enabled {
        if user.role != Role::Admin {
            return forbidden();
        }
        let st = s.clone();
        let rows = blocking(move || Some(list(&st))).await.unwrap_or_default();
        Some(user_list(&rows, user.user_id))
    } else {
        None
    };
    let inner = UsersPage { list }.html();
    respond(&s, &headers, &user, "Users", &inner)
}

/// Datastar posts its signals as JSON: `newuser` and `newrole`.
pub(super) async fn create(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    body: Bytes,
) -> Response {
    if !usable(&s, &user) {
        return forbidden();
    }
    let signals = serde_json::from_slice::<serde_json::Value>(&body).unwrap_or_default();
    let field = |k: &str| {
        signals
            .get(k)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_owned()
    };
    let username = field("newuser");
    let role: Role = field("newrole").parse().unwrap_or(Role::Readonly);
    let Some(server) = s.server_url.clone() else {
        return events(vec![failure(
            "The server's address is unknown: set server.url.",
        )]);
    };
    let me = user.user_id;
    let made = blocking(move || {
        let db = open(&s.pool)?;
        let made = invite::create_account(&db.conn, &username, role).and_then(|made| {
            token_invite(&s, &server, made.id, username.trim(), Some(&made.password))
        });
        Some((made, list(&s)))
    })
    .await;
    let Some((made, rows)) = made else {
        return events(vec![failure("The account could not be made.")]);
    };
    match made {
        Ok(i) => events(vec![
            result(&invite_panel(&i)),
            patch(&user_list(&rows, me), None),
            Event::default()
                .event("datastar-patch-signals")
                .data("signals {\"newuser\":\"\"}"),
        ]),
        Err(e) => events(vec![failure(&e.to_string())]),
    }
}

#[derive(serde::Deserialize)]
pub(super) struct InviteQuery {
    #[serde(default)]
    reset: bool,
}

pub(super) async fn invite(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
    Query(q): Query<InviteQuery>,
) -> Response {
    if !usable(&s, &user) {
        return forbidden();
    }
    let Some(server) = s.server_url.clone() else {
        return events(vec![failure(
            "The server's address is unknown: set server.url.",
        )]);
    };
    let made = blocking(move || {
        let db = open(&s.pool)?;
        let row = users::get_user_by_id(&db.conn, id).ok()??;
        let password = if q.reset {
            match invite::set_password(&db.conn, &row.username, None) {
                Ok(p) => {
                    crate::clients::registry().disconnect(&row.username);
                    Some(p)
                }
                Err(e) => return Some(Err(e)),
            }
        } else {
            None
        };
        Some(token_invite(
            &s,
            &server,
            row.id,
            &row.username,
            password.as_deref(),
        ))
    })
    .await;
    match made {
        Some(Ok(i)) => events(vec![result(&invite_panel(&i))]),
        Some(Err(e)) => events(vec![failure(&e.to_string())]),
        None => events(vec![failure("No such account.")]),
    }
}

fn clear_password_signal() -> Event {
    Event::default()
        .event("datastar-patch-signals")
        .data("signals {\"setpassword\":\"\"}")
}

#[derive(Template)]
#[template(path = "users/password.html")]
struct PasswordForm<'a> {
    id: i64,
    name: &'a str,
}

/// The form asking for an account's new password. Its own route, so a value
/// typed into one account's form and never submitted cannot reach another's:
/// Datastar posts every signal with every request.
pub(super) async fn password_form(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
) -> Response {
    if !usable(&s, &user) {
        return forbidden();
    }
    let row = blocking(move || users::get_user_by_id(&open(&s.pool)?.conn, id).ok()?).await;
    let Some(row) = row else {
        return events(vec![failure("No such account.")]);
    };
    let form = PasswordForm {
        id,
        name: &row.username,
    };
    events(vec![clear_password_signal(), result(&form.html())])
}

/// Set the password the form posts as its `setpassword` signal.
pub(super) async fn set_password(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
    body: Bytes,
) -> Response {
    if !usable(&s, &user) {
        return forbidden();
    }
    let password = serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v.get("setpassword")?.as_str().map(str::to_owned))
        .unwrap_or_default();
    let done = blocking(move || {
        let db = open(&s.pool)?;
        let row = users::get_user_by_id(&db.conn, id).ok()??;
        let done = invite::set_password(&db.conn, &row.username, Some(&password));
        if done.is_ok() {
            crate::clients::registry().disconnect(&row.username);
        }
        Some((row, done))
    })
    .await;
    match done {
        Some((row, Ok(_))) => events(vec![
            message(
                &format!(
                    "{}'s password is changed, and their devices are signed out.",
                    row.username
                ),
                false,
            ),
            clear_password_signal(),
        ]),
        Some((_, Err(e))) => events(vec![failure(&e.to_string())]),
        None => events(vec![failure("No such account.")]),
    }
}

#[derive(serde::Deserialize)]
pub(super) struct RoleQuery {
    role: String,
}

pub(super) async fn set_role(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
    Query(q): Query<RoleQuery>,
) -> Response {
    if !usable(&s, &user) {
        return forbidden();
    }
    let Ok(role) = q.role.parse::<Role>() else {
        return events(vec![failure("Unknown access level.")]);
    };
    let me = user.user_id;
    let done = blocking(move || {
        let db = open(&s.pool)?;
        let row = users::get_user_by_id(&db.conn, id).ok()??;
        let done = invite::set_role(&db.conn, &row.username, role);
        Some((done, list(&s)))
    })
    .await;
    match done {
        Some((Ok(()), rows)) => events(vec![patch(&user_list(&rows, me), None), result(&NOTHING)]),
        // Redraw the list too, so the select goes back to what is stored.
        Some((Err(e), rows)) => events(vec![
            patch(&user_list(&rows, me), None),
            failure(&e.to_string()),
        ]),
        None => events(vec![failure("No such account.")]),
    }
}

pub(super) async fn delete(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
) -> Response {
    if !usable(&s, &user) {
        return forbidden();
    }
    if id == user.user_id {
        return events(vec![failure("You cannot delete your own account here.")]);
    }
    let me = user.user_id;
    let done = blocking(move || {
        let db = open(&s.pool)?;
        let row = users::get_user_by_id(&db.conn, id).ok()??;
        let done = invite::delete_account(&db.conn, &row.username);
        if done.is_ok() {
            crate::clients::registry().disconnect(&row.username);
        }
        Some((done, list(&s)))
    })
    .await;
    match done {
        Some((Ok(()), rows)) => events(vec![patch(&user_list(&rows, me), None), result(&NOTHING)]),
        Some((Err(e), _)) => events(vec![failure(&e.to_string())]),
        None => events(vec![failure("No such account.")]),
    }
}
