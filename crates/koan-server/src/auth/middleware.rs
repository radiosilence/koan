//! Axum middleware for JWT authentication.
//!
//! Extracts a token from the `koan_access` cookie or `Authorization: Bearer`,
//! validates it, and injects `AuthUser` into request extensions.

use std::sync::Arc;

use axum::extract::Request;
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use subtle::ConstantTimeEq;

use koan_core::auth::{self, Role};

use super::AuthUser;

/// Shared state for the auth middleware.
#[derive(Clone)]
pub struct AuthState {
    /// Ed25519 public key PEM for JWT verification.
    pub public_pem: Arc<Vec<u8>>,
    /// Whether auth is enforced.
    pub auth_enabled: bool,
    /// Process-scoped introspection key. Bypasses auth when matched.
    /// Generated randomly on server start, dies with the process.
    pub introspection_key: Option<Arc<String>>,
}

/// Axum middleware: validate JWT and inject `AuthUser`.
///
/// When `auth_enabled = false`, injects anonymous admin and passes through.
/// When `auth_enabled = true`, requires a valid token.
pub async fn auth_middleware(
    axum::extract::State(state): axum::extract::State<AuthState>,
    mut request: Request,
    next: Next,
) -> Response {
    if !state.auth_enabled {
        request.extensions_mut().insert(AuthUser::anonymous_admin());
        return next.run(request).await;
    }

    // Check for introspection key (playground bypass).
    if let Some(ref expected_key) = state.introspection_key
        && let Some(provided) = request
            .headers()
            .get("X-Introspection-Key")
            .and_then(|v| v.to_str().ok())
        && provided
            .as_bytes()
            .ct_eq(expected_key.as_bytes())
            .unwrap_u8()
            == 1
    {
        request.extensions_mut().insert(AuthUser::anonymous_admin());
        return next.run(request).await;
    }

    let Some(token) = extract_token(&request) else {
        return (
            StatusCode::UNAUTHORIZED,
            [("WWW-Authenticate", "Bearer")],
            "missing or invalid Authorization header",
        )
            .into_response();
    };

    match auth::validate_access_token(&state.public_pem, &token) {
        Ok(claims) => {
            let role = claims.role.parse().unwrap_or(Role::Readonly);
            let user = AuthUser {
                user_id: claims.sub,
                username: claims.username,
                role,
            };
            request.extensions_mut().insert(user);
            next.run(request).await
        }
        Err(_) => (
            StatusCode::UNAUTHORIZED,
            [("WWW-Authenticate", "Bearer")],
            "invalid or expired token",
        )
            .into_response(),
    }
}

/// Priority: `koan_access` cookie, then `Authorization: Bearer`, then `?token=`.
///
/// The query parameter is confined to the WebSocket route, which is the only one
/// that cannot carry a header. A token in a URL survives in shell history, proxy
/// logs and `Referer`.
fn extract_token(request: &Request) -> Option<String> {
    request
        .headers()
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|cookies| {
            cookies
                .split(';')
                .find_map(|c| c.trim().strip_prefix("koan_access=").map(String::from))
        })
        .or_else(|| {
            request
                .headers()
                .get(header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
                .map(String::from)
        })
        .or_else(|| {
            if request.uri().path() != "/graphql/ws" {
                return None;
            }
            request.uri().query().and_then(|q| {
                q.split('&')
                    .find_map(|pair| pair.strip_prefix("token=").map(String::from))
            })
        })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use axum::routing::get;
    use tower::ServiceExt as _;

    /// Echoes the `AuthUser` the middleware injected, so tests can assert on it.
    async fn echo_user(axum::Extension(user): axum::Extension<AuthUser>) -> String {
        format!("{}:{}", user.username, user.role.as_str())
    }

    async fn call(state: AuthState, req: HttpRequest<Body>) -> (StatusCode, String) {
        let app = axum::Router::new()
            .route("/graphql", get(echo_user))
            .route("/graphql/ws", get(echo_user))
            .layer(axum::middleware::from_fn_with_state(state, auth_middleware));
        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
            .await
            .unwrap();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    /// A live keypair plus a matching token for `role`.
    fn keys_and_token(role: Role) -> (Vec<u8>, String) {
        let (private_pem, public_pem) = auth::generate_keypair_pem().unwrap();
        let token = auth::mint_access_token(private_pem.as_bytes(), 7, "alice", role, 900).unwrap();
        (public_pem.into_bytes(), token)
    }

    fn enforcing(public_pem: Vec<u8>, key: Option<&str>) -> AuthState {
        AuthState {
            public_pem: Arc::new(public_pem),
            auth_enabled: true,
            introspection_key: key.map(|k| Arc::new(k.to_string())),
        }
    }

    #[tokio::test]
    async fn auth_disabled_grants_anonymous_admin() {
        let state = AuthState {
            public_pem: Arc::new(Vec::new()),
            auth_enabled: false,
            introspection_key: None,
        };
        let req = HttpRequest::get("/graphql").body(Body::empty()).unwrap();
        let (status, body) = call(state, req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "anonymous:admin");
    }

    #[tokio::test]
    async fn missing_token_is_unauthorized() {
        let (public_pem, _) = keys_and_token(Role::Admin);
        let req = HttpRequest::get("/graphql").body(Body::empty()).unwrap();
        let (status, _) = call(enforcing(public_pem, None), req).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn bearer_token_authenticates() {
        let (public_pem, token) = keys_and_token(Role::User);
        let req = HttpRequest::get("/graphql")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let (status, body) = call(enforcing(public_pem, None), req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "alice:user");
    }

    #[tokio::test]
    async fn cookie_takes_precedence_over_bearer() {
        let (public_pem, cookie_token) = keys_and_token(Role::Readonly);
        let req = HttpRequest::get("/graphql")
            .header(
                header::COOKIE,
                format!("other=1; koan_access={cookie_token}"),
            )
            .header(header::AUTHORIZATION, "Bearer garbage")
            .body(Body::empty())
            .unwrap();
        let (status, body) = call(enforcing(public_pem, None), req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "alice:readonly");
    }

    #[tokio::test]
    async fn query_param_token_only_works_on_the_ws_route() {
        let (public_pem, token) = keys_and_token(Role::Admin);

        let req = HttpRequest::get(format!("/graphql?token={token}"))
            .body(Body::empty())
            .unwrap();
        let (status, _) = call(enforcing(public_pem.clone(), None), req).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let req = HttpRequest::get(format!("/graphql/ws?token={token}"))
            .body(Body::empty())
            .unwrap();
        let (status, body) = call(enforcing(public_pem, None), req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "alice:admin");
    }

    #[tokio::test]
    async fn introspection_key_bypasses_auth_only_when_it_matches() {
        let (public_pem, _) = keys_and_token(Role::Admin);

        let req = HttpRequest::get("/graphql")
            .header("X-Introspection-Key", "sekrit")
            .body(Body::empty())
            .unwrap();
        let (status, body) = call(enforcing(public_pem.clone(), Some("sekrit")), req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "anonymous:admin");

        let req = HttpRequest::get("/graphql")
            .header("X-Introspection-Key", "sekrjt")
            .body(Body::empty())
            .unwrap();
        let (status, _) = call(enforcing(public_pem, Some("sekrit")), req).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn tampered_token_is_rejected() {
        let (public_pem, token) = keys_and_token(Role::Admin);
        let req = HttpRequest::get("/graphql")
            .header(header::AUTHORIZATION, format!("Bearer {token}x"))
            .body(Body::empty())
            .unwrap();
        let (status, _) = call(enforcing(public_pem, None), req).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn token_signed_by_another_key_is_rejected() {
        let (_, token) = keys_and_token(Role::Admin);
        let (other_public, _) = keys_and_token(Role::Admin);
        let req = HttpRequest::get("/graphql")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let (status, _) = call(enforcing(other_public, None), req).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn unparseable_role_claim_falls_back_to_readonly() {
        let (private_pem, public_pem) = auth::generate_keypair_pem().unwrap();
        let token = auth::mint_access_token_with_role_str(
            private_pem.as_bytes(),
            7,
            "alice",
            "wizard",
            900,
        )
        .unwrap();
        let req = HttpRequest::get("/graphql")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let (status, body) = call(enforcing(public_pem.into_bytes(), None), req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "alice:readonly");
    }
}
