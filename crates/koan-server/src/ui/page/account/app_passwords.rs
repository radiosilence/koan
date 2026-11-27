//! App passwords: what a Subsonic app that only signs in with a username and
//! password is given in place of the account's own.

use axum::Extension;
use axum::extract::{Path, State};
use axum::response::Response;
use koan_core::db::queries::app_passwords;
use koan_kit_web as kit;

use super::{Credential, Credentials, MAX_NAME, NO_ACCOUNTS, Named, name};
use crate::auth::AuthUser;
use crate::html::{Html, Render};
use crate::share::blocking;
use crate::ui::{Logged, UiState, flash_bad, open, paths};

const ID: &str = "app-passwords";

/// The section, as `user`'s app passwords stand, with `made` shown this once.
pub(in crate::ui) fn section(
    conn: &rusqlite::Connection,
    user: i64,
    made: Option<String>,
) -> Option<Html> {
    let passwords = app_passwords::list_app_passwords(conn, user).logged()?;
    Some(
        Credentials {
            id: ID,
            heading: "App passwords",
            about: "For a Subsonic app that only signs in with a username and password: give it your username and an app password in place of yours. Make one per app. kōan keeps it encrypted, never shows it again, and it stops working when you revoke it or change your password.",
            what: "app password",
            form: kit::Form::post(paths::app_passwords()),
            field: kit::Input::text("name")
                .placeholder("Name, e.g. Arpeggi")
                .label("App password name")
                .attr("maxlength", MAX_NAME.to_string())
                .autocomplete("off")
                .required()
                .class("flex-1"),
            create: "Create app password",
            rows: passwords
                .iter()
                .map(|p| Credential::new(p.id, &p.name, p.created_at, p.last_used_at))
                .collect(),
            empty: "No app passwords yet.",
            made,
            revoke: paths::app_password_revoke,
            question: "Revoke this app password? The app using it will stop working.",
        }
        .html(),
    )
}

fn replaced(html: Html) -> Response {
    askama_datastar::events([
        html.patch()
            .mode(askama_datastar::ElementPatchMode::Replace)
            .into(),
        kit::Confirm::close().into(),
    ])
}

pub(in crate::ui) async fn create(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    axum::Form(f): axum::Form<Named>,
) -> Response {
    if !s.auth_enabled {
        return flash_bad(NO_ACCOUNTS);
    }
    let Some(name) = name(&f.name).and_then(|n| app_passwords::app_password_name(&n)) else {
        return flash_bad("Give the app password a name of up to 100 characters.");
    };
    let key = koan_core::auth::app_password_key(&s.auth.private_pem);
    let made = blocking(move || {
        let db = open(&s.pool)?;
        let password = match app_passwords::create_app_password(&db.conn, &key, user.user_id, &name)
        {
            Ok((_, password)) => password,
            Err(e @ app_passwords::CreateAppPasswordError::TooMany) => {
                return Some(Err(e.to_string()));
            }
            Err(_) => return None,
        };
        section(&db.conn, user.user_id, Some(password)).map(Ok)
    })
    .await;
    match made {
        Some(Ok(html)) => replaced(html),
        Some(Err(reason)) => flash_bad(&reason),
        None => flash_bad("The app password could not be made."),
    }
}

pub(in crate::ui) async fn revoke(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
) -> Response {
    let html = blocking(move || {
        let db = open(&s.pool)?;
        app_passwords::revoke_app_password(&db.conn, id, user.user_id).logged()?;
        section(&db.conn, user.user_id, None)
    })
    .await;
    match html {
        Some(html) => replaced(html),
        None => flash_bad("The app password could not be revoked."),
    }
}

#[cfg(test)]
mod tests {
    use crate::ui::fixture::*;

#[tokio::test]
async fn app_passwords_are_shown_once_sealed_and_revoked() {
    let f = setup(true);
    let post = |uri: &str, body: &str| {
        Request::post(uri)
            .header(header::HOST, HOST)
            .header(
                header::COOKIE,
                format!("koan_access={}", access_token(&f.state)),
            )
            .header(header::CONTENT_TYPE, "application/json")
            .header("datastar-request", "true")
            .body(Body::from(body.to_owned()))
            .unwrap()
    };
    let r = send(
        &f.app,
        authed(&f.state, "/account").body(Body::empty()).unwrap(),
    )
    .await;
    assert!(r.body.contains("No app passwords yet."), "{}", r.body);

    let r = send(
        &f.app,
        post("/account/app-passwords", r#"{"appname":"Arpeggi"}"#),
    )
    .await;
    let password = r
        .body
        .split("id=\"app-password-result-value\" readonly value=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap_or_else(|| panic!("no app password in {}", r.body))
        .to_owned();

    // Sealed in the database, never kept or listed as itself.
    let db = Database::open(f.state.pool.path()).unwrap();
    let stored: Vec<u8> = db
        .conn
        .query_row("SELECT sealed FROM app_passwords", [], |row| row.get(0))
        .unwrap();
    assert!(
        !stored
            .windows(password.len())
            .any(|w| w == password.as_bytes())
    );
    let r = send(
        &f.app,
        authed(&f.state, "/account").body(Body::empty()).unwrap(),
    )
    .await;
    assert!(r.body.contains("Arpeggi") && !r.body.contains(&password));

    let key = koan_core::auth::app_password_key(&f.state.private_pem);
    let signed_in = |db: &Database| {
        matches!(
            queries::app_passwords::authenticate_app_password(&db.conn, &key, "alice", |p| {
                p == password
            })
            .unwrap(),
            queries::app_passwords::AppPasswordAuth::Matched(_)
        )
    };
    assert!(signed_in(&db));

    let user = queries::auth::get_user_by_username(&db.conn, "alice")
        .unwrap()
        .unwrap();
    let id = queries::app_passwords::list_app_passwords(&db.conn, user.id).unwrap()[0].id;
    let r = send(
        &f.app,
        post(&format!("/account/app-passwords/{id}/revoke"), "{}"),
    )
    .await;
    assert!(r.body.contains("No app passwords yet."), "{}", r.body);
    assert!(!signed_in(&db));
}
}
