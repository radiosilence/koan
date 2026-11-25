//! The protocol of an OAuth 2.1 authorization server for one protected
//! resource, so a client such as an MCP client can sign in with an account
//! here and nothing in between.
//!
//! The client finds the server from the resource's 401 (RFC 9728), reads its
//! metadata (RFC 8414), registers itself (RFC 7591), and sends its user to
//! sign in and approve. The code it gets back is traded, with PKCE, for the
//! service's own access and refresh tokens.
//!
//! Registration is open to any client and keeps no state. The client id is a
//! JWT signed with the server's key and carrying the client's redirect URIs, so
//! a registration survives a restart without a table of clients, and a key
//! rotation sends every client back to register. Since anyone may register
//! under any name, a consent page should name a client by the host it returns
//! to. Codes live in memory for a minute and are spent once.
//!
//! Every address is the [`Server`]'s issuer: one taken from request headers
//! would let a client choose it. The endpoints are `{issuer}/oauth/authorize`,
//! `/oauth/token` and `/oauth/register`. The service serves them, draws the
//! consent page, and decides what tokens a code is worth.

use std::collections::HashMap;
use std::sync::Arc;

use base64::Engine as _;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use url::Url;

use crate::sessions::OAuthGrant;

pub const CODE_TTL_SECS: u64 = 60;
const CLIENT_TYP: &str = "koan-client";
const MAX_REDIRECT_URIS: usize = 5;
const MAX_REDIRECT_URI_LEN: usize = 512;
const MAX_CLIENT_NAME: usize = 80;
/// A registration is a few URIs and a name.
pub const MAX_REGISTRATION_BODY: usize = 8 * 1024;
/// How long after its use a spent refresh token of a grant may come back as a
/// retry rather than a replay: see [`crate::sessions::revoke_replayed_grant`].
pub const REPLAY_GRACE_SECS: i64 = 30;

/// An authorization server for one protected resource.
#[derive(Clone, Debug)]
pub struct Server {
    issuer: String,
    resource: String,
    resource_name: String,
    /// Hosts clients may return to, beyond this machine; empty for any.
    redirect_hosts: Arc<Vec<String>>,
}

impl Server {
    /// `issuer` is the server's own address, `resource_path` where the
    /// protected resource is under it (`/mcp`), and `redirect_hosts` the hosts
    /// a client may register a redirect to (empty for any).
    pub fn new(
        issuer: &str,
        resource_path: &str,
        resource_name: &str,
        redirect_hosts: Vec<String>,
    ) -> Option<Self> {
        let issuer = issuer.trim().trim_end_matches('/');
        if issuer.is_empty() {
            return None;
        }
        Some(Self {
            issuer: issuer.to_owned(),
            resource: format!("{issuer}{resource_path}"),
            resource_name: resource_name.to_owned(),
            redirect_hosts: Arc::new(redirect_hosts),
        })
    }

    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    /// The protected resource's address.
    pub fn resource(&self) -> &str {
        &self.resource
    }

    /// The protected resource's metadata (RFC 9728).
    pub fn protected_resource_metadata(&self) -> Value {
        json!({
            "resource": self.resource,
            "authorization_servers": [self.issuer],
            "bearer_methods_supported": ["header"],
            "resource_name": self.resource_name,
        })
    }

    /// The authorization server's metadata (RFC 8414).
    pub fn authorization_server_metadata(&self) -> Value {
        let base = &self.issuer;
        json!({
            "issuer": base,
            "authorization_endpoint": format!("{base}/oauth/authorize"),
            "token_endpoint": format!("{base}/oauth/token"),
            "registration_endpoint": format!("{base}/oauth/register"),
            "response_types_supported": ["code"],
            "grant_types_supported": ["authorization_code", "refresh_token"],
            "code_challenge_methods_supported": ["S256"],
            "token_endpoint_auth_methods_supported": ["none"],
            "authorization_response_iss_parameter_supported": true,
        })
    }

    /// A `resource` names the protected resource or is absent: it is the only
    /// resource here.
    pub fn resource_ok(&self, resource: &str) -> bool {
        resource.is_empty() || resource.trim_end_matches('/') == self.resource
    }

    /// HTTPS, or plain HTTP back to this machine, where a desktop client
    /// listens for its callback; and with redirect hosts set, one of those
    /// hosts or this machine.
    pub fn allowed_redirect(&self, uri: &str) -> bool {
        allowed_redirect(uri, &self.redirect_hosts)
    }

    /// Register a client: its id, which is a JWT carrying what it registered,
    /// and the registration response's body (RFC 7591).
    pub fn register(&self, private_pem: &[u8], r: Registration) -> Result<Value, OAuthError> {
        if r.redirect_uris.is_empty()
            || r.redirect_uris.len() > MAX_REDIRECT_URIS
            || !r.redirect_uris.iter().all(|u| self.allowed_redirect(u))
        {
            return Err(OAuthError::new(
                "invalid_redirect_uri",
                "redirect URIs must be https, or http to localhost, and this server may limit the hosts",
            ));
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
            iat: crate::clock::unix_secs() as u64,
        };
        let id = EncodingKey::from_ed_pem(private_pem)
            .and_then(|k| jsonwebtoken::encode(&Header::new(Algorithm::EdDSA), &c, &k))
            .map_err(|_| OAuthError::new("server_error", "the client id could not be made"))?;
        Ok(json!({
            "client_id": id,
            "client_id_issued_at": c.iat,
            "client_name": c.client_name,
            "redirect_uris": c.redirect_uris,
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
        }))
    }

    /// The client `id` names, if this server registered it.
    pub fn client(&self, public_pem: &[u8], id: &str) -> Option<Client> {
        let key = DecodingKey::from_ed_pem(public_pem).ok()?;
        let mut v = Validation::new(Algorithm::EdDSA);
        v.set_required_spec_claims::<&str>(&[]);
        v.validate_exp = false;
        let c = jsonwebtoken::decode::<Client>(id, &key, &v).ok()?.claims;
        (c.typ == CLIENT_TYP).then_some(c)
    }

    /// An authorization request this server will answer: a client it
    /// registered, and one of that client's own redirect URIs that this
    /// server still allows. Until both hold, a failure is a page on this
    /// server, never a redirect, or it would bounce people to any address; the
    /// error says why, for that page.
    ///
    /// The redirect is checked against the hosts allowed now, not only when
    /// the client registered: a client id never expires, so limiting the
    /// hosts would otherwise leave every client registered before free to
    /// return anywhere.
    pub fn check(
        &self,
        public_pem: &[u8],
        q: &AuthorizeRequest,
    ) -> Result<Authorizing, &'static str> {
        let client = self
            .client(public_pem, &q.client_id)
            .ok_or("This app is not registered here.")?;
        let uri = if q.redirect_uri.is_empty() && client.redirect_uris.len() == 1 {
            client.redirect_uris[0].clone()
        } else {
            q.redirect_uri.clone()
        };
        if !client.redirect_uris.contains(&uri) {
            return Err("This app asked to return to an address it did not register.");
        }
        if !self.allowed_redirect(&uri) {
            return Err("This server no longer lets apps return to this app's address.");
        }
        let to = Url::parse(&uri).map_err(|_| "This app's return address is not valid.")?;
        Ok(Authorizing {
            client,
            redirect_uri: uri,
            to,
        })
    }

    /// Why a checked client's request is refused, as the error to send back
    /// to it.
    pub fn request_error(&self, q: &AuthorizeRequest) -> Option<(&'static str, &'static str)> {
        if q.response_type != "code" {
            return Some(("unsupported_response_type", "only code"));
        }
        if q.code_challenge.is_empty() || q.code_challenge_method != "S256" {
            return Some(("invalid_request", "PKCE with S256 is required"));
        }
        if !self.resource_ok(&q.resource) {
            return Some((
                "invalid_target",
                "the request names a resource not served here",
            ));
        }
        None
    }

    /// Where to send the browser with the answer to a checked request:
    /// `pairs`, the request's `state`, and the issuer (RFC 9207).
    pub fn answer(&self, to: &Url, q: &AuthorizeRequest, pairs: &[(&str, &str)]) -> String {
        let mut to = to.clone();
        {
            let mut qp = to.query_pairs_mut();
            qp.extend_pairs(pairs);
            if !q.state.is_empty() {
                qp.append_pair("state", &q.state);
            }
            qp.append_pair("iss", &self.issuer);
        }
        to.into()
    }
}

fn is_loopback(url: &Url) -> bool {
    matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
}

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

/// An error to answer a client with: the body of a 4xx (RFC 6749 §5.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthError {
    pub error: &'static str,
    pub description: &'static str,
}

impl OAuthError {
    pub fn new(error: &'static str, description: &'static str) -> Self {
        Self { error, description }
    }

    pub fn body(&self) -> Value {
        json!({ "error": self.error, "error_description": self.description })
    }
}

/// What a client registered, as its id carries it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Client {
    typ: String,
    pub redirect_uris: Vec<String>,
    pub client_name: String,
    pub iat: u64,
}

#[derive(Debug, Default, Deserialize)]
pub struct Registration {
    #[serde(default)]
    pub redirect_uris: Vec<String>,
    pub client_name: Option<String>,
}

/// An authorization request's parameters, as a query or as the consent
/// form posts them back.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AuthorizeRequest {
    pub response_type: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub code_challenge: String,
    pub code_challenge_method: String,
    pub state: String,
    pub scope: String,
    pub resource: String,
    /// The button pressed on the consent page.
    pub decision: String,
}

impl AuthorizeRequest {
    /// The request's own parameters, as a consent form carries them through.
    pub fn fields(&self) -> [(&'static str, &str); 8] {
        [
            ("response_type", &self.response_type),
            ("client_id", &self.client_id),
            ("redirect_uri", &self.redirect_uri),
            ("code_challenge", &self.code_challenge),
            ("code_challenge_method", &self.code_challenge_method),
            ("state", &self.state),
            ("scope", &self.scope),
            ("resource", &self.resource),
        ]
    }

    /// The request's own parameters as a query string, without the empty
    /// ones.
    pub fn query(&self) -> String {
        url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(self.fields().into_iter().filter(|(_, v)| !v.is_empty()))
            .finish()
    }
}

/// A checked authorization request: its client, the redirect URI as the
/// client registered it, and that URI parsed.
#[derive(Debug, Clone)]
pub struct Authorizing {
    pub client: Client,
    pub redirect_uri: String,
    pub to: Url,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct TokenRequest {
    pub grant_type: String,
    pub code: String,
    pub redirect_uri: String,
    pub client_id: String,
    pub code_verifier: String,
    pub refresh_token: String,
    pub resource: String,
}

/// The token response's body.
pub fn tokens(access: &str, refresh: &str, expires_in: u64) -> Value {
    json!({
        "access_token": access,
        "token_type": "Bearer",
        "expires_in": expires_in,
        "refresh_token": refresh,
    })
}

/// An approval, until its code is spent and for as long after as the code
/// would have lived, so a second use can be told from a forged one.
struct Code {
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

/// What a code is good for, once the request presenting it has been checked
/// against it.
#[derive(Debug, PartialEq, Eq)]
pub enum Exchange {
    /// A new grant for this account.
    Issue(i64, OAuthGrant),
    /// The code was spent already: it was intercepted, and the grant it was
    /// exchanged for may be in the wrong hands (OAuth 2.1 §4.1.3), so revoke
    /// it ([`crate::sessions::revoke_grant`]).
    Replayed(String),
    Refused,
}

/// Codes awaiting their token request.
#[derive(Clone, Default)]
pub struct Codes(Arc<Mutex<HashMap<String, Code>>>);

impl Codes {
    /// A code for `user_id`'s approval of the checked request `q`.
    pub fn issue(
        &self,
        q: &AuthorizeRequest,
        approved: &Authorizing,
        user_id: i64,
    ) -> Result<String, crate::AuthError> {
        let code = crate::secret::random_token()?;
        let now = crate::clock::unix_secs() as u64;
        let mut codes = self.0.lock();
        codes.retain(|_, g| g.expires > now);
        codes.insert(
            code.clone(),
            Code {
                client_id: q.client_id.clone(),
                client_name: approved.client.client_name.clone(),
                redirect_uri: approved.redirect_uri.clone(),
                redirect_given: !q.redirect_uri.is_empty(),
                challenge: q.code_challenge.clone(),
                user_id,
                expires: now + CODE_TTL_SECS,
                spent: None,
            },
        );
        Ok(code)
    }

    /// Check a token request against the code it presents, spending the code.
    pub fn exchange(&self, t: &TokenRequest) -> Exchange {
        let now = crate::clock::unix_secs() as u64;
        let mut codes = self.0.lock();
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
        let Ok(id) = crate::secret::random_token() else {
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
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn server() -> Server {
        Server::new("https://music.example/", "/mcp", "kōan", vec![]).unwrap()
    }

    fn registered(private: &str) -> String {
        let body = server()
            .register(
                private.as_bytes(),
                Registration {
                    redirect_uris: vec!["https://client.example/cb".into()],
                    client_name: Some("  Client\u{1}  ".into()),
                },
            )
            .unwrap();
        assert_eq!(body["client_name"], "Client");
        body["client_id"].as_str().unwrap().to_owned()
    }

    fn request(client_id: &str, verifier: &str) -> AuthorizeRequest {
        AuthorizeRequest {
            response_type: "code".into(),
            client_id: client_id.into(),
            code_challenge: base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(Sha256::digest(verifier.as_bytes())),
            code_challenge_method: "S256".into(),
            state: "xyz".into(),
            ..Default::default()
        }
    }

    #[test]
    fn a_registered_client_is_answered_at_its_own_address() {
        let (private, public) = crate::keys::generate_pem().unwrap();
        let id = registered(&private);
        let s = server();
        assert_eq!(s.issuer(), "https://music.example");
        assert_eq!(s.resource(), "https://music.example/mcp");
        let q = request(&id, "verifier");
        let ok = s.check(public.as_bytes(), &q).unwrap();
        assert_eq!(ok.redirect_uri, "https://client.example/cb");
        assert_eq!(s.request_error(&q), None);
        let to = s.answer(&ok.to, &q, &[("code", "c")]);
        assert_eq!(
            to,
            "https://client.example/cb?code=c&state=xyz&iss=https%3A%2F%2Fmusic.example"
        );
        // Another address, or a client registered elsewhere, is a page here.
        let elsewhere = AuthorizeRequest {
            redirect_uri: "https://evil.example/cb".into(),
            ..q.clone()
        };
        assert!(s.check(public.as_bytes(), &elsewhere).is_err());
        let (_, other) = crate::keys::generate_pem().unwrap();
        assert!(s.check(other.as_bytes(), &q).is_err());
        let plain = AuthorizeRequest {
            code_challenge_method: "plain".into(),
            ..q
        };
        assert_eq!(
            s.request_error(&plain),
            Some(("invalid_request", "PKCE with S256 is required"))
        );
    }

    #[test]
    fn a_code_is_spent_once_with_its_verifier() {
        let (private, public) = crate::keys::generate_pem().unwrap();
        let id = registered(&private);
        let s = server();
        let q = request(&id, "verifier");
        let ok = s.check(public.as_bytes(), &q).unwrap();
        let codes = Codes::default();
        let code = codes.issue(&q, &ok, 7).unwrap();
        let token = |verifier: &str| TokenRequest {
            grant_type: "authorization_code".into(),
            code: code.clone(),
            client_id: id.clone(),
            code_verifier: verifier.into(),
            ..Default::default()
        };
        assert_eq!(codes.exchange(&token("wrong")), Exchange::Refused);
        let Exchange::Issue(user, grant) = codes.exchange(&token("verifier")) else {
            panic!("the code was not exchanged");
        };
        assert_eq!((user, grant.client_name.as_str()), (7, "Client"));
        assert_eq!(
            codes.exchange(&token("verifier")),
            Exchange::Replayed(grant.id)
        );
    }

    #[test]
    fn limiting_the_hosts_reaches_clients_registered_before() {
        let (private, public) = crate::keys::generate_pem().unwrap();
        let id = registered(&private);
        let q = request(&id, "verifier");
        assert!(server().check(public.as_bytes(), &q).is_ok());
        let limited = Server::new(
            "https://music.example/",
            "/mcp",
            "kōan",
            vec!["claude.ai".into()],
        )
        .unwrap();
        assert!(limited.check(public.as_bytes(), &q).is_err());
    }

    #[test]
    fn registrations_must_return_somewhere_safe() {
        let (private, _) = crate::keys::generate_pem().unwrap();
        let refused = server().register(
            private.as_bytes(),
            Registration {
                redirect_uris: vec!["http://evil.example/cb".into()],
                client_name: None,
            },
        );
        assert_eq!(refused.unwrap_err().error, "invalid_redirect_uri");
        let none = server().register(private.as_bytes(), Registration::default());
        assert!(none.is_err());
    }
}
