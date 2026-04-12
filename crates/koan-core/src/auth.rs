//! Authentication primitives: Ed25519 JWT signing, Argon2id password hashing.
//!
//! Ed25519 keypair is generated once and stored in the config directory.
//! JWTs use EdDSA (Ed25519) for signing — 128-bit security, tiny keys, fast.

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use ring::signature::KeyPair;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::config;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

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
    #[error("keypair not found — run `koan auth setup` first")]
    NoKeypair,
    #[error("{0}")]
    Other(String),
}

// ---------------------------------------------------------------------------
// Roles
// ---------------------------------------------------------------------------

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

    /// Returns true if this role has at least the given permission level.
    /// Admin > User > Readonly.
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

// ---------------------------------------------------------------------------
// JWT Claims
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    /// Subject — user ID.
    pub sub: i64,
    /// Username.
    pub username: String,
    /// Role.
    pub role: String,
    /// Issued at (unix timestamp).
    pub iat: u64,
    /// Expiration (unix timestamp).
    pub exp: u64,
}

// ---------------------------------------------------------------------------
// Password hashing (Argon2id)
// ---------------------------------------------------------------------------

/// Hash a password using Argon2id with a random salt.
pub fn hash_password(password: &str) -> Result<String, AuthError> {
    use argon2::Argon2;
    use argon2::password_hash::{PasswordHasher, SaltString, rand_core::OsRng};

    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    argon2
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| AuthError::Hash(e.to_string()))
}

/// Verify a password against an Argon2id hash.
pub fn verify_password(password: &str, hash: &str) -> Result<(), AuthError> {
    use argon2::Argon2;
    use argon2::password_hash::{PasswordHash, PasswordVerifier};

    let parsed = PasswordHash::new(hash).map_err(|e| AuthError::Hash(e.to_string()))?;
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .map_err(|_| AuthError::InvalidPassword)
}

// ---------------------------------------------------------------------------
// Ed25519 Keypair management
// ---------------------------------------------------------------------------

pub fn keypair_dir() -> PathBuf {
    config::config_dir().join("auth")
}

fn private_key_path() -> PathBuf {
    keypair_dir().join("ed25519.pem")
}

fn public_key_path() -> PathBuf {
    keypair_dir().join("ed25519.pub.pem")
}

/// Generate a new Ed25519 keypair and write PEM files to the config dir.
/// Returns (private_pem, public_pem).
pub fn generate_keypair() -> Result<(Vec<u8>, Vec<u8>), AuthError> {
    let dir = keypair_dir();
    fs::create_dir_all(&dir)?;

    // Ensure the auth directory is gitignored — keys must never be committed.
    let gitignore = dir.join(".gitignore");
    if !gitignore.exists() {
        let _ = fs::write(&gitignore, "*\n");
    }

    // Generate Ed25519 keypair using ring (via jsonwebtoken's internal ring dep).
    // jsonwebtoken's EncodingKey::from_ed_pem expects PKCS8 PEM.
    let rng = ring::rand::SystemRandom::new();
    let pkcs8_doc = ring::signature::Ed25519KeyPair::generate_pkcs8(&rng)
        .map_err(|e| AuthError::Other(format!("keypair generation failed: {}", e)))?;

    let private_pem = pem::encode(&pem::Pem::new("PRIVATE KEY", pkcs8_doc.as_ref()));

    // Extract public key from the keypair.
    let kp = ring::signature::Ed25519KeyPair::from_pkcs8(pkcs8_doc.as_ref())
        .map_err(|e| AuthError::Other(format!("keypair parse failed: {}", e)))?;
    let pub_bytes = kp.public_key().as_ref();

    // Wrap public key in SubjectPublicKeyInfo DER (for Ed25519 this is a fixed prefix + 32 bytes).
    // OID 1.3.101.112 = id-EdDSA (Ed25519).
    let mut spki = vec![
        0x30, 0x2a, // SEQUENCE, 42 bytes total
        0x30, 0x05, // SEQUENCE (AlgorithmIdentifier), 5 bytes
        0x06, 0x03, 0x2b, 0x65, 0x70, // OID 1.3.101.112
        0x03, 0x21, 0x00, // BIT STRING, 33 bytes, 0 unused bits
    ];
    spki.extend_from_slice(pub_bytes);
    let public_pem = pem::encode(&pem::Pem::new("PUBLIC KEY", spki));

    // Write key files with restrictive permissions set BEFORE writing content
    // to avoid a window where the file exists with default (world-readable) mode.
    #[cfg(unix)]
    {
        use std::fs::OpenOptions;
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        use std::os::unix::fs::PermissionsExt;

        let mut f = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(private_key_path())?;
        f.write_all(private_pem.as_bytes())?;

        let mut f = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o644)
            .open(public_key_path())?;
        f.write_all(public_pem.as_bytes())?;

        let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
    }

    #[cfg(not(unix))]
    {
        fs::write(private_key_path(), &private_pem)?;
        fs::write(public_key_path(), &public_pem)?;
    }

    Ok((private_pem.into_bytes(), public_pem.into_bytes()))
}

/// Load the Ed25519 keypair from disk. Returns (private_pem, public_pem).
pub fn load_keypair() -> Result<(Vec<u8>, Vec<u8>), AuthError> {
    let priv_path = private_key_path();
    let pub_path = public_key_path();

    if !priv_path.exists() || !pub_path.exists() {
        return Err(AuthError::NoKeypair);
    }

    let private_pem = fs::read(&priv_path)?;
    let public_pem = fs::read(&pub_path)?;
    Ok((private_pem, public_pem))
}

/// Load or generate the keypair. Generates if missing.
pub fn load_or_generate_keypair() -> Result<(Vec<u8>, Vec<u8>), AuthError> {
    match load_keypair() {
        Ok(kp) => Ok(kp),
        Err(AuthError::NoKeypair) => generate_keypair(),
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------
// JWT encode / decode
// ---------------------------------------------------------------------------

/// Mint a new access token.
pub fn mint_access_token(
    private_pem: &[u8],
    user_id: i64,
    username: &str,
    role: Role,
    ttl_secs: u64,
) -> Result<String, AuthError> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let claims = Claims {
        sub: user_id,
        username: username.to_string(),
        role: role.as_str().to_string(),
        iat: now,
        exp: now + ttl_secs,
    };

    let key = EncodingKey::from_ed_pem(private_pem)?;
    let header = Header::new(Algorithm::EdDSA);
    let token = jsonwebtoken::encode(&header, &claims, &key)?;
    Ok(token)
}

/// Validate an access token and return its claims.
pub fn validate_access_token(public_pem: &[u8], token: &str) -> Result<Claims, AuthError> {
    let key = DecodingKey::from_ed_pem(public_pem)?;
    let mut validation = Validation::new(Algorithm::EdDSA);
    // Only require exp (expiry). sub and iat are custom fields, not JWT spec strings.
    validation.set_required_spec_claims(&["exp"]);

    let data = jsonwebtoken::decode::<Claims>(token, &key, &validation)?;
    Ok(data.claims)
}

// ---------------------------------------------------------------------------
// Time helpers
// ---------------------------------------------------------------------------

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Parse a duration string like "15m", "7d", "24h", "3600s" into seconds.
pub fn parse_duration_secs(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }

    let (num_str, multiplier) = if let Some(n) = s.strip_suffix('d') {
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

    let num: u64 = num_str.parse().ok()?;
    Some(num * multiplier)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_hash_and_verify() {
        let password = "hunter2";
        let hash = hash_password(password).unwrap();
        assert!(hash.starts_with("$argon2"));
        verify_password(password, &hash).unwrap();
    }

    #[test]
    fn password_verify_wrong() {
        let hash = hash_password("correct").unwrap();
        let result = verify_password("wrong", &hash);
        assert!(matches!(result, Err(AuthError::InvalidPassword)));
    }

    #[test]
    fn keypair_generate_and_jwt_roundtrip() {
        let (priv_pem, pub_pem) = generate_keypair().unwrap();

        let token = mint_access_token(&priv_pem, 42, "testuser", Role::Admin, 3600).unwrap();
        let claims = validate_access_token(&pub_pem, &token).unwrap();

        assert_eq!(claims.sub, 42);
        assert_eq!(claims.username, "testuser");
        assert_eq!(claims.role, "admin");
    }

    #[test]
    fn expired_token_rejected() {
        let (priv_pem, pub_pem) = generate_keypair().unwrap();
        // Manually create a token that expired 10 minutes ago.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let claims = Claims {
            sub: 1,
            username: "user".into(),
            role: "user".into(),
            iat: now - 1200,
            exp: now - 600, // expired 10 min ago
        };
        let key = jsonwebtoken::EncodingKey::from_ed_pem(&priv_pem).unwrap();
        let header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::EdDSA);
        let token = jsonwebtoken::encode(&header, &claims, &key).unwrap();
        let result = validate_access_token(&pub_pem, &token);
        assert!(result.is_err());
    }

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
    fn parse_duration() {
        assert_eq!(parse_duration_secs("15m"), Some(900));
        assert_eq!(parse_duration_secs("7d"), Some(604800));
        assert_eq!(parse_duration_secs("24h"), Some(86400));
        assert_eq!(parse_duration_secs("3600s"), Some(3600));
        assert_eq!(parse_duration_secs("3600"), Some(3600));
        assert_eq!(parse_duration_secs(""), None);
    }
}
