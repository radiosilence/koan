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
