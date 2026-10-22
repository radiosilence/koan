//! A public demo server (`server.demo`): one shared account that strangers
//! sign in to, and state wiped on a schedule by whatever runs the server.
//!
//! The account is an ordinary `user`, so playlists, favourites, shares,
//! invites to its own devices and pairing all work, and the wipe undoes them.
//! What one visitor could use to take the account from the others is refused
//! for everyone: changing its password, revoking its keys and app passwords
//! (every visitor's device holds one), and connecting a scrobbling account,
//! which would send every visitor's plays to one stranger. Account management
//! is refused too, so an admin made by mistake cannot open those again.

use std::sync::OnceLock;

use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// The account every visitor signs in as, named on the sign-in page.
pub(crate) const USERNAME: &str = "demo";
pub(crate) const PASSWORD: &str = "demo";

static ON: OnceLock<bool> = OnceLock::new();

/// Whether this server is a demo, as `start` recorded it.
pub(crate) fn enabled() -> bool {
    ON.get().copied().unwrap_or(false)
}

/// Make this server a demo: record it, and make the shared account if it is
/// missing, as it is after every wipe.
pub(crate) fn start(conn: &rusqlite::Connection) -> Result<(), String> {
    use koan_core::db::queries::auth as users;
    let _ = ON.set(true);
    if users::get_user_by_username(conn, USERNAME)
        .map_err(|e| e.to_string())?
        .is_none()
    {
        users::create_user(conn, USERNAME, PASSWORD, koan_core::auth::Role::User)
            .map_err(|e| e.to_string())?;
        log::info!("Demo server: made the account {USERNAME}");
    }
    Ok(())
}

const MESSAGE: &str = "This is a demo server: its account, keys and scrobbling cannot be changed.";

/// Subsonic endpoints refused, without the `.view` some clients add.
const REST: &[&str] = &[
    "koanSetUserPassword",
    "koanRevokeApiKey",
    "koanRevokeAppPassword",
    "koanScrobblingConnect",
    "koanSetUserRole",
    "koanDeleteUser",
    "koanCreateUser",
];

/// Whether a demo server refuses a request to `path`.
fn refused(path: &str) -> bool {
    if let Some(method) = path.strip_prefix("/rest/") {
        return REST.contains(&method.trim_end_matches(".view"));
    }
    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
    matches!(
        parts.as_slice(),
        ["account", "keys" | "app-passwords", _, "revoke"]
            | ["scrobbling", "listenbrainz"]
            | ["scrobbling", "lastfm", "connect"]
            | ["users", _, "password" | "role" | "delete"]
            | ["users", _, "password", "form"]
    )
}

/// The layer `server.demo` puts in front of every route.
pub(crate) async fn guard(request: Request, next: Next) -> Response {
    let path = request.uri().path();
    if !refused(path) {
        return next.run(request).await;
    }
    if path.starts_with("/rest/") {
        let json = request.uri().query().is_some_and(|q| {
            form_urlencoded::parse(q.as_bytes()).any(|(k, v)| k == "f" && v == "json")
        });
        return crate::subsonic::not_authorized(json, MESSAGE);
    }
    (StatusCode::FORBIDDEN, MESSAGE).into_response()
}

#[cfg(test)]
mod tests {
    use super::refused;

    #[test]
    fn what_could_take_the_account_from_other_visitors_is_refused() {
        for path in [
            "/rest/koanSetUserPassword",
            "/rest/koanRevokeApiKey.view",
            "/rest/koanRevokeAppPassword",
            "/rest/koanScrobblingConnect",
            "/rest/koanDeleteUser",
            "/account/keys/3/revoke",
            "/account/app-passwords/3/revoke",
            "/scrobbling/listenbrainz",
            "/scrobbling/lastfm/connect",
            "/users/2/password",
            "/users/2/password/form",
            "/users/2/role",
            "/users/2/delete",
        ] {
            assert!(refused(path), "{path}");
        }
    }

    #[test]
    fn everything_else_goes_through() {
        for path in [
            "/rest/koanInvite",
            "/rest/koanPairApprove",
            "/rest/createPlaylist",
            "/rest/koanRevokeKey",
            "/rest/koanScrobbling",
            "/account",
            "/account/keys",
            "/scrobbling",
            "/pair/abc/approve",
            "/graphql",
        ] {
            assert!(!refused(path), "{path}");
        }
    }
}
