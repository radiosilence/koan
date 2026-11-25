//! Invite tokens: what a link to join a server carries.
//!
//! A token is a JWT signed with the server's key, naming an account, a mark
//! of its password hash, and when it stops working. Nothing is stored when one
//! is made, so another can be made at any time, and one works until it
//! expires or the account's password changes, which changes the mark. So a
//! token that sets the account's password works once, and a password reset is
//! how a link sent to the wrong place is withdrawn.
//!
//! What a token is traded for is the service's to decide: kōan's apps get an
//! API key each, a browser might choose the password. How the link looks is
//! the service's too.

use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::accounts::{AccountError, other};
use crate::users::{self, UserRow};

/// How long an invite sent as a link works for.
pub const TOKEN_TTL_SECS: u64 = 7 * 24 * 60 * 60;
/// How long an invite shown as a QR code works for. A code on a screen is seen
/// by whoever can see the screen, or a recording of it.
pub const QR_TOKEN_TTL_SECS: u64 = 10 * 60;
const TOKEN_TYP: &str = "koan-invite";

#[derive(Serialize, Deserialize)]
struct Claims {
    typ: String,
    /// The account's id. Ids are never reused, so a token outlives no account
    /// it was made for.
    sub: i64,
    username: String,
    /// [`password_mark`] of the account's password hash when the token was
    /// made.
    pwd: String,
    iat: u64,
    exp: u64,
}

/// Enough of a digest of the password hash to tell when it changed, and too
/// little to say anything about the password.
fn password_mark(password_hash: &str) -> String {
    crate::secret::sha256_hex(password_hash)[..16].to_owned()
}

/// A token for the account `user_id`, signed with the server's private key
/// and good for `ttl_secs`.
pub fn mint_token(
    conn: &Connection,
    private_pem: &[u8],
    user_id: i64,
    ttl_secs: u64,
) -> Result<String, AccountError> {
    let user = users::get_user_by_id(conn, user_id)
        .map_err(other)?
        .ok_or_else(|| AccountError::NoSuchUser(user_id.to_string()))?;
    let now = crate::clock::unix_secs() as u64;
    let claims = Claims {
        typ: TOKEN_TYP.into(),
        sub: user.id,
        username: user.username,
        pwd: password_mark(&user.password_hash),
        iat: now,
        exp: now.saturating_add(ttl_secs),
    };
    let key = EncodingKey::from_ed_pem(private_pem).map_err(other)?;
    jsonwebtoken::encode(&Header::new(Algorithm::EdDSA), &claims, &key).map_err(other)
}

/// The account `token` invites, as it stands: refused unless the token was
/// signed with this server's key, is unexpired, and names an account that
/// still exists with the password it had when the token was made.
pub fn invited(conn: &Connection, public_pem: &[u8], token: &str) -> Result<UserRow, AccountError> {
    let key = DecodingKey::from_ed_pem(public_pem).map_err(other)?;
    let mut validation = Validation::new(Algorithm::EdDSA);
    validation.set_required_spec_claims(&["exp"]);
    // Both clocks are the server's, so no skew to allow for.
    validation.leeway = 0;
    let claims = jsonwebtoken::decode::<Claims>(token, &key, &validation)
        .map_err(|_| AccountError::BadInvite)?
        .claims;
    if claims.typ != TOKEN_TYP {
        return Err(AccountError::BadInvite);
    }
    users::get_user_by_id(conn, claims.sub)
        .map_err(other)?
        .filter(|u| u.username == claims.username)
        .filter(|u| password_mark(&u.password_hash) == claims.pwd)
        .ok_or(AccountError::BadInvite)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Role;
    use crate::accounts::{create_account, delete_account, set_password};
    use crate::keys::generate_pem;
    use crate::schema::test_db;

    #[test]
    fn a_token_lasts_as_long_as_it_was_minted_for() {
        let conn = test_db();
        let (private, public) = generate_pem().unwrap();
        let made = create_account(&conn, "sarita", Role::User).unwrap();
        let key = DecodingKey::from_ed_pem(public.as_bytes()).unwrap();
        for ttl in [TOKEN_TTL_SECS, QR_TOKEN_TTL_SECS] {
            let token = mint_token(&conn, private.as_bytes(), made.id, ttl).unwrap();
            let claims =
                jsonwebtoken::decode::<Claims>(&token, &key, &Validation::new(Algorithm::EdDSA))
                    .unwrap()
                    .claims;
            assert_eq!(claims.exp - claims.iat, ttl);
        }
    }

    #[test]
    fn only_this_servers_live_tokens_name_an_account() {
        let conn = test_db();
        let (private, public) = generate_pem().unwrap();
        let made = create_account(&conn, "sarita", Role::User).unwrap();
        let token = mint_token(&conn, private.as_bytes(), made.id, TOKEN_TTL_SECS).unwrap();
        assert_eq!(
            invited(&conn, public.as_bytes(), &token).unwrap().username,
            "sarita"
        );
        // Another server's key, something that is not a token, an access
        // token, or a token for a deleted account are all refused alike.
        let (_, elsewhere) = generate_pem().unwrap();
        assert!(matches!(
            invited(&conn, elsewhere.as_bytes(), &token),
            Err(AccountError::BadInvite)
        ));
        assert!(matches!(
            invited(&conn, public.as_bytes(), "nonsense"),
            Err(AccountError::BadInvite)
        ));
        let session =
            crate::token::mint_access_token(private.as_bytes(), made.id, "sarita", Role::User, 60)
                .unwrap();
        assert!(matches!(
            invited(&conn, public.as_bytes(), &session),
            Err(AccountError::BadInvite)
        ));
        let now = crate::clock::unix_secs() as u64;
        let lapsed = Claims {
            typ: TOKEN_TYP.into(),
            sub: made.id,
            username: "sarita".into(),
            pwd: password_mark(
                &users::get_user_by_id(&conn, made.id)
                    .unwrap()
                    .unwrap()
                    .password_hash,
            ),
            iat: now - 1200,
            exp: now - 600,
        };
        let expired = jsonwebtoken::encode(
            &Header::new(Algorithm::EdDSA),
            &lapsed,
            &EncodingKey::from_ed_pem(private.as_bytes()).unwrap(),
        )
        .unwrap();
        assert!(matches!(
            invited(&conn, public.as_bytes(), &expired),
            Err(AccountError::BadInvite)
        ));
        delete_account(&conn, "sarita").unwrap();
        assert!(matches!(
            invited(&conn, public.as_bytes(), &token),
            Err(AccountError::BadInvite)
        ));
    }

    #[test]
    fn a_new_password_withdraws_links_already_sent() {
        let conn = test_db();
        let (private, public) = generate_pem().unwrap();
        let made = create_account(&conn, "sarita", Role::User).unwrap();
        let sent = mint_token(&conn, private.as_bytes(), made.id, TOKEN_TTL_SECS).unwrap();
        set_password(&conn, "sarita", None).unwrap();
        assert!(matches!(
            invited(&conn, public.as_bytes(), &sent),
            Err(AccountError::BadInvite)
        ));
        let again = mint_token(&conn, private.as_bytes(), made.id, TOKEN_TTL_SECS).unwrap();
        invited(&conn, public.as_bytes(), &again).unwrap();
    }
}
