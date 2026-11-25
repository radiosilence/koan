//! Authentication, from `koan-auth`, and where kōan keeps its signing key.
//!
//! The Ed25519 keypair is generated once and kept in the config directory's
//! `auth/`, which is never committed.

use std::path::PathBuf;

pub use koan_auth::app_passwords::{app_password_key, open_app_password, seal_app_password};
pub use koan_auth::changes::{
    account_changed, account_changed_since, account_mark, any_account_changed_since,
};
pub use koan_auth::keys::generate_pem as generate_keypair_pem;
pub use koan_auth::password::{hash_password, verify_password};
pub use koan_auth::second_factor;
pub use koan_auth::secret::{random_api_key, random_app_password, random_token, sha256_hex};
pub use koan_auth::token::{
    Claims, mint_access_token, mint_scoped_token, parse_duration_secs, validate_access_token,
    validate_scoped_token,
};
pub use koan_auth::{ANONYMOUS, AuthError, Role};

use crate::config;

/// The scope of a token granted to an MCP client through OAuth: valid at
/// `/mcp` and nowhere else, so the limits the MCP sets on what a client may do
/// cannot be stepped round by presenting the same token to GraphQL.
pub const MCP_SCOPE: &str = "mcp";

pub fn keypair_dir() -> PathBuf {
    config::config_dir().join("auth")
}

/// Generate a new keypair and write it to [`keypair_dir`]. Returns the PEM,
/// private then public.
pub fn generate_keypair() -> Result<(Vec<u8>, Vec<u8>), AuthError> {
    koan_auth::keys::generate(&keypair_dir())
}

/// The keypair in [`keypair_dir`], private then public.
pub fn load_keypair() -> Result<(Vec<u8>, Vec<u8>), AuthError> {
    koan_auth::keys::load(&keypair_dir()).map_err(|e| match e {
        AuthError::NoKeypair => {
            AuthError::Other("keypair not found — run `koan auth setup` first".into())
        }
        e => e,
    })
}

/// The keypair in [`keypair_dir`], generated there if there is none.
pub fn load_or_generate_keypair() -> Result<(Vec<u8>, Vec<u8>), AuthError> {
    koan_auth::keys::load_or_generate(&keypair_dir())
}
