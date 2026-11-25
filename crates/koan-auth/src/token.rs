//! Access tokens: short-lived JWTs signed with the server's Ed25519 key,
//! naming an account and what it was when the token was made.
//!
//! A token's claims hold for its whole life, so a service that takes one reads
//! the account again rather than trusting its role (a demoted admin would
//! otherwise keep the role until the token lapsed).

use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

use crate::{AuthError, Role};

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    /// The account's id.
    pub sub: i64,
    pub username: String,
    pub role: String,
    /// Issued at (unix seconds).
    pub iat: u64,
    /// Expires at (unix seconds).
    pub exp: u64,
    /// The one resource a scoped token is good for. An unscoped token is a
    /// session's, and goes anywhere a session does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

/// A session's access token.
pub fn mint_access_token(
    private_pem: &[u8],
    user_id: i64,
    username: &str,
    role: Role,
    ttl_secs: u64,
) -> Result<String, AuthError> {
    mint_scoped_token(private_pem, user_id, username, role, ttl_secs, None)
}

/// An access token, limited to `scope` when there is one.
pub fn mint_scoped_token(
    private_pem: &[u8],
    user_id: i64,
    username: &str,
    role: Role,
    ttl_secs: u64,
    scope: Option<&str>,
) -> Result<String, AuthError> {
    let now = crate::clock::unix_secs() as u64;
    let claims = Claims {
        sub: user_id,
        username: username.to_string(),
        role: role.as_str().to_string(),
        iat: now,
        // A lifetime set near `u64::MAX` would otherwise wrap round to a
        // token that is born expired, or panic in a debug build.
        exp: now.saturating_add(ttl_secs),
        scope: scope.map(str::to_owned),
    };
    let key = EncodingKey::from_ed_pem(private_pem)?;
    Ok(jsonwebtoken::encode(
        &Header::new(Algorithm::EdDSA),
        &claims,
        &key,
    )?)
}

/// A session's access token's claims. A scoped token is refused: it is good
/// only where its scope is asked for.
pub fn validate_access_token(public_pem: &[u8], token: &str) -> Result<Claims, AuthError> {
    validate_scoped_token(public_pem, token, None)
}

/// The claims of an access token whose scope is exactly `scope`.
pub fn validate_scoped_token(
    public_pem: &[u8],
    token: &str,
    scope: Option<&str>,
) -> Result<Claims, AuthError> {
    let key = DecodingKey::from_ed_pem(public_pem)?;
    let mut validation = Validation::new(Algorithm::EdDSA);
    // `sub` and `iat` are this crate's own fields, not the JWT spec's strings.
    validation.set_required_spec_claims(&["exp"]);
    let claims = jsonwebtoken::decode::<Claims>(token, &key, &validation)?.claims;
    if claims.scope.as_deref() != scope {
        return Err(AuthError::Other("token not valid here".into()));
    }
    Ok(claims)
}

/// A duration such as "15m", "7d", "24h" or "3600s" (or bare seconds), in
/// seconds.
pub fn parse_duration_secs(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (num, multiplier) = if let Some(n) = s.strip_suffix('d') {
        (n, 86400)
    } else if let Some(n) = s.strip_suffix('h') {
        (n, 3600)
    } else if let Some(n) = s.strip_suffix('m') {
        (n, 60)
    } else if let Some(n) = s.strip_suffix('s') {
        (n, 1)
    } else {
        (s, 1)
    };
    num.parse::<u64>().ok()?.checked_mul(multiplier)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::generate_pem;

    #[test]
    fn a_scoped_token_is_good_only_where_its_scope_is_asked_for() {
        let (private, public) = generate_pem().unwrap();
        let scoped =
            mint_scoped_token(private.as_bytes(), 1, "user", Role::Admin, 900, Some("mcp"))
                .unwrap();
        assert!(validate_access_token(public.as_bytes(), &scoped).is_err());
        assert!(validate_scoped_token(public.as_bytes(), &scoped, Some("mcp")).is_ok());
        let session = mint_access_token(private.as_bytes(), 1, "user", Role::Admin, 900).unwrap();
        assert!(validate_scoped_token(public.as_bytes(), &session, Some("mcp")).is_err());
    }

    #[test]
    fn keypair_generate_and_jwt_roundtrip() {
        let (private, public) = generate_pem().unwrap();
        let token =
            mint_access_token(private.as_bytes(), 42, "testuser", Role::Admin, 3600).unwrap();
        let claims = validate_access_token(public.as_bytes(), &token).unwrap();
        assert_eq!(claims.sub, 42);
        assert_eq!(claims.username, "testuser");
        assert_eq!(claims.role, "admin");
    }

    #[test]
    fn a_token_signed_by_another_key_is_refused() {
        let (private, _) = generate_pem().unwrap();
        let (_, other) = generate_pem().unwrap();
        let token = mint_access_token(private.as_bytes(), 1, "user", Role::User, 60).unwrap();
        assert!(validate_access_token(other.as_bytes(), &token).is_err());
    }

    #[test]
    fn expired_token_rejected() {
        let (private, public) = generate_pem().unwrap();
        let now = crate::clock::unix_secs() as u64;
        let claims = Claims {
            sub: 1,
            username: "user".into(),
            role: "user".into(),
            iat: now - 1200,
            exp: now - 600,
            scope: None,
        };
        let key = EncodingKey::from_ed_pem(private.as_bytes()).unwrap();
        let token = jsonwebtoken::encode(&Header::new(Algorithm::EdDSA), &claims, &key).unwrap();
        assert!(validate_access_token(public.as_bytes(), &token).is_err());
    }

    #[test]
    fn a_lifetime_too_long_to_count_still_makes_a_live_token() {
        let (private, public) = generate_pem().unwrap();
        let token = mint_access_token(private.as_bytes(), 1, "user", Role::User, u64::MAX).unwrap();
        assert_eq!(
            validate_access_token(public.as_bytes(), &token)
                .unwrap()
                .exp,
            u64::MAX
        );
    }

    #[test]
    fn parse_duration() {
        assert_eq!(parse_duration_secs("15m"), Some(900));
        assert_eq!(parse_duration_secs("7d"), Some(604800));
        assert_eq!(parse_duration_secs("24h"), Some(86400));
        assert_eq!(parse_duration_secs("3600s"), Some(3600));
        assert_eq!(parse_duration_secs("3600"), Some(3600));
        assert_eq!(parse_duration_secs(""), None);
        assert_eq!(parse_duration_secs("99999999999999999999d"), None);
    }
}
