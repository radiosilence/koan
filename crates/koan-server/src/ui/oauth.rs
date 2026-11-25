//! koan as an OAuth 2.1 authorization server, so an MCP client can sign in to
//! `/mcp` with a koan account and nothing else in between. The protocol is
//! `koan_auth::oauth`'s; the routes, the consent page and the tokens a code is
//! worth are kōan's.
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

use askama::Template;
use axum::Extension;
use axum::Form;
use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use koan_auth::oauth::{Authorizing, Exchange, OAuthError, Registration, Server, TokenRequest};
use koan_core::auth;
use koan_core::db::queries::auth as auth_queries;

use super::{Logged, UiState, html, open};
use crate::auth::AuthUser;
use crate::html::Render;
use crate::share::not_found;

pub(super) use koan_auth::oauth::{
    AuthorizeRequest as AuthorizeParams, Codes, MAX_REGISTRATION_BODY,
};

/// The path, under this server's origin, of `/mcp`'s resource metadata.
pub const RESOURCE_METADATA: &str = "/.well-known/oauth-protected-resource/mcp";

/// This server as the authorization server for `/mcp`. Its address is
/// `server.url`, which OAuth needs a fixed one of; without one, none of this
/// is served.
fn server(s: &UiState) -> Option<Server> {
    Server::new(
        s.server_url.as_deref()?,
        "/mcp",
        "kōan",
        s.redirect_hosts.to_vec(),
    )
}

/// Where `/mcp` is, for the page explaining how to connect an assistant.
pub(super) fn mcp_url(s: &UiState) -> Option<String> {
    server(s).map(|server| server.resource().to_owned())
}

pub(super) async fn protected_resource(State(s): State<UiState>) -> Response {
    match server(&s) {
        Some(server) => Json(server.protected_resource_metadata()).into_response(),
        None => not_found(),
    }
}

pub(super) async fn authorization_server(State(s): State<UiState>) -> Response {
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

pub(super) async fn register(State(s): State<UiState>, Json(r): Json<Registration>) -> Response {
    let Some(server) = server(&s) else {
        return not_found();
    };
    match server.register(&s.auth.private_pem, r) {
        Ok(body) => (StatusCode::CREATED, Json(body)).into_response(),
        Err(e) if e.error == "server_error" => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        Err(e) => oauth_error(StatusCode::BAD_REQUEST, &e),
    }
}

/// An authorization request this server will answer, or the page saying why
/// not: never a redirect until the client and its redirect URI hold, or this
/// would bounce users to any address.
fn check(s: &UiState, q: &AuthorizeParams) -> Result<(Server, Authorizing), Box<Response>> {
    let server = server(s).ok_or_else(|| Box::new(not_found()))?;
    match server.check(&s.auth.public_pem, q) {
        Ok(authorizing) => Ok((server, authorizing)),
        Err(why) => {
            let page = Refused {
                base: s.base(),
                title: "Cannot connect",
                why,
            };
            Err(Box::new(html(StatusCode::BAD_REQUEST, page.html())))
        }
    }
}

/// Send the browser back to the client with `pairs`.
fn answer(
    server: &Server,
    to: &Authorizing,
    q: &AuthorizeParams,
    pairs: &[(&str, &str)],
) -> Response {
    (
        StatusCode::SEE_OTHER,
        [
            (header::LOCATION, server.answer(&to.to, q, pairs)),
            (header::CACHE_CONTROL, "no-store".into()),
        ],
    )
        .into_response()
}

#[derive(Template)]
#[template(path = "oauth/refused.html")]
struct Refused<'a> {
    base: String,
    title: &'a str,
    why: &'a str,
}

/// The consent page. `fields` carries the request on to the approval, and
/// `here` brings the person back to it after signing in as someone else.
#[derive(Template)]
#[template(path = "oauth/consent.html")]
struct Consent<'a> {
    base: String,
    title: &'a str,
    host: &'a str,
    name: &'a str,
    abilities: &'a str,
    user: &'a str,
    fields: [(&'a str, &'a str); 8],
    here: String,
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

pub(super) async fn authorize(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<AuthorizeParams>,
) -> Response {
    let (server, to) = match check(&s, &q) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if let Some((error, why)) = server.request_error(&q) {
        return answer(
            &server,
            &to,
            &q,
            &[("error", error), ("error_description", why)],
        );
    }
    let page = Consent {
        base: s.base(),
        title: "Connect an app",
        host: to.to.host_str().unwrap_or_default(),
        name: &to.client.client_name,
        abilities: abilities(&user),
        user: &user.username,
        fields: q.fields(),
        here: format!("/oauth/authorize?{}", q.query()),
    };
    let mut resp = html(StatusCode::OK, page.html());
    // The approval answers with a redirect to the client, which a browser
    // holds the form's `form-action` to as well.
    let csp = super::PAGE_CSP.replace(
        "form-action 'self'",
        &format!(
            "form-action 'self' {}",
            to.to.origin().ascii_serialization()
        ),
    );
    if let Ok(v) = HeaderValue::from_str(&csp) {
        resp.headers_mut()
            .insert(header::CONTENT_SECURITY_POLICY, v);
    }
    resp
}

pub(super) async fn approve(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
    Form(q): Form<AuthorizeParams>,
) -> Response {
    if !super::session::same_origin(&headers) {
        return (StatusCode::FORBIDDEN, "cross-site request refused").into_response();
    }
    let (server, to) = match check(&s, &q) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if let Some((error, why)) = server.request_error(&q) {
        return answer(
            &server,
            &to,
            &q,
            &[("error", error), ("error_description", why)],
        );
    }
    if q.decision != "allow" {
        return answer(&server, &to, &q, &[("error", "access_denied")]);
    }
    let Ok(code) = s.codes.issue(&q, &to, user.user_id) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    answer(&server, &to, &q, &[("code", &code)])
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

pub(super) async fn token(State(s): State<UiState>, Form(t): Form<TokenRequest>) -> Response {
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
