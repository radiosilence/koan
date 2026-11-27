//! koan as an OAuth 2.1 authorization server, so an MCP client can sign in to
//! `/mcp` with a koan account and nothing else in between. The protocol is
//! `koan_auth::oauth`'s; the routes and the tokens a code is worth are kōan's,
//! and the consent page is `sign_in::consent`.
//!
//! The client finds this server from the `/mcp` 401 (RFC 9728), reads its
//! metadata (RFC 8414), registers itself (RFC 7591), and sends its user here to
//! sign in to the web UI and approve. The code it gets back is traded, with
//! PKCE, for koan's own access and refresh tokens: the ones the apps and the
//! web UI hold, so `/mcp` checks them the same way and acts as that account,
//! and refreshing is koan's own rotation.
//!
//! Registration is open to any client and keeps no state. The client id is a
//! JWT signed with the server's key and carrying the client's redirect URIs, so
//! a registration survives a restart without a table of clients, and a key
//! rotation sends every client back to register. Since anyone may register
//! under any name, the consent page names a client by the host it returns to.
//! Codes live in memory for a minute and are spent once.
//!
//! Every address here is `server.url`: an issuer taken from request
//! headers would let a client choose it. Without one, none of this is served.

use std::sync::Arc;

use axum::Form;
use axum::Json;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::middleware::from_fn_with_state;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use koan_auth::oauth::{Exchange, OAuthError, Registration, Server, TokenRequest};
use koan_core::auth;
use koan_core::db::queries::auth as auth_queries;

use super::{Logged, UiState, open, paths};
use crate::auth::AuthUser;
use crate::auth::routes::{RateLimiter, rate_limit};
use crate::share::not_found;

pub(super) use koan_auth::oauth::{
    AuthorizeRequest as AuthorizeParams, Codes, MAX_REGISTRATION_BODY,
};

/// The path, under this server's origin, of `/mcp`'s resource metadata.
pub const RESOURCE_METADATA: &str = "/.well-known/oauth-protected-resource/mcp";

/// This server as the authorization server for `/mcp`. Its address is
/// `server.url`, which OAuth needs a fixed one of; without one, none of this
/// is served.
pub(super) fn server(s: &UiState) -> Option<Server> {
    Server::new(
        s.server_url.as_deref()?,
        "/mcp",
        "kōan",
        s.redirect_hosts.to_vec(),
    )
}

/// The protocol's routes: discovery, registration and tokens. Registering
/// and token requests are unauthenticated, so each is capped per address.
pub(super) fn routes(base: &str) -> axum::Router<UiState> {
    let router = axum::Router::new()
        .route(paths::PROTECTED_RESOURCE, get(protected_resource))
        .route(RESOURCE_METADATA, get(protected_resource))
        .route(paths::AUTHORIZATION_SERVER, get(authorization_server))
        // Registering stores nothing, but signs a client id each time.
        .route(
            paths::REGISTER,
            post(register)
                .layer(axum::extract::DefaultBodyLimit::max(MAX_REGISTRATION_BODY))
                .layer(from_fn_with_state(
                    Arc::new(RateLimiter::new(3600, 10)),
                    rate_limit,
                )),
        )
        .route(
            paths::TOKEN,
            post(token).layer(from_fn_with_state(
                Arc::new(RateLimiter::new(60, 60)),
                rate_limit,
            )),
        );
    router.merge(path_inserted_discovery(base))
}

/// Under a path, where RFC 8414 and RFC 9728 put the metadata: between the
/// host and the path (`/.well-known/oauth-authorization-server/koan`), outside
/// what a proxy forwards for the path. It reaches here when the proxy forwards
/// that too; see `crate::base`.
fn path_inserted_discovery(base: &str) -> axum::Router<UiState> {
    if base.is_empty() || crate::base::problem(base).is_some() {
        return axum::Router::new();
    }
    axum::Router::new()
        .route(
            &format!("{}{base}", paths::AUTHORIZATION_SERVER),
            get(authorization_server),
        )
        .route(
            &format!("{}{base}/mcp", paths::PROTECTED_RESOURCE),
            get(protected_resource),
        )
}

/// Where `/mcp` is, for the page explaining how to connect an assistant.
pub(super) fn mcp_url(s: &UiState) -> Option<String> {
    server(s).map(|server| server.resource().to_owned())
}

async fn protected_resource(State(s): State<UiState>) -> Response {
    match server(&s) {
        Some(server) => Json(server.protected_resource_metadata()).into_response(),
        None => not_found(),
    }
}

async fn authorization_server(State(s): State<UiState>) -> Response {
    match server(&s) {
        Some(server) => Json(server.authorization_server_metadata()).into_response(),
        None => not_found(),
    }
}

fn oauth_error(status: StatusCode, error: &OAuthError) -> Response {
    (
        status,
        [(header::CACHE_CONTROL, "no-store")],
        Json(error.body()),
    )
        .into_response()
}

async fn register(State(s): State<UiState>, Json(r): Json<Registration>) -> Response {
    let Some(server) = server(&s) else {
        return not_found();
    };
    match server.register(&s.auth.private_pem, r) {
        Ok(body) => (StatusCode::CREATED, Json(body)).into_response(),
        Err(e) if e.error == "server_error" => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        Err(e) => oauth_error(StatusCode::BAD_REQUEST, &e),
    }
}

/// What a grant lets the client do, at the role `/mcp` caps it to.
pub(super) fn abilities(user: &AuthUser) -> &'static str {
    match crate::mcp::capped(user.role) {
        auth::Role::Readonly => "browse and search your library, and see what is playing",
        auth::Role::User => {
            "search and change your library and playlists, and play music on your devices"
        }
        auth::Role::Admin => {
            "do anything your account can, including moving files and changing settings"
        }
    }
}

fn tokens(s: &UiState, access: String, refresh: String) -> Response {
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(koan_auth::oauth::tokens(
            &access,
            &refresh,
            s.auth.access_ttl_secs,
        )),
    )
        .into_response()
}

async fn token(State(s): State<UiState>, Form(t): Form<TokenRequest>) -> Response {
    let Some(server) = server(&s) else {
        return not_found();
    };
    let invalid = |why: &'static str| {
        oauth_error(
            StatusCode::BAD_REQUEST,
            &OAuthError::new("invalid_grant", why),
        )
    };
    // Clients re-register on `invalid_client`, which is what a key rotation
    // needs of them.
    if (t.grant_type == "authorization_code" || !t.client_id.is_empty())
        && server.client(&s.auth.public_pem, &t.client_id).is_none()
    {
        return oauth_error(
            StatusCode::UNAUTHORIZED,
            &OAuthError::new("invalid_client", "unknown client; register again"),
        );
    }
    if !server.resource_ok(&t.resource) {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            &OAuthError::new("invalid_target", "the only resource here is /mcp"),
        );
    }
    match t.grant_type.as_str() {
        "authorization_code" => {
            let (user_id, grant) = match s.codes.exchange(&t) {
                Exchange::Issue(user_id, grant) => (user_id, grant),
                Exchange::Replayed(grant) => {
                    // OAuth 2.1 §4.1.3: a code used twice was intercepted, and
                    // what was issued for it may be in the wrong hands.
                    let pool = s.pool.clone();
                    let _ = tokio::task::spawn_blocking(move || {
                        let db = open(&pool)?;
                        auth_queries::revoke_grant(&db.conn, &grant).logged()
                    })
                    .await;
                    log::warn!("an OAuth code was presented twice: revoked its grant");
                    return invalid("code already used");
                }
                Exchange::Refused => return invalid("unknown or expired code"),
            };
            let st = s.clone();
            let issued = tokio::task::spawn_blocking(move || {
                let db = open(&st.pool)?;
                let user = auth_queries::get_user_by_id(&db.conn, user_id).logged()??;
                let access = auth::mint_scoped_token(
                    &st.auth.private_pem,
                    user.id,
                    &user.username,
                    user.role,
                    st.auth.access_ttl_secs,
                    Some(auth::MCP_SCOPE),
                )
                .logged()?;
                let refresh = auth::random_token().logged()?;
                let expires = koan_core::clock::unix_secs() + st.auth.refresh_ttl_secs as i64;
                auth_queries::store_grant_token(&db.conn, &refresh, user.id, expires, Some(&grant))
                    .logged()?;
                Some((access, refresh))
            })
            .await
            .ok()
            .flatten();
            match issued {
                Some((access, refresh)) => tokens(&s, access, refresh),
                None => invalid("the account is gone"),
            }
        }
        "refresh_token" => {
            let auth = s.auth.clone();
            let rt = t.refresh_token;
            let rotated = tokio::task::spawn_blocking(move || {
                crate::auth::routes::rotate(&auth, &rt, None).ok()
            })
            .await
            .ok()
            .flatten();
            match rotated {
                Some((access, refresh)) => tokens(&s, access, refresh),
                None => invalid("unknown or expired refresh token"),
            }
        }
        _ => oauth_error(
            StatusCode::BAD_REQUEST,
            &OAuthError::new(
                "unsupported_grant_type",
                "authorization_code or refresh_token",
            ),
        ),
    }
}
