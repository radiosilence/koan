//! Accounts and signing in, for kōan and the services built beside it.
//!
//! Everything here is a function over a `rusqlite::Connection`, or state a
//! server keeps in memory, so a service keeps its own database, its own HTTP
//! stack and its own pages. The API is synchronous: kōan's core is by design,
//! and a service on tokio runs these calls on its blocking pool, since argon2
//! and the password verifier's waits hold a thread.
//!
//! - [`users`] and [`accounts`]: accounts and their roles, the first admin, a
//!   new password ending every session, and the last admin, who cannot be
//!   removed or demoted.
//! - [`sessions`]: opaque secrets a client holds, stored as their SHA-256,
//!   either presented as they are or exchanged for a new one on each use, with
//!   an OAuth grant's spent token coming back taken as theft.
//! - [`api_keys`] and [`app_passwords`]: credentials a client keeps instead of
//!   the account's password.
//! - [`password`]: argon2id, and the [`password::Verifier`] every door that
//!   takes a password goes through.
//! - [`second_factor`], [`totp`] and [`webauthn`]: what a password sign-in
//!   asks for besides, once an account turns it on: a code from an
//!   authenticator app, a security key or passkey, or a recovery code.
//! - [`keys`], [`token`] and [`invite`]: the Ed25519 key a server signs with,
//!   and the access and invite tokens it signs.
//! - [`changes`]: changes to accounts, for what outlives the request that
//!   authenticated it.
//! - [`limit`], [`net`] and [`web`]: per-address limits, the address a request
//!   came from, an authenticating proxy's header, and the checks a form's POST
//!   and a `next` parameter need.
//! - [`oauth`]: the protocol of an OAuth 2.1 authorization server.
//! - [`pair`]: devices without a keyboard, approved from one signed in.
//! - [`setup`]: the code that guards making the first admin.
//! - [`schema`]: the tables, versioned apart from the rest of a database.
//!
//! Nothing here draws a page: each service writes its own, in its own words.

pub mod accounts;
pub mod api_keys;
pub mod app_passwords;
mod cbor;
pub mod changes;
mod clock;
pub mod invite;
pub mod keys;
pub mod limit;
pub mod net;
pub mod oauth;
pub mod pair;
pub mod password;
pub mod schema;
pub mod second_factor;
pub mod secret;
pub mod sessions;
pub mod setup;
mod sql;
pub mod token;
pub mod totp;
pub mod users;
pub mod web;
pub mod webauthn;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The name a server acts under when nobody signed in: sign-in switched off,
/// or a local process such as kōan's own MCP. No account may take it, or it
/// would inherit what is scoped to that name.
pub const ANONYMOUS: &str = "anonymous";

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("jwt error: {0}")]
    Jwt(#[from] jsonwebtoken::errors::Error),
    #[error("argon2 hash error: {0}")]
    Hash(String),
    #[error("password verification failed")]
    InvalidPassword,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("no signing keypair has been made")]
    NoKeypair,
    #[error("{0}")]
    Other(String),
}

/// What an account may do. Admin > User > Readonly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    User,
    Readonly,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::User => "user",
            Role::Readonly => "readonly",
        }
    }

    /// Whether this role has at least `required`'s permissions.
    pub fn has_permission(&self, required: Role) -> bool {
        match required {
            Role::Readonly => true,
            Role::User => matches!(self, Role::Admin | Role::User),
            Role::Admin => matches!(self, Role::Admin),
        }
    }
}

impl std::str::FromStr for Role {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "admin" => Ok(Role::Admin),
            "user" => Ok(Role::User),
            "readonly" => Ok(Role::Readonly),
            _ => Err(format!("invalid role: '{s}'")),
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_permissions() {
        assert!(Role::Admin.has_permission(Role::Admin));
        assert!(Role::Admin.has_permission(Role::User));
        assert!(Role::Admin.has_permission(Role::Readonly));

        assert!(!Role::User.has_permission(Role::Admin));
        assert!(Role::User.has_permission(Role::User));
        assert!(Role::User.has_permission(Role::Readonly));

        assert!(!Role::Readonly.has_permission(Role::Admin));
        assert!(!Role::Readonly.has_permission(Role::User));
        assert!(Role::Readonly.has_permission(Role::Readonly));
    }

    #[test]
    fn roles_read_back_as_written() {
        for role in [Role::Admin, Role::User, Role::Readonly] {
            assert_eq!(role.as_str().parse::<Role>(), Ok(role));
        }
        assert!("root".parse::<Role>().is_err());
    }
}
