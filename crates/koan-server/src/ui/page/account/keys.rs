//! API keys: what a Subsonic client that supports them signs in with in place
//! of the account's password.

use axum::Extension;
use axum::extract::{Path, State};
use axum::response::Response;
use koan_core::db::queries::api_keys;
use koan_kit_web as kit;

use super::{Credential, Credentials, MAX_NAME, NO_ACCOUNTS, Named, name};
use crate::auth::AuthUser;
use crate::html::{Html, Render};
use crate::share::blocking;
use crate::ui::{Logged, UiState, flash_bad, open, paths};

const ID: &str = "keys";

/// The section, as `user`'s keys stand, with `made` shown this once.
pub(in crate::ui) fn section(
    conn: &rusqlite::Connection,
    user: i64,
    made: Option<String>,
) -> Option<Html> {
    let keys = api_keys::list_api_keys(conn, Some(user)).logged()?;
    Some(
        Credentials {
            id: ID,
            heading: "API keys",
            about: "A Subsonic client that supports API keys can sign in with one instead of your password. It acts as you, with your permissions, until you revoke it.",
            what: "key",
            form: kit::Form::post(paths::keys()),
            field: kit::Input::text("name")
                .placeholder("Name, e.g. phone")
                .label("Key name")
                .attr("maxlength", MAX_NAME.to_string())
                .autocomplete("off")
                .required()
                .class("flex-1"),
            create: "Create key",
            rows: keys
                .iter()
                .map(|k| Credential::new(k.id, &k.name, k.created_at, k.last_used_at))
                .collect(),
            empty: "No keys yet.",
            made,
            revoke: paths::key_revoke,
            question: "Revoke this key? Clients using it will stop working.",
        }
        .html(),
    )
}

/// The section replaced whole, so the form's typed name goes.
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
    let Some(name) = name(&f.name) else {
        return flash_bad("Give the key a name of up to 100 characters.");
    };
    let html = blocking(move || {
        let db = open(&s.pool)?;
        let (_, key) = api_keys::create_api_key(&db.conn, user.user_id, &name).logged()?;
        section(&db.conn, user.user_id, Some(key))
    })
    .await;
    match html {
        Some(html) => replaced(html),
        None => flash_bad("The key could not be made."),
    }
}

pub(in crate::ui) async fn revoke(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
) -> Response {
    let html = blocking(move || {
        let db = open(&s.pool)?;
        api_keys::revoke_api_key(&db.conn, id, Some(user.user_id)).logged()?;
        section(&db.conn, user.user_id, None)
    })
    .await;
    match html {
        Some(html) => replaced(html),
        None => flash_bad("The key could not be revoked."),
    }
}

#[cfg(test)]
mod tests {
    use crate::ui::fixture::*;

    #[tokio::test]
    async fn api_keys_are_shown_once_listed_and_revoked() {
        let f = setup(true);
        let post = |uri: &str, body: &str, datastar: bool| {
            let mut req = Request::post(uri)
                .header(header::HOST, HOST)
                .header(
                    header::COOKIE,
                    format!("koan_access={}", access_token(&f.state)),
                )
                .header(header::CONTENT_TYPE, "application/json");
            if datastar {
                req = req.header("datastar-request", "true");
            }
            req.body(Body::from(body.to_owned())).unwrap()
        };

        let r = send(
            &f.app,
            authed(&f.state, "/account").body(Body::empty()).unwrap(),
        )
        .await;
        assert_eq!(r.status, StatusCode::OK);
        assert!(r.body.contains("No keys yet."), "{}", r.body);

        let r = send(
            &f.app,
            post("/account/keys", r#"{"keyname":"phone"}"#, false),
        )
        .await;
        assert_eq!(r.status, StatusCode::FORBIDDEN);

        let r = send(
            &f.app,
            post("/account/keys", r#"{"keyname":"phone"}"#, true),
        )
        .await;
        let key = r
            .body
            .split("id=\"key-result-value\" readonly value=\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .unwrap_or_else(|| panic!("no key in {}", r.body))
            .to_owned();
        let db = Database::open(f.state.pool.path()).unwrap();
        let user = queries::api_keys::authenticate_api_key(&db.conn, &key)
            .unwrap()
            .unwrap();
        assert_eq!(user.username, "alice");

        let r = send(
            &f.app,
            authed(&f.state, "/account").body(Body::empty()).unwrap(),
        )
        .await;
        assert!(
            r.body.contains("phone") && r.body.contains("last used"),
            "{}",
            r.body
        );
        assert!(!r.body.contains(&key));

        let id = queries::api_keys::list_api_keys(&db.conn, Some(user.id)).unwrap()[0].id;
        let r = send(
            &f.app,
            post(&format!("/account/keys/{id}/revoke"), "{}", true),
        )
        .await;
        assert!(r.body.contains("No keys yet."), "{}", r.body);
        assert!(
            queries::api_keys::authenticate_api_key(&db.conn, &key)
                .unwrap()
                .is_none()
        );
    }
}
