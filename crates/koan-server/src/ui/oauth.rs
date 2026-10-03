//! koan as an OAuth 2.1 authorization server, so an MCP client can sign in to
//! `/mcp` with a koan account and nothing else in between.
//!
//! The client finds this server from the `/mcp` 401 (RFC 9728), reads its
//! metadata (RFC 8414), registers itself (RFC 7591), and sends its user here to
//! sign in to the web UI and approve. The code it gets back is traded, with
//! PKCE, for koan's own access and refresh tokens: the ones the apps and the
//! web UI hold, so `/mcp` checks them the same way and acts as that account,
//! and refreshing is koan's own rotation.
//!
//! Registration keeps no state. The client id is a JWT signed with the
//! server's key and carrying the client's redirect URIs, so a registration
//! survives a restart without a table of clients. Codes live in memory for a
//! minute and are spent once.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Extension;
use axum::Form;
use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use koan_core::auth;
use koan_core::db::queries::auth as auth_queries;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use super::{UiState, html, open};
use crate::auth::AuthUser;
use crate::share::escape;

const CODE_TTL_SECS: u64 = 60;

/// What an approval grants, until its code is spent.
pub(super) struct Grant {
    client_id: String,
    redirect_uri: String,
    challenge: String,
    user_id: i64,
    expires: u64,
}

pub(super) type Codes = Arc<parking_lot::Mutex<HashMap<String, Grant>>>;

/// The path, under this server's origin, of `/mcp`'s resource metadata.
pub const RESOURCE_METADATA: &str = "/.well-known/oauth-protected-resource/mcp";

fn base(s: &UiState, headers: &HeaderMap) -> Result<String, Box<Response>> {
    crate::origin::origin(headers, s.public_url.as_deref())
        .ok_or_else(|| Box::new((StatusCode::BAD_REQUEST, "no Host").into_response()))
}

pub(super) async fn protected_resource(State(s): State<UiState>, headers: HeaderMap) -> Response {
    let base = match base(&s, &headers) {
        Ok(b) => b,
        Err(r) => return *r,
    };
    Json(json!({
        "resource": format!("{base}/mcp"),
        "authorization_servers": [base],
        "bearer_methods_supported": ["header"],
        "resource_name": "kōan",
    }))
    .into_response()
}

pub(super) async fn authorization_server(State(s): State<UiState>, headers: HeaderMap) -> Response {
    let base = match base(&s, &headers) {
        Ok(b) => b,
        Err(r) => return *r,
    };
    Json(json!({
        "issuer": base,
        "authorization_endpoint": format!("{base}/oauth/authorize"),
        "token_endpoint": format!("{base}/oauth/token"),
        "registration_endpoint": format!("{base}/oauth/register"),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none"],
        "authorization_response_iss_parameter_supported": true,
    }))
    .into_response()
}

fn oauth_error(status: StatusCode, error: &str, description: &str) -> Response {
    (
        status,
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({ "error": error, "error_description": description })),
    )
        .into_response()
}

/// HTTPS anywhere; plain HTTP only back to this machine, where a desktop
/// client listens for its callback.
fn allowed_redirect(uri: &str) -> bool {
    let Ok(url) = Url::parse(uri) else {
        return false;
    };
    if url.fragment().is_some() {
        return false;
    }
    match url.scheme() {
        "https" => url.host().is_some(),
        "http" => matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")),
        _ => false,
    }
}

#[derive(Serialize, Deserialize)]
struct Client {
    redirect_uris: Vec<String>,
    client_name: String,
    iat: u64,
}

fn client(s: &UiState, id: &str) -> Option<Client> {
    let key = DecodingKey::from_ed_pem(&s.auth.public_pem).ok()?;
    let mut v = Validation::new(Algorithm::EdDSA);
    v.set_required_spec_claims::<&str>(&[]);
    v.validate_exp = false;
    jsonwebtoken::decode::<Client>(id, &key, &v)
        .ok()
        .map(|d| d.claims)
}

#[derive(Deserialize)]
pub(super) struct Registration {
    #[serde(default)]
    redirect_uris: Vec<String>,
    client_name: Option<String>,
}

pub(super) async fn register(State(s): State<UiState>, Json(r): Json<Registration>) -> Response {
    if r.redirect_uris.is_empty() || !r.redirect_uris.iter().all(|u| allowed_redirect(u)) {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_redirect_uri",
            "redirect URIs must be https, or http to localhost",
        );
    }
    let client_name: String = r
        .client_name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or("An MCP client")
        .chars()
        .take(80)
        .collect();
    let c = Client {
        redirect_uris: r.redirect_uris,
        client_name,
        iat: auth::now_unix(),
    };
    let id = EncodingKey::from_ed_pem(&s.auth.private_pem)
        .and_then(|k| jsonwebtoken::encode(&Header::new(Algorithm::EdDSA), &c, &k));
    let Ok(id) = id else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    (
        StatusCode::CREATED,
        Json(json!({
            "client_id": id,
            "client_id_issued_at": c.iat,
            "client_name": c.client_name,
            "redirect_uris": c.redirect_uris,
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
        })),
    )
        .into_response()
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct AuthorizeParams {
    response_type: String,
    client_id: String,
    redirect_uri: String,
    code_challenge: String,
    code_challenge_method: String,
    state: String,
    scope: String,
    resource: String,
    /// The button pressed on the consent page.
    decision: String,
}

/// An authorization request this server will answer: a client it registered,
/// and one of that client's own redirect URIs. Until both hold, a failure is a
/// page here, never a redirect, or this would bounce users to any address.
fn check(s: &UiState, q: &AuthorizeParams) -> Result<(Client, Url), Box<Response>> {
    let refuse = |why: &str| {
        Box::new(html(
            StatusCode::BAD_REQUEST,
            page("Cannot connect", &format!("<p>{}</p>", escape(why))),
        ))
    };
    let c = client(s, &q.client_id).ok_or_else(|| refuse("This app is not registered here."))?;
    let uri = if q.redirect_uri.is_empty() && c.redirect_uris.len() == 1 {
        c.redirect_uris[0].clone()
    } else {
        q.redirect_uri.clone()
    };
    if !c.redirect_uris.contains(&uri) {
        return Err(refuse(
            "This app asked to return to an address it did not register.",
        ));
    }
    let url = Url::parse(&uri).map_err(|_| refuse("This app's return address is not valid."))?;
    Ok((c, url))
}

fn redirect_with(
    s: &UiState,
    headers: &HeaderMap,
    mut to: Url,
    q: &AuthorizeParams,
    pairs: &[(&str, &str)],
) -> Response {
    {
        let mut qp = to.query_pairs_mut();
        qp.extend_pairs(pairs);
        if !q.state.is_empty() {
            qp.append_pair("state", &q.state);
        }
        if let Ok(iss) = base(s, headers) {
            qp.append_pair("iss", &iss);
        }
    }
    (
        StatusCode::SEE_OTHER,
        [
            (header::LOCATION, to.to_string()),
            (header::CACHE_CONTROL, "no-store".into()),
        ],
    )
        .into_response()
}

fn page(title: &str, body: &str) -> String {
    format!(
        "{head}</head><body class=signin><main><h1>kōan</h1>{body}</main></body></html>",
        head = super::pages::head(title),
    )
}

pub(super) async fn authorize(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
    Query(q): Query<AuthorizeParams>,
) -> Response {
    let (c, to) = match check(&s, &q) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if q.response_type != "code" {
        return redirect_with(
            &s,
            &headers,
            to,
            &q,
            &[("error", "unsupported_response_type")],
        );
    }
    if q.code_challenge.is_empty() || q.code_challenge_method != "S256" {
        return redirect_with(
            &s,
            &headers,
            to,
            &q,
            &[
                ("error", "invalid_request"),
                ("error_description", "PKCE with S256 is required"),
            ],
        );
    }
    let hidden = |name: &str, value: &str| {
        format!(
            "<input type=hidden name={name} value=\"{}\">",
            escape(value)
        )
    };
    let fields = [
        hidden("response_type", &q.response_type),
        hidden("client_id", &q.client_id),
        hidden("redirect_uri", to.as_str()),
        hidden("code_challenge", &q.code_challenge),
        hidden("code_challenge_method", &q.code_challenge_method),
        hidden("state", &q.state),
        hidden("scope", &q.scope),
        hidden("resource", &q.resource),
    ]
    .concat();
    let body = format!(
        "<form method=post action=\"/oauth/authorize\">{fields}\
<p><strong>{name}</strong> wants to use kōan as <strong>{user}</strong>: \
search and change your library and playlists, and play music on your devices.</p>\
<p><small>It will return you to {host}.</small></p>\
<button class=primary name=decision value=allow>Allow</button>\
<button class=quiet name=decision value=deny>Deny</button></form>",
        name = escape(&c.client_name),
        user = escape(&user.username),
        host = escape(to.host_str().unwrap_or_default()),
    );
    let mut resp = html(StatusCode::OK, page("Connect an app", &body));
    // The approval answers with a redirect to the client, which a browser
    // holds the form's `form-action` to as well.
    let csp = super::PAGE_CSP.replace(
        "form-action 'self'",
        &format!("form-action 'self' {}", to.origin().ascii_serialization()),
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
    let (_, to) = match check(&s, &q) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if q.decision != "allow" {
        return redirect_with(&s, &headers, to, &q, &[("error", "access_denied")]);
    }
    if q.code_challenge.is_empty() || q.code_challenge_method != "S256" {
        return redirect_with(&s, &headers, to, &q, &[("error", "invalid_request")]);
    }
    let Ok(code) = auth::random_token() else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let now = auth::now_unix();
    {
        let mut codes = s.codes.lock();
        codes.retain(|_, g| g.expires > now);
        codes.insert(
            code.clone(),
            Grant {
                client_id: q.client_id.clone(),
                redirect_uri: to.to_string(),
                challenge: q.code_challenge.clone(),
                user_id: user.user_id,
                expires: now + CODE_TTL_SECS,
            },
        );
    }
    redirect_with(&s, &headers, to, &q, &[("code", &code)])
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct TokenRequest {
    grant_type: String,
    code: String,
    redirect_uri: String,
    client_id: String,
    code_verifier: String,
    refresh_token: String,
}

fn tokens(s: &UiState, access: String, refresh: String) -> Response {
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({
            "access_token": access,
            "token_type": "Bearer",
            "expires_in": s.auth.access_ttl_secs,
            "refresh_token": refresh,
        })),
    )
        .into_response()
}

pub(super) async fn token(State(s): State<UiState>, Form(t): Form<TokenRequest>) -> Response {
    let invalid = |why: &str| oauth_error(StatusCode::BAD_REQUEST, "invalid_grant", why);
    match t.grant_type.as_str() {
        "authorization_code" => {
            let Some(grant) = s.codes.lock().remove(&t.code) else {
                return invalid("unknown or spent code");
            };
            let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(Sha256::digest(t.code_verifier.as_bytes()));
            if grant.expires <= auth::now_unix()
                || grant.client_id != t.client_id
                || grant.redirect_uri != t.redirect_uri
                || challenge != grant.challenge
            {
                return invalid("code does not match this request");
            }
            let st = s.clone();
            let issued = tokio::task::spawn_blocking(move || {
                let db = open(&st.pool)?;
                let user = auth_queries::get_user_by_id(&db.conn, grant.user_id).ok()??;
                let access = auth::mint_access_token(
                    &st.auth.private_pem,
                    user.id,
                    &user.username,
                    user.role,
                    st.auth.access_ttl_secs,
                )
                .ok()?;
                let refresh = auth::random_token().ok()?;
                let expires = auth::now_unix() as i64 + st.auth.refresh_ttl_secs as i64;
                auth_queries::store_refresh_token(&db.conn, &refresh, user.id, expires).ok()?;
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
            let rotated =
                tokio::task::spawn_blocking(move || crate::auth::routes::rotate(&auth, &rt).ok())
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
            "unsupported_grant_type",
            "authorization_code or refresh_token",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::allowed_redirect;

    #[test]
    fn redirects_are_https_or_this_machine() {
        assert!(allowed_redirect("https://claude.ai/api/mcp/auth_callback"));
        assert!(allowed_redirect("http://localhost:33418/callback"));
        assert!(allowed_redirect("http://127.0.0.1:9/cb"));
        assert!(!allowed_redirect("http://evil.example/cb"));
        assert!(!allowed_redirect("https://claude.ai/cb#frag"));
        assert!(!allowed_redirect("javascript:alert(1)"));
    }
}
