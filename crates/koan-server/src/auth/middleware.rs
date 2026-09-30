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

use koan_core::auth;
use koan_core::db::pool::Pool;

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
    /// Where a token's account is looked up; see `super::current_user`.
    pub pool: Arc<Pool>,
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

    let user = match auth::validate_access_token(&state.public_pem, &token) {
        Ok(claims) => super::current_user(&state.pool, claims).await,
        Err(_) => None,
    };
    match user {
        Some(user) => {
            request.extensions_mut().insert(user);
            next.run(request).await
        }
        None => (
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
    use koan_core::auth::Role;
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

    /// A live keypair plus a matching token for `alice` at `role`.
    fn keys_and_token(role: Role) -> (Vec<u8>, String) {
        let (private_pem, public_pem) = auth::generate_keypair_pem().unwrap();
        let token = auth::mint_access_token(private_pem.as_bytes(), 1, "alice", role, 900).unwrap();
        (public_pem.into_bytes(), token)
    }

    /// Enforcing auth over a database whose first account is `username` at
    /// `role`: id 1, which the tokens above name.
    fn enforcing_with(
        public_pem: Vec<u8>,
        key: Option<&str>,
        username: &str,
        role: Role,
    ) -> (AuthState, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("koan.db");
        let db = koan_core::db::connection::Database::open(&path).unwrap();
        koan_core::db::queries::auth::create_user(&db.conn, username, "pw", role).unwrap();
        let state = AuthState {
            public_pem: Arc::new(public_pem),
            auth_enabled: true,
            introspection_key: key.map(|k| Arc::new(k.to_string())),
            pool: Arc::new(Pool::new(path)),
        };
        (state, dir)
    }

    fn enforcing(
        public_pem: Vec<u8>,
        key: Option<&str>,
        role: Role,
    ) -> (AuthState, tempfile::TempDir) {
        enforcing_with(public_pem, key, "alice", role)
    }

    #[tokio::test]
    async fn auth_disabled_grants_anonymous_admin() {
        let state = AuthState {
            public_pem: Arc::new(Vec::new()),
            auth_enabled: false,
            introspection_key: None,
            pool: Arc::new(Pool::new("/nonexistent/koan.db".into())),
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
        let (state, _dir) = enforcing(public_pem, None, Role::Admin);
        let (status, _) = call(state, req).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn bearer_token_authenticates() {
        let (public_pem, token) = keys_and_token(Role::User);
        let req = HttpRequest::get("/graphql")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let (state, _dir) = enforcing(public_pem, None, Role::User);
        let (status, body) = call(state, req).await;
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
        let (state, _dir) = enforcing(public_pem, None, Role::Readonly);
        let (status, body) = call(state, req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "alice:readonly");
    }

    #[tokio::test]
    async fn query_param_token_only_works_on_the_ws_route() {
        let (public_pem, token) = keys_and_token(Role::Admin);

        let req = HttpRequest::get(format!("/graphql?token={token}"))
            .body(Body::empty())
            .unwrap();
        let (state, _dir) = enforcing(public_pem, None, Role::Admin);
        let (status, _) = call(state.clone(), req).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let req = HttpRequest::get(format!("/graphql/ws?token={token}"))
            .body(Body::empty())
            .unwrap();
        let (status, body) = call(state, req).await;
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
        let (state, _dir) = enforcing(public_pem, Some("sekrit"), Role::Admin);
        let (status, body) = call(state.clone(), req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "anonymous:admin");

        let req = HttpRequest::get("/graphql")
            .header("X-Introspection-Key", "sekrjt")
            .body(Body::empty())
            .unwrap();
        let (status, _) = call(state, req).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn tampered_token_is_rejected() {
        let (public_pem, token) = keys_and_token(Role::Admin);
        let req = HttpRequest::get("/graphql")
            .header(header::AUTHORIZATION, format!("Bearer {token}x"))
            .body(Body::empty())
            .unwrap();
        let (state, _dir) = enforcing(public_pem, None, Role::Admin);
        let (status, _) = call(state, req).await;
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
        let (state, _dir) = enforcing(other_public, None, Role::Admin);
        let (status, _) = call(state, req).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn the_role_is_the_accounts_now_not_the_tokens() {
        let (public_pem, token) = keys_and_token(Role::Admin);
        let req = || {
            HttpRequest::get("/graphql")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap()
        };
        // Demoted since the token was minted.
        let (state, _dir) = enforcing(public_pem.clone(), None, Role::Readonly);
        assert_eq!(
            call(state, req()).await,
            (StatusCode::OK, "alice:readonly".into())
        );

        // Deleted since.
        let (state, dir) = enforcing(public_pem.clone(), None, Role::Admin);
        let db = koan_core::db::connection::Database::open(&dir.path().join("koan.db")).unwrap();
        koan_core::db::queries::auth::delete_user(&db.conn, 1).unwrap();
        assert_eq!(call(state, req()).await.0, StatusCode::UNAUTHORIZED);

        // Its id now belongs to someone else.
        let (state, _dir) = enforcing_with(public_pem, None, "bob", Role::Admin);
        assert_eq!(call(state, req()).await.0, StatusCode::UNAUTHORIZED);
    }
}
