//! The first admin, made in the browser: a server with auth on and no account
//! yet answers `/setup` with a form, so one in Docker needs no shell to
//! become usable. The form asks for a code the server writes to its log, so
//! the admin is whoever runs the server rather than whoever reaches the page
//! first: a database can be empty because a volume did not mount, not only on
//! a new install. The page closes for good the moment any account exists, and
//! `graphql.setup_wizard` turns it off for anyone who would rather make the
//! admin with `koan auth setup`.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Form;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use koan_auth::setup::SetupCode;
use serde::Deserialize;

use koan_core::db::queries::auth as auth_queries;
use koan_core::invite::{self, AccountError};

use super::{UiState, html, open, pages, see_other};
use crate::auth::routes::{ClientIp, SignedIn, authenticate};
use crate::share::blocking;

/// How often the code is written to the log again while the page is in use,
/// so it is still there after the log has been rotated.
const LOG_AGAIN: Duration = Duration::from_secs(60);

/// The code `/setup` asks for: made the first time it is needed, and written
/// to the log, where only the server's operator looks.
#[derive(Clone, Default)]
pub struct SetupGate(Arc<Mutex<Option<(SetupCode, Instant)>>>);

impl SetupGate {
    /// The code, made the first time it is asked for, and logged then and
    /// at most once a minute after.
    pub fn code(&self) -> Option<SetupCode> {
        let mut held = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let code = match held.take() {
            Some((code, logged)) if logged.elapsed() < LOG_AGAIN => {
                *held = Some((code.clone(), logged));
                return Some(code);
            }
            Some((code, _)) => code,
            None => SetupCode::new()
                .map_err(|e| log::error!("the setup code could not be made: {e}"))
                .ok()?,
        };
        log::warn!(
            "No accounts yet: open /setup and enter the setup code {} to make the first \
             admin. Set graphql.setup_wizard = false to make it with `koan auth setup` instead.",
            code.as_str()
        );
        *held = Some((code.clone(), Instant::now()));
        Some(code)
    }
}

/// The code the setup page asks for, while the page is open: auth on and the
/// wizard allowed (a gate given), and no account. A database that cannot be
/// read is not an empty one.
pub(super) async fn open_for_setup(s: &UiState) -> Option<SetupCode> {
    let gate = s.setup.clone()?;
    let pool = s.pool.clone();
    let empty = blocking(move || {
        let db = open(&pool)?;
        auth_queries::has_users(&db.conn).ok().map(|has| !has)
    })
    .await
    .unwrap_or(false);
    empty.then(|| gate.code()).flatten()
}

#[derive(Deserialize)]
pub(super) struct SetupForm {
    #[serde(default)]
    code: String,
    username: String,
    password: String,
    confirm: String,
}

pub(super) async fn form(State(s): State<UiState>) -> Response {
    if open_for_setup(&s).await.is_none() {
        return see_other("/login");
    }
    html(StatusCode::OK, pages::setup(&s, "", "", None))
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
    let Some(code) = open_for_setup(&s).await else {
        return see_other("/login");
    };
    let again = |status, why: &str| html(status, pages::setup(&s, &f.username, &f.code, Some(why)));
    if !code.matches(&f.code) {
        log::warn!("web UI: a wrong setup code was entered at /setup, from {from}");
        return again(
            StatusCode::FORBIDDEN,
            "That is not the setup code. The server writes it to its log while this page is open.",
        );
    }
    if f.password != f.confirm {
        return again(StatusCode::BAD_REQUEST, "The passwords do not match.");
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
        Some(Err(e)) => return again(StatusCode::BAD_REQUEST, &capitalised(&e.to_string())),
        None => {
            return again(
                StatusCode::INTERNAL_SERVER_ERROR,
                "The account could not be made.",
            );
        }
    };
    log::warn!("web UI: {username:?} made the first admin account through /setup, from {from}");
    match authenticate(&s.auth, username, &f.password, None, from).await {
        Ok(SignedIn::Session(_, access, refresh)) => (
            StatusCode::SEE_OTHER,
            [(header::LOCATION, "/")],
            s.auth.session_cookies(&access, &refresh),
        )
            .into_response(),
        // Made, but not signed in: the sign-in form will do it.
        Ok(SignedIn::NeedsSecond { .. }) | Err(_) => see_other("/login"),
    }
}

/// An `AccountError`, which reads as a clause, as a sentence.
fn capitalised(message: &str) -> String {
    let mut chars = message.chars();
    chars.next().map_or_else(String::new, |c| {
        format!("{}{}.", c.to_uppercase(), chars.as_str())
    })
}
