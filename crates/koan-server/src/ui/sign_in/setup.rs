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

use askama::Template;
use koan_kit_web as kit;

use super::{cross_site, framed, same_origin};
use crate::auth::routes::{ClientIp, SignedIn, authenticate};
use crate::html::Render;
use crate::share::blocking;
use crate::ui::{UiState, capitalised, open, paths, see_other};

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
pub(in crate::ui) async fn open_for_setup(s: &UiState) -> Option<SetupCode> {
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
pub(in crate::ui) struct SetupForm {
    #[serde(default)]
    code: String,
    username: String,
    password: String,
    confirm: String,
}

/// The setup page: the setup code from the server's log, and the first
/// admin's name and password, chosen here.
#[derive(Template)]
#[template(path = "sign_in/setup.html")]
struct Setup<'a> {
    code: &'a str,
    username: &'a str,
    error: Option<&'a str>,
}

impl Setup<'_> {
    fn form(&self) -> kit::Form<'static> {
        kit::Form::plain(paths::setup())
    }

    fn fields(&self) -> [kit::Field<'static>; 4] {
        let plain = |name: &'static str| {
            kit::Input::text(name)
                .attr("autocapitalize", "none")
                .attr("spellcheck", "false")
                .required()
        };
        let password = |name: &'static str| {
            kit::Input::password(name)
                .attr("minlength", "8")
                .autocomplete("new-password")
                .required()
        };
        [
            kit::Field::new(
                "setup code",
                plain("code")
                    .value(self.code.to_owned())
                    .autocomplete("one-time-code")
                    .autofocus(),
            ),
            kit::Field::new(
                "username",
                plain("username")
                    .value(self.username.to_owned())
                    .autocomplete("username"),
            ),
            kit::Field::new("password", password("password")),
            kit::Field::new("confirm password", password("confirm")),
        ]
    }
}

fn page(
    s: &UiState,
    status: StatusCode,
    username: &str,
    code: &str,
    error: Option<&str>,
) -> Response {
    let view = Setup {
        code,
        username,
        error,
    };
    framed(s, status, "Set up", view.html(), false)
}

pub(in crate::ui) async fn form(State(s): State<UiState>) -> Response {
    if open_for_setup(&s).await.is_none() {
        return see_other(paths::LOGIN);
    }
    page(&s, StatusCode::OK, "", "", None)
}

pub(in crate::ui) async fn create(
    State(s): State<UiState>,
    ClientIp(from): ClientIp,
    headers: HeaderMap,
    Form(f): Form<SetupForm>,
) -> Response {
    if !same_origin(&headers) {
        return cross_site();
    }
    let Some(code) = open_for_setup(&s).await else {
        return see_other(paths::LOGIN);
    };
    let again = |status, why: &str| page(&s, status, &f.username, &f.code, Some(why));
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
        Some(Err(AccountError::AlreadySetUp)) => return see_other(paths::LOGIN),
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
            [(header::LOCATION, paths::HOME)],
            s.auth.session_cookies(&access, &refresh),
        )
            .into_response(),
        // Made, but not signed in: the sign-in form will do it.
        Ok(SignedIn::NeedsSecond { .. }) | Err(_) => see_other(paths::LOGIN),
    }
}

#[cfg(test)]
mod tests {
    use crate::ui::fixture::*;

    #[tokio::test]
    async fn an_empty_server_sends_sign_in_to_setup_which_makes_the_admin_once() {
        let (_dir, app, code) = unset(true);
        let code = code.unwrap();
        let r = send(&app, get("/login").body(Body::empty()).unwrap()).await;
        assert_eq!((r.status, r.location()), (StatusCode::SEE_OTHER, "/setup"));
        let r = send(&app, get("/setup").body(Body::empty()).unwrap()).await;
        assert_eq!(r.status, StatusCode::OK);
        assert!(r.body.contains("action=\"setup\""));
        assert!(r.body.contains("name=\"code\""));

        let mismatch = send(
            &app,
            form(
                "/setup",
                &format!("code={code}&username=ada&password=longenough&confirm=different"),
            ),
        )
        .await;
        assert_eq!(mismatch.status, StatusCode::BAD_REQUEST);
        assert!(mismatch.body.contains("do not match"));
        let short = send(
            &app,
            form(
                "/setup",
                &format!("code={code}&username=ada&password=short&confirm=short"),
            ),
        )
        .await;
        assert_eq!(short.status, StatusCode::BAD_REQUEST);
        assert!(short.body.contains("at least 8"));

        let cross = Request::post("/setup")
            .header(header::HOST, HOST)
            .header(header::ORIGIN, "https://evil.test")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(format!(
                "code={code}&username=eve&password=longenough&confirm=longenough"
            )))
            .unwrap();
        assert_eq!(send(&app, cross).await.status, StatusCode::FORBIDDEN);

        // Reaching the page first is not enough: without the code from the log,
        // nobody is made admin.
        for guess in [
            "username=eve&password=longenough&confirm=longenough",
            "code=&username=eve&password=longenough&confirm=longenough",
            "code=aaaa-bbbb-cccc-dddd&username=eve&password=longenough&confirm=longenough",
        ] {
            let r = send(&app, form("/setup", guess)).await;
            assert_eq!(r.status, StatusCode::FORBIDDEN, "{guess}");
            assert!(r.body.contains("not the setup code"), "{guess}");
            assert!(r.cookies().is_empty(), "{guess}");
        }
        let r = send(&app, get("/setup").body(Body::empty()).unwrap()).await;
        assert_eq!(r.status, StatusCode::OK);

        // However it is typed back.
        let typed = code.to_uppercase().replace('-', "+");
        let made = send(
            &app,
            form(
                "/setup",
                &format!("code={typed}&username=ada&password=longenough&confirm=longenough"),
            ),
        )
        .await;
        assert_eq!((made.status, made.location()), (StatusCode::SEE_OTHER, "/"));
        let access = made.cookie("koan_access");
        let home = send(
            &app,
            get("/")
                .header(header::COOKIE, format!("koan_access={access}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(home.status, StatusCode::OK);

        // Closed for good: the page, a second admin, and sign-in is sign-in again.
        let r = send(&app, get("/setup").body(Body::empty()).unwrap()).await;
        assert_eq!((r.status, r.location()), (StatusCode::SEE_OTHER, "/login"));
        let again = send(
            &app,
            form(
                "/setup",
                &format!("code={code}&username=eve&password=longenough&confirm=longenough"),
            ),
        )
        .await;
        assert_eq!(
            (again.status, again.location()),
            (StatusCode::SEE_OTHER, "/login")
        );
        assert!(again.cookies().is_empty());
        let r = send(&app, get("/login").body(Body::empty()).unwrap()).await;
        assert_eq!(r.status, StatusCode::OK);
    }

    #[tokio::test]
    async fn setup_turned_off_leaves_an_empty_server_to_the_cli() {
        let (_dir, app, code) = unset(false);
        assert_eq!(code, None);
        let r = send(&app, get("/login").body(Body::empty()).unwrap()).await;
        assert_eq!(r.status, StatusCode::OK);
        let r = send(&app, get("/setup").body(Body::empty()).unwrap()).await;
        assert_eq!((r.status, r.location()), (StatusCode::SEE_OTHER, "/login"));
        let r = send(
            &app,
            form(
                "/setup",
                "username=ada&password=longenough&confirm=longenough",
            ),
        )
        .await;
        assert_eq!((r.status, r.location()), (StatusCode::SEE_OTHER, "/login"));
        assert!(r.cookies().is_empty());
    }
}
