//! The first admin, made in the browser: a server with auth on and no account
//! yet answers `/setup` with a form, so one in a container needs no shell to
//! become usable. Whoever reaches it first becomes the admin, which is why it
//! closes for good the moment any account exists, and why `graphql.setup_wizard`
//! turns it off for anyone who would rather not have that window at all.

use axum::Form;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use koan_core::db::queries::auth as auth_queries;
use koan_core::invite::{self, AccountError};

use super::{UiState, html, open, pages, see_other};
use crate::auth::routes::{ClientIp, authenticate};
use crate::share::blocking;

/// Whether the setup page is open: auth on, the wizard allowed, and no account.
/// A database that cannot be read is not an empty one.
pub(super) async fn open_for_setup(s: &UiState) -> bool {
    if !s.auth_enabled || !s.setup_wizard {
        return false;
    }
    let pool = s.pool.clone();
    blocking(move || {
        let db = open(&pool)?;
        auth_queries::has_users(&db.conn).ok().map(|has| !has)
    })
    .await
    .unwrap_or(false)
}

#[derive(Deserialize)]
pub(super) struct SetupForm {
    username: String,
    password: String,
    confirm: String,
}

pub(super) async fn form(State(s): State<UiState>) -> Response {
    if !open_for_setup(&s).await {
        return see_other("/login");
    }
    html(StatusCode::OK, pages::setup("", None))
}

pub(super) async fn create(
    State(s): State<UiState>,
    ClientIp(from): ClientIp,
    headers: HeaderMap,
    Form(f): Form<SetupForm>,
) -> Response {
    if !super::session::same_origin(&headers) {
        return (StatusCode::FORBIDDEN, "cross-site request refused").into_response();
    }
    if !open_for_setup(&s).await {
        return see_other("/login");
    }
    if f.password != f.confirm {
        return html(
            StatusCode::BAD_REQUEST,
            pages::setup(&f.username, Some("The passwords do not match.")),
        );
    }
    let pool = s.pool.clone();
    let (username, password) = (f.username.clone(), f.password.clone());
    let made = blocking(move || {
        let db = open(&pool)?;
        Some(invite::create_first_admin(&db.conn, &username, &password))
    })
    .await;
    let username = match made {
        Some(Ok(_)) => f.username.trim(),
        // Someone else finished first: theirs is the server now.
        Some(Err(AccountError::AlreadySetUp)) => return see_other("/login"),
        Some(Err(e)) => {
            return html(
                StatusCode::BAD_REQUEST,
                pages::setup(&f.username, Some(&capitalised(&e.to_string()))),
            );
        }
        None => {
            return html(
                StatusCode::INTERNAL_SERVER_ERROR,
                pages::setup(&f.username, Some("The account could not be made.")),
            );
        }
    };
    log::warn!("web UI: {username:?} made the first admin account through /setup, from {from}");
    match authenticate(&s.auth, username, &f.password, from).await {
        Ok((_, access, refresh)) => (
            StatusCode::SEE_OTHER,
            [(header::LOCATION, "/")],
            s.auth.session_cookies(&access, &refresh),
        )
            .into_response(),
        // Made, but not signed in: the sign-in form will do it.
        Err(_) => see_other("/login"),
    }
}

/// An `AccountError`, which reads as a clause, as a sentence.
fn capitalised(message: &str) -> String {
    let mut chars = message.chars();
    chars.next().map_or_else(String::new, |c| {
        format!("{}{}.", c.to_uppercase(), chars.as_str())
    })
}
