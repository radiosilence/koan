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

#[cfg(test)]
mod tests {
    use crate::ui::fixture::*;

    mod oauth {
        use super::*;
        use base64::Engine as _;
        use sha2::{Digest, Sha256};

        const CALLBACK: &str = "https://claude.ai/api/mcp/auth_callback";
        const VERIFIER: &str = "a-verifier-of-at-least-forty-three-characters-long";

        fn challenge() -> String {
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(VERIFIER))
        }

        fn query(pairs: &[(&str, &str)]) -> String {
            form_urlencoded::Serializer::new(String::new())
                .extend_pairs(pairs)
                .finish()
        }

        fn json(r: &Reply) -> serde_json::Value {
            serde_json::from_str(&r.body).unwrap_or_else(|_| panic!("not JSON: {}", r.body))
        }

        async fn register(f: &Fixture, redirect: &str) -> Reply {
            send(
                &f.app,
                Request::post("/oauth/register")
                    .header(header::HOST, HOST)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({ "redirect_uris": [redirect], "client_name": "Claude" })
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
        }

        fn authorize_params<'a>(client_id: &'a str, challenge: &'a str) -> Vec<(&'a str, &'a str)> {
            vec![
                ("response_type", "code"),
                ("client_id", client_id),
                ("redirect_uri", CALLBACK),
                ("code_challenge", challenge),
                ("code_challenge_method", "S256"),
                ("state", "xyz"),
                ("resource", "https://koan.test/mcp"),
            ]
        }

        /// Approve as alice; the reply is the redirect to the client.
        async fn approve(f: &Fixture, params: &[(&str, &str)], origin: &str) -> Reply {
            let mut params = params.to_vec();
            params.push(("decision", "allow"));
            send(
                &f.app,
                Request::post("/oauth/authorize")
                    .header(header::HOST, HOST)
                    .header(header::ORIGIN, origin)
                    .header(
                        header::COOKIE,
                        format!("koan_access={}", access_token(&f.state)),
                    )
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(query(&params)))
                    .unwrap(),
            )
            .await
        }

        fn code_of(location: &str) -> String {
            let url = reqwest::Url::parse(location).unwrap();
            let get = |k: &str| {
                url.query_pairs()
                    .find(|(n, _)| n == k)
                    .map(|(_, v)| v.into_owned())
            };
            assert_eq!(get("state").as_deref(), Some("xyz"));
            assert_eq!(get("iss").as_deref(), Some(ORIGIN));
            get("code").expect("a code")
        }

        async fn token(f: &Fixture, pairs: &[(&str, &str)]) -> Reply {
            let mut req = form("/oauth/token", &query(pairs));
            req.headers_mut().remove(header::ORIGIN);
            send(&f.app, req).await
        }

        #[tokio::test]
        async fn an_mcp_client_signs_in_and_refreshes() {
            let f = setup_at(true, Some("https://koan.test/"));
            let meta = send(
                &f.app,
                get("/.well-known/oauth-authorization-server")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
            assert_eq!(json(&meta)["issuer"], ORIGIN);
            let resource = send(
                &f.app,
                get(crate::ui::RESOURCE_METADATA)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
            assert_eq!(json(&resource)["resource"], "https://koan.test/mcp");

            let reg = register(&f, CALLBACK).await;
            assert_eq!(reg.status, StatusCode::CREATED);
            let client_id = json(&reg)["client_id"].as_str().unwrap().to_owned();
            let challenge = challenge();
            let params = authorize_params(&client_id, &challenge);
            let uri = format!("/oauth/authorize?{}", query(&params));

            // Signed out: through sign-in and back.
            let r = send(&f.app, get(&uri).body(Body::empty()).unwrap()).await;
            assert_eq!(r.status, StatusCode::SEE_OTHER);
            assert!(
                r.location()
                    .starts_with("/auth/resume?next=%2Foauth%2Fauthorize")
            );

            let r = send(&f.app, authed(&f.state, &uri).body(Body::empty()).unwrap()).await;
            assert_eq!(r.status, StatusCode::OK);
            assert!(
                r.body
                    .contains("Connect kōan to <span class=\"normal-case\">claude.ai</span>?"),
                "{}",
                r.body
            );
            let csp = r.headers[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
            assert!(csp.contains("form-action 'self' https://claude.ai"));
            assert!(csp.contains("frame-ancestors 'none'"));

            let r = approve(&f, &params, ORIGIN).await;
            assert_eq!(r.status, StatusCode::SEE_OTHER);
            assert!(r.location().starts_with(CALLBACK));
            let code = code_of(r.location());

            let exchange = [
                ("grant_type", "authorization_code"),
                ("code", code.as_str()),
                ("redirect_uri", CALLBACK),
                ("client_id", client_id.as_str()),
                ("code_verifier", VERIFIER),
                ("resource", "https://koan.test/mcp"),
            ];
            let r = token(&f, &exchange).await;
            assert_eq!(r.status, StatusCode::OK, "{}", r.body);
            let t = json(&r);
            // Good at /mcp and nowhere else.
            let access = t["access_token"].as_str().unwrap();
            let claims =
                auth::validate_scoped_token(&f.state.public_pem, access, Some(auth::MCP_SCOPE))
                    .unwrap();
            assert_eq!(claims.username, "alice");
            assert!(auth::validate_access_token(&f.state.public_pem, access).is_err());
            let r = send(
                &f.app,
                get("/albums")
                    .header(header::COOKIE, format!("koan_access={access}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
            assert_ne!(r.status, StatusCode::OK);
            let refresh = t["refresh_token"].as_str().unwrap().to_owned();

            let r = token(
                &f,
                &[
                    ("grant_type", "refresh_token"),
                    ("refresh_token", refresh.as_str()),
                    ("client_id", client_id.as_str()),
                ],
            )
            .await;
            assert_eq!(r.status, StatusCode::OK, "{}", r.body);
            let refreshed = json(&r)["access_token"].as_str().unwrap().to_owned();
            assert!(auth::validate_access_token(&f.state.public_pem, &refreshed).is_err());
            let rotated = json(&r)["refresh_token"].as_str().unwrap().to_owned();

            // The code again: refused, and the grant it made is revoked.
            let r = token(&f, &exchange).await;
            assert_eq!(r.status, StatusCode::BAD_REQUEST);
            let r = token(
                &f,
                &[
                    ("grant_type", "refresh_token"),
                    ("refresh_token", rotated.as_str()),
                ],
            )
            .await;
            assert_eq!(r.status, StatusCode::BAD_REQUEST);
        }

        #[tokio::test]
        async fn a_redirect_uri_is_matched_as_registered_and_may_be_left_out() {
            let f = setup_at(true, Some(ORIGIN));
            // Re-serialised, this gains a trailing slash.
            let registered = "http://localhost:33418";
            let reg = register(&f, registered).await;
            let client_id = json(&reg)["client_id"].as_str().unwrap().to_owned();
            let challenge = challenge();
            for given in ["", registered] {
                let mut params = authorize_params(&client_id, &challenge);
                params[2] = ("redirect_uri", given);
                let uri = format!("/oauth/authorize?{}", query(&params));
                let r = send(&f.app, authed(&f.state, &uri).body(Body::empty()).unwrap()).await;
                assert_eq!(r.status, StatusCode::OK, "{}", r.body);
                let r = approve(&f, &params, ORIGIN).await;
                assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
                let code = code_of(r.location());
                let r = token(
                    &f,
                    &[
                        ("grant_type", "authorization_code"),
                        ("code", code.as_str()),
                        ("redirect_uri", given),
                        ("client_id", client_id.as_str()),
                        ("code_verifier", VERIFIER),
                    ],
                )
                .await;
                assert_eq!(r.status, StatusCode::OK, "{given:?}: {}", r.body);
            }
        }

        #[tokio::test]
        async fn requests_that_do_not_hold_are_refused() {
            let f = setup_at(true, Some(ORIGIN));
            assert_eq!(
                register(&f, "http://evil.example/cb").await.status,
                StatusCode::BAD_REQUEST
            );
            let client_id = json(&register(&f, CALLBACK).await)["client_id"]
                .as_str()
                .unwrap()
                .to_owned();
            let challenge = challenge();

            // Not this client's redirect, or not a client: a page, never a redirect.
            let mut params = authorize_params(&client_id, &challenge);
            params[2] = ("redirect_uri", "https://evil.example/cb");
            let uri = format!("/oauth/authorize?{}", query(&params));
            let r = send(&f.app, authed(&f.state, &uri).body(Body::empty()).unwrap()).await;
            assert_eq!(r.status, StatusCode::BAD_REQUEST);
            assert!(!r.headers.contains_key(header::LOCATION));
            let uri = format!(
                "/oauth/authorize?{}",
                query(&authorize_params("forged", &challenge))
            );
            let r = send(&f.app, authed(&f.state, &uri).body(Body::empty()).unwrap()).await;
            assert_eq!(r.status, StatusCode::BAD_REQUEST);

            // No PKCE: back to the client with an error.
            let mut params = authorize_params(&client_id, &challenge);
            params[4] = ("code_challenge_method", "plain");
            let uri = format!("/oauth/authorize?{}", query(&params));
            let r = send(&f.app, authed(&f.state, &uri).body(Body::empty()).unwrap()).await;
            assert!(r.location().contains("error=invalid_request"));

            // Another site's form.
            let params = authorize_params(&client_id, &challenge);
            let r = approve(&f, &params, "https://evil.example").await;
            assert_eq!(r.status, StatusCode::FORBIDDEN);

            // A wrong verifier.
            let code = code_of(approve(&f, &params, ORIGIN).await.location());
            let r = token(
                &f,
                &[
                    ("grant_type", "authorization_code"),
                    ("code", code.as_str()),
                    ("redirect_uri", CALLBACK),
                    ("client_id", client_id.as_str()),
                    ("code_verifier", "not-the-verifier"),
                ],
            )
            .await;
            assert_eq!(json(&r)["error"], "invalid_grant");

            // An unknown client is told to register again.
            let r = token(
                &f,
                &[
                    ("grant_type", "authorization_code"),
                    ("code", code.as_str()),
                    ("client_id", "forged"),
                ],
            )
            .await;
            assert_eq!(r.status, StatusCode::UNAUTHORIZED);
            assert_eq!(json(&r)["error"], "invalid_client");
        }

        #[tokio::test]
        async fn the_assistants_page_gives_the_address_once_there_is_one() {
            let f = setup_at(true, Some(ORIGIN));
            let r = send(
                &f.app,
                authed(&f.state, "/connect").body(Body::empty()).unwrap(),
            )
            .await;
            assert_eq!(r.status, StatusCode::OK);
            assert!(
                r.body.contains("value=\"https://koan.test/mcp\""),
                "{}",
                r.body
            );
            let f = setup(true);
            let r = send(
                &f.app,
                authed(&f.state, "/connect").body(Body::empty()).unwrap(),
            )
            .await;
            assert!(
                r.body.contains("needs to know its own address"),
                "{}",
                r.body
            );
        }

        #[tokio::test]
        async fn without_a_server_url_there_is_no_oauth() {
            let f = setup(true);
            for uri in [
                "/.well-known/oauth-authorization-server",
                crate::ui::RESOURCE_METADATA,
            ] {
                let r = send(&f.app, get(uri).body(Body::empty()).unwrap()).await;
                assert_eq!(r.status, StatusCode::NOT_FOUND);
            }
            assert_eq!(register(&f, CALLBACK).await.status, StatusCode::NOT_FOUND);
        }

        #[tokio::test]
        async fn under_a_path_the_metadata_is_also_where_rfc_8414_puts_it() {
            let f = setup_at(true, Some("https://koan.test/koan"));
            for uri in [
                "/.well-known/oauth-authorization-server",
                "/.well-known/oauth-authorization-server/koan",
            ] {
                let r = send(&f.app, get(uri).body(Body::empty()).unwrap()).await;
                assert_eq!(json(&r)["issuer"], "https://koan.test/koan", "{uri}");
                assert_eq!(
                    json(&r)["token_endpoint"],
                    "https://koan.test/koan/oauth/token"
                );
            }
            let r = send(
                &f.app,
                get("/.well-known/oauth-protected-resource/koan/mcp")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
            assert_eq!(json(&r)["resource"], "https://koan.test/koan/mcp");
        }
    }
}
