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
//! Registration is open to any client and keeps no state. The client id is a
//! JWT signed with the server's key and carrying the client's redirect URIs, so
//! a registration survives a restart without a table of clients, and a key
//! rotation sends every client back to register. Since anyone may register
//! under any name, the consent page names a client by the host it returns to.
//! Codes live in memory for a minute and are spent once.
//!
//! Every address here is `sharing.public_url`: an issuer taken from request
//! headers would let a client choose it. Without one, none of this is served.

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
use koan_core::db::queries::auth::{self as auth_queries, OAuthGrant};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use super::pages::{SIGNIN_BODY, SIGNIN_MAIN, SIGNIN_TITLE};
use super::{UiState, html, open};
use crate::auth::AuthUser;
use crate::share::{escape, not_found};

const CODE_TTL_SECS: u64 = 60;
const CLIENT_TYP: &str = "koan-client";
const MAX_REDIRECT_URIS: usize = 5;
const MAX_REDIRECT_URI_LEN: usize = 512;
const MAX_CLIENT_NAME: usize = 80;
/// A registration is a few URIs and a name.
pub(super) const MAX_REGISTRATION_BODY: usize = 8 * 1024;

/// The path, under this server's origin, of `/mcp`'s resource metadata.
pub const RESOURCE_METADATA: &str = "/.well-known/oauth-protected-resource/mcp";

/// An approval, until its code is spent and for as long after as the code
/// would have lived, so a second use can be told from a forged one.
pub(super) struct Code {
    client_id: String,
    client_name: String,
    /// As registered: compared as a string, never re-serialised.
    redirect_uri: String,
    /// Whether the authorization request named it, which decides whether the
    /// token request must (RFC 6749 §4.1.3).
    redirect_given: bool,
    challenge: String,
    user_id: i64,
    expires: u64,
    /// The grant the code was exchanged for.
    spent: Option<String>,
}

pub(super) type Codes = Arc<parking_lot::Mutex<HashMap<String, Code>>>;

/// This server's address, which OAuth needs a fixed one of.
pub(super) fn public_base(s: &UiState) -> Option<String> {
    s.public_url
        .as_deref()
        .map(|u| u.trim().trim_end_matches('/'))
        .filter(|u| !u.is_empty())
        .map(str::to_owned)
}

pub(super) async fn protected_resource(State(s): State<UiState>) -> Response {
    let Some(base) = public_base(&s) else {
        return not_found();
    };
    Json(json!({
        "resource": format!("{base}/mcp"),
        "authorization_servers": [base],
        "bearer_methods_supported": ["header"],
        "resource_name": "kōan",
    }))
    .into_response()
}

pub(super) async fn authorization_server(State(s): State<UiState>) -> Response {
    let Some(base) = public_base(&s) else {
        return not_found();
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

fn is_loopback(url: &Url) -> bool {
    matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
}

/// HTTPS, or plain HTTP back to this machine, where a desktop client listens
/// for its callback; and with `mcp.redirect_hosts` set, one of those hosts or
/// this machine.
fn allowed_redirect(uri: &str, hosts: &[String]) -> bool {
    let Ok(url) = Url::parse(uri) else {
        return false;
    };
    if uri.len() > MAX_REDIRECT_URI_LEN || url.fragment().is_some() {
        return false;
    }
    let scheme_ok = match url.scheme() {
        "https" => url.host().is_some(),
        "http" => is_loopback(&url),
        _ => false,
    };
    scheme_ok
        && (hosts.is_empty()
            || is_loopback(&url)
            || url
                .host_str()
                .is_some_and(|h| hosts.iter().any(|a| a.eq_ignore_ascii_case(h))))
}

#[derive(Serialize, Deserialize)]
struct Client {
    typ: String,
    redirect_uris: Vec<String>,
    client_name: String,
    iat: u64,
}

fn client(s: &UiState, id: &str) -> Option<Client> {
    let key = DecodingKey::from_ed_pem(&s.auth.public_pem).ok()?;
    let mut v = Validation::new(Algorithm::EdDSA);
    v.set_required_spec_claims::<&str>(&[]);
    v.validate_exp = false;
    let c = jsonwebtoken::decode::<Client>(id, &key, &v).ok()?.claims;
    (c.typ == CLIENT_TYP).then_some(c)
}

#[derive(Deserialize)]
pub(super) struct Registration {
    #[serde(default)]
    redirect_uris: Vec<String>,
    client_name: Option<String>,
}

pub(super) async fn register(State(s): State<UiState>, Json(r): Json<Registration>) -> Response {
    if public_base(&s).is_none() {
        return not_found();
    }
    if r.redirect_uris.is_empty()
        || r.redirect_uris.len() > MAX_REDIRECT_URIS
        || !r
            .redirect_uris
            .iter()
            .all(|u| allowed_redirect(u, &s.redirect_hosts))
    {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_redirect_uri",
            "redirect URIs must be https, or http to localhost, and this server may limit the hosts",
        );
    }
    let client_name: String = r
        .client_name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or("Unnamed app")
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_CLIENT_NAME)
        .collect();
    let c = Client {
        typ: CLIENT_TYP.into(),
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

/// A `resource` names `/mcp` or is absent: it is the only resource here.
fn resource_ok(base: &str, resource: &str) -> bool {
    resource.is_empty() || resource.trim_end_matches('/') == format!("{base}/mcp")
}

/// An authorization request this server will answer: a client it registered,
/// and one of that client's own redirect URIs. Until both hold, a failure is a
/// page here, never a redirect, or this would bounce users to any address.
/// The client's registered redirect URI is returned as the string it registered.
fn check(s: &UiState, q: &AuthorizeParams) -> Result<(String, Client, String, Url), Box<Response>> {
    let refuse = |why: &str| {
        Box::new(html(
            StatusCode::BAD_REQUEST,
            page("Cannot connect", &format!("<p>{}</p>", escape(why))),
        ))
    };
    let base = public_base(s).ok_or_else(|| Box::new(not_found()))?;
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
    Ok((base, c, uri, url))
}

/// The answer to an authorization request whose client and redirect URI hold:
/// `pairs`, the request's `state`, and the issuer (RFC 9207).
fn answer(base: &str, mut to: Url, q: &AuthorizeParams, pairs: &[(&str, &str)]) -> Response {
    {
        let mut qp = to.query_pairs_mut();
        qp.extend_pairs(pairs);
        if !q.state.is_empty() {
            qp.append_pair("state", &q.state);
        }
        qp.append_pair("iss", base);
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

/// Why a well-formed client's request is refused, as the error to send back.
fn request_error(base: &str, q: &AuthorizeParams) -> Option<(&'static str, &'static str)> {
    if q.response_type != "code" {
        return Some(("unsupported_response_type", "only code"));
    }
    if q.code_challenge.is_empty() || q.code_challenge_method != "S256" {
        return Some(("invalid_request", "PKCE with S256 is required"));
    }
    if !resource_ok(base, &q.resource) {
        return Some(("invalid_target", "the only resource here is /mcp"));
    }
    None
}

pub(super) fn page(title: &str, body: &str) -> String {
    format!(
        "{head}</head><body class=\"{SIGNIN_BODY}\"><main class=\"{SIGNIN_MAIN}\">\
<h1 class=\"{SIGNIN_TITLE}\">kōan</h1>{body}</main></body></html>",
        head = super::pages::head(title),
    )
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
    let (base, c, _, to) = match check(&s, &q) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if let Some((error, why)) = request_error(&base, &q) {
        return answer(
            &base,
            to,
            &q,
            &[("error", error), ("error_description", why)],
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
        hidden("redirect_uri", &q.redirect_uri),
        hidden("code_challenge", &q.code_challenge),
        hidden("code_challenge_method", &q.code_challenge_method),
        hidden("state", &q.state),
        hidden("scope", &q.scope),
        hidden("resource", &q.resource),
    ]
    .concat();
    let here = format!(
        "/oauth/authorize?{}",
        serde_urlencoded_pairs(&[
            ("response_type", &q.response_type),
            ("client_id", &q.client_id),
            ("redirect_uri", &q.redirect_uri),
            ("code_challenge", &q.code_challenge),
            ("code_challenge_method", &q.code_challenge_method),
            ("state", &q.state),
            ("scope", &q.scope),
            ("resource", &q.resource),
        ])
    );
    let host = to.host_str().unwrap_or_default();
    let body = format!(
        "<h2>Connect kōan to {host}?</h2>\
<p>An app calling itself <em>{name}</em> wants to {abilities}, as <strong>{user}</strong>. \
Anyone can give an app any name: {host} is where it really goes.</p>\
<p><small>Approve only a connection you started yourself, just now. \
Approving connects whichever account started it.</small></p>\
<form class=\"grid gap-3.5\" method=post action=\"/oauth/authorize\">{fields}\
<button class=\"primary\" name=decision value=allow>Allow</button>\
<button class=\"quiet\" name=decision value=deny>Deny</button></form>\
<form class=\"grid gap-3.5\" method=post action=\"/auth/signout\">{next}<button class=\"quiet\">Not {user}? Sign out</button></form>",
        host = escape(host),
        name = escape(&c.client_name),
        abilities = abilities(&user),
        user = escape(&user.username),
        next = hidden("next", &here),
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

fn serde_urlencoded_pairs(pairs: &[(&str, &str)]) -> String {
    form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs.iter().filter(|(_, v)| !v.is_empty()))
        .finish()
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
    let (base, c, uri, to) = match check(&s, &q) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if let Some((error, why)) = request_error(&base, &q) {
        return answer(
            &base,
            to,
            &q,
            &[("error", error), ("error_description", why)],
        );
    }
    if q.decision != "allow" {
        return answer(&base, to, &q, &[("error", "access_denied")]);
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
            Code {
                client_id: q.client_id.clone(),
                client_name: c.client_name,
                redirect_uri: uri,
                redirect_given: !q.redirect_uri.is_empty(),
                challenge: q.code_challenge.clone(),
                user_id: user.user_id,
                expires: now + CODE_TTL_SECS,
                spent: None,
            },
        );
    }
    answer(&base, to, &q, &[("code", &code)])
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
    resource: String,
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

/// What a code is good for, once the request presenting it has been checked
/// against it: a new grant, or nothing.
enum Exchange {
    Issue(i64, OAuthGrant),
    Replayed(String),
    Refused,
}

fn exchange(s: &UiState, t: &TokenRequest) -> Exchange {
    let now = auth::now_unix();
    let mut codes = s.codes.lock();
    let Some(code) = codes.get_mut(&t.code).filter(|c| c.expires > now) else {
        return Exchange::Refused;
    };
    if let Some(grant) = &code.spent {
        return Exchange::Replayed(grant.clone());
    }
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(t.code_verifier.as_bytes()));
    if code.client_id != t.client_id
        || !(t.redirect_uri == code.redirect_uri
            || !code.redirect_given && t.redirect_uri.is_empty())
        || challenge != code.challenge
    {
        return Exchange::Refused;
    }
    let Ok(id) = auth::random_token() else {
        return Exchange::Refused;
    };
    code.spent = Some(id.clone());
    Exchange::Issue(
        code.user_id,
        OAuthGrant {
            id,
            client_name: code.client_name.clone(),
        },
    )
}

pub(super) async fn token(State(s): State<UiState>, Form(t): Form<TokenRequest>) -> Response {
    let Some(base) = public_base(&s) else {
        return not_found();
    };
    let invalid = |why: &str| oauth_error(StatusCode::BAD_REQUEST, "invalid_grant", why);
    // Clients re-register on `invalid_client`, which is what a key rotation
    // needs of them.
    if (t.grant_type == "authorization_code" || !t.client_id.is_empty())
        && client(&s, &t.client_id).is_none()
    {
        return oauth_error(
            StatusCode::UNAUTHORIZED,
            "invalid_client",
            "unknown client; register again",
        );
    }
    if !resource_ok(&base, &t.resource) {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_target",
            "the only resource here is /mcp",
        );
    }
    match t.grant_type.as_str() {
        "authorization_code" => {
            let (user_id, grant) = match exchange(&s, &t) {
                Exchange::Issue(user_id, grant) => (user_id, grant),
                Exchange::Replayed(grant) => {
                    // OAuth 2.1 §4.1.3: a code used twice was intercepted, and
                    // what was issued for it may be in the wrong hands.
                    let pool = s.pool.clone();
                    let _ = tokio::task::spawn_blocking(move || {
                        let db = open(&pool)?;
                        auth_queries::revoke_grant(&db.conn, &grant).ok()
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
                let user = auth_queries::get_user_by_id(&db.conn, user_id).ok()??;
                let access = auth::mint_scoped_token(
                    &st.auth.private_pem,
                    user.id,
                    &user.username,
                    user.role,
                    st.auth.access_ttl_secs,
                    Some(auth::MCP_SCOPE),
                )
                .ok()?;
                let refresh = auth::random_token().ok()?;
                let expires = auth::now_unix() as i64 + st.auth.refresh_ttl_secs as i64;
                auth_queries::store_grant_token(&db.conn, &refresh, user.id, expires, Some(&grant))
                    .ok()?;
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
        let any: &[String] = &[];
        assert!(allowed_redirect(
            "https://claude.ai/api/mcp/auth_callback",
            any
        ));
        assert!(allowed_redirect("https://chatgpt.com/cb", any));
        assert!(allowed_redirect("http://localhost:33418/callback", any));
        assert!(allowed_redirect("http://127.0.0.1:9/cb", any));
        assert!(!allowed_redirect("http://evil.example/cb", any));
        assert!(!allowed_redirect("https://claude.ai/cb#frag", any));
        assert!(!allowed_redirect("javascript:alert(1)", any));
        let long = format!("https://claude.ai/{}", "a".repeat(600));
        assert!(!allowed_redirect(&long, any));
    }

    #[test]
    fn configured_hosts_limit_registration_but_not_loopback() {
        let hosts = ["claude.ai".to_owned()];
        assert!(allowed_redirect("https://Claude.ai/cb", &hosts));
        assert!(allowed_redirect("http://localhost:9/cb", &hosts));
        assert!(!allowed_redirect("https://chatgpt.com/cb", &hosts));
    }
}
