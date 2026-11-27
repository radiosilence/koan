//! Two-step sign-in on the Account page: setting up an authenticator app or a
//! security key, new recovery codes, and removing either. The rules are
//! `koan_auth::second_factor`'s; this draws them.
//!
//! The first factor added ends every session, key and app password the
//! account had, this browser's session included, so the response that adds it
//! carries a new session for this browser, which has just shown both. Removing
//! the last one ends the sessions too, and likewise signs this browser in
//! again.

use askama::Template;
use axum::Extension;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use koan_auth::second_factor::{self as factor, Added, Factors, Purpose, SecondFactorError};
use koan_auth::webauthn::{self, Registration};
use koan_kit_web as kit;
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::html::{Html, Render};
use crate::share::blocking;
use crate::ui::{Logged, Sent, UiState, capitalised, flash_bad, open, patch, paths};

/// The section's region, which every action here redraws.
const ID: &str = "two-step";

const REMOVE_TOTP: &str = "Remove the authenticator app?";
const REMOVE_KEY: &str = "Remove this security key?";
const REMOVE_LAST: &str = "This is your last second factor: removing it turns two-step sign-in off and signs out your other browsers.";
const NEW_CODES: &str = "Make new recovery codes? The ones you have stop working.";

/// An authenticator app being set up: its QR code, the secret to type
/// instead, grouped in fours as apps show it.
struct Setup {
    qr: Option<kit::Qr<'static>>,
    secret: String,
}

struct Key {
    id: i64,
    name: String,
    created: String,
    used: Option<String>,
}

/// The Account page's two-step section, redrawn whole by every action here.
#[derive(Template)]
#[template(path = "account/two_step.html")]
struct Section<'a> {
    factors: Factors,
    keys: Vec<Key>,
    setup: Option<Setup>,
    /// Security keys need `server.url` at an address browsers allow them on.
    keys_possible: bool,
    /// Recovery codes just made, shown this once.
    codes: Option<Vec<String>>,
    error: Option<&'a str>,
}

impl Section<'_> {
    fn confirm_totp_removal(&self) -> &'static str {
        if self.factors.keys.is_empty() {
            REMOVE_LAST
        } else {
            REMOVE_TOTP
        }
    }

    fn confirm_key_removal(&self) -> &'static str {
        if !self.factors.totp && self.factors.keys.len() == 1 {
            REMOVE_LAST
        } else {
            REMOVE_KEY
        }
    }

    fn id(&self) -> &'static str {
        ID
    }

    fn begin_totp(&self) -> kit::Button<'static> {
        kit::Button::standard("Set up an authenticator app").post(paths::totp())
    }

    fn remove_totp(&self) -> kit::Button<'static> {
        kit::Button::quiet("Remove")
            .bad()
            .post(paths::totp_remove())
            .confirm(self.confirm_totp_removal(), "Remove")
    }

    fn confirm_form(&self) -> kit::Form<'static> {
        kit::Form::post(paths::totp_confirm())
    }

    fn code(&self) -> kit::Input<'static> {
        kit::Input::text("code")
            .placeholder("Code from the app")
            .label("Code from the app")
            .attr("inputmode", "numeric")
            .attr("maxlength", "6")
            .autocomplete("one-time-code")
            .required()
            .class("flex-1")
    }

    fn remove_key(&self, k: &Key) -> kit::Button<'static> {
        kit::Button::quiet("Remove")
            .bad()
            .post(paths::security_key_remove(k.id))
            .confirm(self.confirm_key_removal(), "Remove")
    }

    fn key_name(&self) -> kit::Input<'static> {
        kit::Input::text("securitykey")
            .placeholder("Name, e.g. iPhone or YubiKey")
            .label("Passkey or security key name")
            .attr("maxlength", koan_auth::second_factor::MAX_NAME.to_string())
            .autocomplete("off")
            .required()
            .class("flex-1")
    }

    fn begin_key(&self) -> &'static str {
        paths::security_keys_begin()
    }

    fn finish_key(&self) -> &'static str {
        paths::security_keys()
    }

    fn new_codes(&self) -> kit::Button<'static> {
        kit::Button::standard("Make new codes")
            .post(paths::recovery_codes())
            .confirm(NEW_CODES, "Make new codes")
    }

    fn key_detail(&self, k: &Key) -> String {
        match &k.used {
            Some(used) => format!("Added {} · last used {used}", k.created),
            None => format!("Added {} · never used", k.created),
        }
    }
}

fn day(t: i64) -> String {
    crate::ui::page::scrobbling::day(t)
}

/// The section as `user_id`'s factors stand, with what has just happened.
fn section(
    s: &UiState,
    conn: &rusqlite::Connection,
    user_id: i64,
    codes: Option<Vec<String>>,
    error: Option<&str>,
) -> Option<Html> {
    let factors = factor::factors(conn, user_id).logged()?;
    let key = factor::totp_key(&s.auth.private_pem);
    let setup = if factors.totp {
        None
    } else {
        factor::pending_totp(conn, &key, user_id)
            .logged()?
            .map(|secret| setup(s, conn, user_id, &secret))
    };
    let keys = factors
        .keys
        .iter()
        .map(|k| Key {
            id: k.id,
            name: k.name.clone(),
            created: day(k.created_at),
            used: k.last_used_at.map(day),
        })
        .collect();
    Some(
        Section {
            factors,
            keys,
            setup,
            keys_possible: s.relying_party().is_some(),
            codes,
            error,
        }
        .html(),
    )
}

/// The section for the Account page.
pub(in crate::ui) fn for_page(
    s: &UiState,
    conn: &rusqlite::Connection,
    user_id: i64,
) -> Option<Html> {
    section(s, conn, user_id, None, None)
}

fn setup(s: &UiState, conn: &rusqlite::Connection, user_id: i64, secret: &[u8]) -> Setup {
    let username = koan_core::db::queries::auth::get_user_by_id(conn, user_id)
        .logged()
        .flatten()
        .map(|u| u.username)
        .unwrap_or_default();
    let uri = koan_auth::totp::key_uri(&issuer(s), &username, secret);
    let plain = koan_auth::totp::base32(secret);
    let secret = plain
        .as_bytes()
        .chunks(4)
        .map(|c| std::str::from_utf8(c).unwrap_or_default())
        .collect::<Vec<_>>()
        .join(" ");
    Setup {
        qr: kit::Qr::new(&uri).map(|q| q.label("Authenticator app setup as a QR code")),
        secret,
    }
}

/// How an authenticator app lists this server: kōan, and the host when there
/// is one, so that two servers' entries can be told apart.
fn issuer(s: &UiState) -> String {
    s.server_url
        .as_deref()
        .and_then(|u| reqwest::Url::parse(u).ok())
        .and_then(|u| u.host_str().map(str::to_owned))
        .map_or_else(|| "kōan".to_owned(), |host| format!("kōan ({host})"))
}

/// Why the caller may not change their second factor, if they may not: a
/// server without accounts, or the demo's shared account, which a visitor
/// would otherwise lock everyone out of.
fn refused(s: &UiState, user: &AuthUser) -> Option<&'static str> {
    if !s.auth_enabled {
        return Some("This server runs without sign-in.");
    }
    if crate::demo::enabled() && user.username == crate::demo::USERNAME {
        return Some("The demo's shared account cannot have two-step sign-in.");
    }
    None
}

/// The section as a patch, with a new session for this browser when the
/// change ended the account's sessions.
async fn answer(s: &UiState, user: &AuthUser, html: Option<Html>, resign: bool) -> Response {
    let Some(html) = html else {
        return flash_bad("That did not work. Reload the page and try again.");
    };
    let events = askama_datastar::events([patch(&html), kit::Confirm::close().into()]);
    if !resign {
        return events;
    }
    match crate::auth::routes::session_for(&s.auth, user.user_id).await {
        Some((access, refresh)) => {
            (s.auth.session_cookies(&access, &refresh), events).into_response()
        }
        None => events,
    }
}

/// Draw the section again, with `error` in it.
async fn with_error(s: UiState, user: &AuthUser, error: &'static str) -> Response {
    let id = user.user_id;
    let st = s.clone();
    let html = blocking(move || section(&st, &open(&st.pool)?.conn, id, None, Some(error))).await;
    answer(&s, user, html, false).await
}

pub(in crate::ui) async fn begin_totp(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
) -> Response {
    if let Some(why) = refused(&s, &user) {
        return with_error(s, &user, why).await;
    }
    let (st, id) = (s.clone(), user.user_id);
    let html = blocking(move || {
        let db = open(&st.pool)?;
        let key = factor::totp_key(&st.auth.private_pem);
        match factor::begin_totp(&db.conn, &key, id) {
            Ok(_) => section(&st, &db.conn, id, None, None),
            Err(SecondFactorError::AlreadyOn) => section(
                &st,
                &db.conn,
                id,
                None,
                Some("The authenticator app is already set up."),
            ),
            Err(e) => {
                log::error!("web UI: setting up an authenticator app: {e}");
                None
            }
        }
    })
    .await;
    answer(&s, &user, html, false).await
}

#[derive(Deserialize)]
pub(in crate::ui) struct Code {
    code: String,
}

/// Confirm the authenticator app with the code the form posts.
pub(in crate::ui) async fn confirm_totp(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    axum::Form(Code { code }): axum::Form<Code>,
) -> Response {
    if let Some(why) = refused(&s, &user) {
        return with_error(s, &user, why).await;
    }
    if s.auth.users.codes_spent(&user.username) {
        return with_error(s, &user, "Too many wrong codes. Try again in a minute.").await;
    }
    let (st, id, name) = (s.clone(), user.user_id, user.username.clone());
    let done = blocking(move || {
        let db = open(&st.pool)?;
        let key = factor::totp_key(&st.auth.private_pem);
        Some(match factor::confirm_totp(&db.conn, &key, id, &code) {
            Ok(Added { recovery_codes }) => {
                let first = recovery_codes.is_some();
                (section(&st, &db.conn, id, recovery_codes, None), first)
            }
            Err(e @ (SecondFactorError::WrongCode | SecondFactorError::NothingPending)) => {
                if matches!(e, SecondFactorError::WrongCode) {
                    st.auth.users.code_failed(&name);
                }
                let message = match e {
                    SecondFactorError::WrongCode => {
                        "That code is not right. Check the app's clock, and try the next code."
                    }
                    _ => "Nothing is waiting to be confirmed. Start again.",
                };
                (section(&st, &db.conn, id, None, Some(message)), false)
            }
            Err(e) => {
                log::error!("web UI: confirming an authenticator app: {e}");
                (None, false)
            }
        })
    })
    .await;
    let (html, first) = done.unwrap_or((None, false));
    if first {
        crate::clients::registry().disconnect(&user.username);
    }
    answer(&s, &user, html, first).await
}

pub(in crate::ui) async fn remove_totp(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
) -> Response {
    if let Some(why) = refused(&s, &user) {
        return with_error(s, &user, why).await;
    }
    let (st, id) = (s.clone(), user.user_id);
    let done = blocking(move || {
        let db = open(&st.pool)?;
        let was = factor::enabled(&db.conn, id).logged()?;
        factor::remove_totp(&db.conn, id).logged()?;
        let off = was && !factor::enabled(&db.conn, id).logged()?;
        Some((section(&st, &db.conn, id, None, None), off))
    })
    .await;
    let (html, off) = done.unwrap_or((None, false));
    answer(&s, &user, html, off).await
}

pub(in crate::ui) async fn remove_key(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Path(key): Path<i64>,
) -> Response {
    if let Some(why) = refused(&s, &user) {
        return with_error(s, &user, why).await;
    }
    let (st, id) = (s.clone(), user.user_id);
    let done = blocking(move || {
        let db = open(&st.pool)?;
        let was = factor::enabled(&db.conn, id).logged()?;
        factor::remove_key(&db.conn, id, key).logged()?;
        let off = was && !factor::enabled(&db.conn, id).logged()?;
        Some((section(&st, &db.conn, id, None, None), off))
    })
    .await;
    let (html, off) = done.unwrap_or((None, false));
    answer(&s, &user, html, off).await
}

pub(in crate::ui) async fn new_recovery_codes(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
) -> Response {
    if let Some(why) = refused(&s, &user) {
        return with_error(s, &user, why).await;
    }
    let (st, id) = (s.clone(), user.user_id);
    let html = blocking(move || {
        let db = open(&st.pool)?;
        if !factor::enabled(&db.conn, id).logged()? {
            return section(
                &st,
                &db.conn,
                id,
                None,
                Some("Turn two-step sign-in on first."),
            );
        }
        let codes = factor::replace_recovery_codes(&db.conn, id).logged()?;
        section(&st, &db.conn, id, Some(codes), None)
    })
    .await;
    answer(&s, &user, html, false).await
}

fn json_error(status: StatusCode, message: &str) -> Response {
    (
        status,
        axum::Json(serde_json::json!({ "message": message })),
    )
        .into_response()
}

#[derive(Deserialize)]
pub(in crate::ui) struct BeginKey {
    name: String,
}

/// Start registering a security key: a ticket and `navigator.credentials
/// .create`'s options, for the page's script.
pub(in crate::ui) async fn begin_key(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    axum::Json(req): axum::Json<BeginKey>,
) -> Response {
    if let Some(why) = refused(&s, &user) {
        return json_error(StatusCode::FORBIDDEN, why);
    }
    let Some(rp) = s.relying_party() else {
        return json_error(
            StatusCode::CONFLICT,
            "Security keys need the server's address (server.url) to be https://.",
        );
    };
    if factor::key_name(&req.name).is_none() {
        return json_error(
            StatusCode::BAD_REQUEST,
            "Give the key a name of up to 100 characters.",
        );
    }
    let Ok((ticket, held)) =
        s.auth
            .users
            .registrations()
            .open(user.user_id, &user.username, Purpose::Register)
    else {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, "That did not work.");
    };
    let (st, id) = (s.clone(), user.user_id);
    let exclude = blocking(move || factor::key_ids(&open(&st.pool)?.conn, id).logged())
        .await
        .unwrap_or_default();
    let options = webauthn::creation_options(
        &rp,
        &factor::user_handle(user.user_id),
        &user.username,
        &held.challenge,
        &exclude,
    );
    axum::Json(serde_json::json!({ "ticket": ticket, "options": options })).into_response()
}

#[derive(Deserialize)]
pub(in crate::ui) struct FinishKey {
    ticket: String,
    name: String,
    registration: Registration,
}

/// Register the security key the page's script made, which it sends through
/// Datastar, and answer with the section redrawn.
pub(in crate::ui) async fn finish_key(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Sent(req): Sent<FinishKey>,
) -> Response {
    if let Some(why) = refused(&s, &user) {
        return flash_bad(why);
    }
    let tickets = s.auth.users.registrations();
    let held = tickets
        .get(&req.ticket, Purpose::Register)
        .filter(|t| t.user_id == user.user_id);
    tickets.close(&req.ticket);
    let (Some(held), Some(rp)) = (held, s.relying_party()) else {
        return flash_bad("That took too long. Try again.");
    };
    let Some(name) = factor::key_name(&req.name) else {
        return flash_bad("Give the key a name of up to 100 characters.");
    };
    let credential = match webauthn::register(&rp, &held.challenge, &req.registration) {
        Ok(c) => c,
        Err(e) => return flash_bad(&capitalised(&e.to_string())),
    };
    let (st, id) = (s.clone(), user.user_id);
    let done = blocking(move || {
        let db = open(&st.pool)?;
        Some(match factor::add_key(&db.conn, id, &name, &credential) {
            Ok(Added { recovery_codes }) => {
                let first = recovery_codes.is_some();
                Ok((section(&st, &db.conn, id, recovery_codes, None), first))
            }
            Err(e @ (SecondFactorError::TooManyKeys | SecondFactorError::KeyTaken)) => {
                Err(capitalised(&e.to_string()))
            }
            Err(e) => {
                log::error!("web UI: registering a security key: {e}");
                Err("The key could not be registered.".to_owned())
            }
        })
    })
    .await;
    match done {
        Some(Ok((html, first))) => {
            if first {
                crate::clients::registry().disconnect(&user.username);
            }
            answer(&s, &user, html, first).await
        }
        Some(Err(message)) => flash_bad(&message),
        None => flash_bad("That did not work."),
    }
}
