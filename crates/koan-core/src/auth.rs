//! Authentication primitives: Ed25519 JWT signing, Argon2id password hashing.
//!
//! Ed25519 keypair is generated once and stored in the config directory.
//! JWTs are signed with EdDSA (Ed25519).

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use ring::signature::KeyPair;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::config;

/// The name a server acts under when nobody signed in: auth switched off, or
/// the local MCP. No account may take it, or it would inherit what is scoped
/// to that name.
pub const ANONYMOUS: &str = "anonymous";

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
    /// The one resource a scoped token is good for; see `MCP_SCOPE`. An
    /// unscoped token is a session and goes anywhere a session does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

/// The scope of a token granted to an MCP client through OAuth: valid at
/// `/mcp` and nowhere else, so the limits the MCP sets on what a client may do
/// cannot be stepped round by presenting the same token to GraphQL.
pub const MCP_SCOPE: &str = "mcp";

// ---------------------------------------------------------------------------
// Password hashing (Argon2id)
// ---------------------------------------------------------------------------

/// Hash a password using Argon2id with a random salt.
pub fn hash_password(password: &str) -> Result<String, AuthError> {
    use argon2::Argon2;
    use argon2::password_hash::PasswordHasher;

    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|h| h.to_string())
        .map_err(|e| AuthError::Hash(e.to_string()))
}

/// Verify a password against an Argon2id hash.
pub fn verify_password(password: &str, hash: &str) -> Result<(), AuthError> {
    use argon2::Argon2;
    use argon2::password_hash::PasswordVerifier;
    use argon2::password_hash::phc::PasswordHash;

    let parsed = PasswordHash::new(hash).map_err(|e| AuthError::Hash(e.to_string()))?;
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .map_err(|_| AuthError::InvalidPassword)
}

// ---------------------------------------------------------------------------
// Random secrets
// ---------------------------------------------------------------------------

/// Generate a 256-bit random secret, hex encoded.
///
/// Used for bearer-style secrets that are compared verbatim rather than hashed
/// (introspection key, Subsonic shared secret), so the entropy has to carry the
/// whole security argument.
pub fn random_token() -> Result<String, AuthError> {
    use ring::rand::SecureRandom;

    let mut bytes = [0u8; 32];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| AuthError::Hash("rng failure".into()))?;
    Ok(bytes.iter().map(|b| format!("{:02x}", b)).collect())
}

/// A new Subsonic API key: 32 random bytes, base64url without padding, so it
/// travels in a query string unescaped.
pub fn random_api_key() -> Result<String, AuthError> {
    use base64::Engine as _;
    use ring::rand::SecureRandom;

    let mut bytes = [0u8; 32];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| AuthError::Hash("rng failure".into()))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

/// SHA-256 of `input`, hex encoded. Refresh tokens are stored under this so a
/// database read does not yield usable credentials.
pub fn sha256_hex(input: &str) -> String {
    ring::digest::digest(&ring::digest::SHA256, input.as_bytes())
        .as_ref()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect()
}

/// The key Subsonic app passwords are sealed under, derived from the server's
/// signing key. A copy of the database alone does not open them, and
/// regenerating the keypair retires every app password along with every
/// session.
pub fn app_password_key(private_pem: &[u8]) -> [u8; 32] {
    use ring::hkdf;

    let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, b"koan").extract(private_pem);
    let mut key = [0u8; 32];
    prk.expand(&[b"subsonic app passwords v1"], hkdf::HKDF_SHA256)
        .and_then(|okm| okm.fill(&mut key))
        .expect("HKDF-SHA256 yields 32 bytes");
    key
}

/// Seal an app password for `user_id`: a random nonce, then the ciphertext and
/// its tag. The user id is authenticated with it, so a sealed password moved to
/// another account's row does not open.
pub fn seal_app_password(
    key: &[u8; 32],
    user_id: i64,
    password: &str,
) -> Result<Vec<u8>, AuthError> {
    use ring::aead::{Aad, CHACHA20_POLY1305, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
    use ring::rand::SecureRandom;

    let sealing = LessSafeKey::new(
        UnboundKey::new(&CHACHA20_POLY1305, key).map_err(|_| AuthError::Hash("bad key".into()))?,
    );
    let mut nonce = [0u8; NONCE_LEN];
    ring::rand::SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| AuthError::Hash("rng failure".into()))?;
    let mut sealed = password.as_bytes().to_vec();
    sealing
        .seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(user_id.to_le_bytes()),
            &mut sealed,
        )
        .map_err(|_| AuthError::Hash("seal failure".into()))?;
    let mut out = nonce.to_vec();
    out.extend_from_slice(&sealed);
    Ok(out)
}

/// The app password `seal_app_password` sealed, if `key` and `user_id` are the
/// ones it was sealed with.
pub fn open_app_password(key: &[u8; 32], user_id: i64, sealed: &[u8]) -> Option<String> {
    use ring::aead::{Aad, CHACHA20_POLY1305, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};

    let opening = LessSafeKey::new(UnboundKey::new(&CHACHA20_POLY1305, key).ok()?);
    let (nonce, ciphertext) = sealed.split_at_checked(NONCE_LEN)?;
    let mut buf = ciphertext.to_vec();
    let plain = opening
        .open_in_place(
            Nonce::try_assume_unique_for_key(nonce).ok()?,
            Aad::from(user_id.to_le_bytes()),
            &mut buf,
        )
        .ok()?;
    String::from_utf8(plain.to_vec()).ok()
}

/// A new app password: four groups of five from an alphabet without the
/// characters a phone keyboard or a reader confuses, about 98 bits. It is
/// typed into an app once, so it is made to be typed.
pub fn random_app_password() -> Result<String, AuthError> {
    use ring::rand::SecureRandom;

    const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    let rng = ring::rand::SystemRandom::new();
    let mut out = String::with_capacity(23);
    let mut byte = [0u8; 1];
    let mut drawn = 0;
    while drawn < 20 {
        rng.fill(&mut byte)
            .map_err(|_| AuthError::Hash("rng failure".into()))?;
        // Rejecting the top of the range keeps every character equally likely.
        if byte[0] as usize >= ALPHABET.len() * (256 / ALPHABET.len()) {
            continue;
        }
        if drawn > 0 && drawn % 5 == 0 {
            out.push('-');
        }
        out.push(ALPHABET[byte[0] as usize % ALPHABET.len()] as char);
        drawn += 1;
    }
    Ok(out)
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

/// Derive a new Ed25519 keypair as PEM. Touches no filesystem state.
/// Returns (private_pem, public_pem).
pub fn generate_keypair_pem() -> Result<(String, String), AuthError> {
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

    Ok((private_pem, public_pem))
}

/// Generate a new Ed25519 keypair and write PEM files to the config dir.
/// Returns (private_pem, public_pem).
pub fn generate_keypair() -> Result<(Vec<u8>, Vec<u8>), AuthError> {
    let (private_pem, public_pem) = generate_keypair_pem()?;

    let dir = keypair_dir();
    fs::create_dir_all(&dir)?;

    // Ensure the auth directory is gitignored — keys must never be committed.
    let gitignore = dir.join(".gitignore");
    if !gitignore.exists() {
        let _ = fs::write(&gitignore, "*\n");
    }

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
    mint_scoped_token(private_pem, user_id, username, role, ttl_secs, None)
}

/// Mint an access token, limited to `scope` when there is one.
pub fn mint_scoped_token(
    private_pem: &[u8],
    user_id: i64,
    username: &str,
    role: Role,
    ttl_secs: u64,
    scope: Option<&str>,
) -> Result<String, AuthError> {
    let now = now_unix();

    let claims = Claims {
        sub: user_id,
        username: username.to_string(),
        role: role.as_str().to_string(),
        iat: now,
        exp: now + ttl_secs,
        scope: scope.map(str::to_owned),
    };

    let key = EncodingKey::from_ed_pem(private_pem)?;
    let header = Header::new(Algorithm::EdDSA);
    let token = jsonwebtoken::encode(&header, &claims, &key)?;
    Ok(token)
}

/// Validate a session's access token and return its claims. A scoped token is
/// refused: it is good only where its scope is checked for.
pub fn validate_access_token(public_pem: &[u8], token: &str) -> Result<Claims, AuthError> {
    validate_scoped_token(public_pem, token, None)
}

/// Validate an access token whose scope is exactly `scope`.
pub fn validate_scoped_token(
    public_pem: &[u8],
    token: &str,
    scope: Option<&str>,
) -> Result<Claims, AuthError> {
    let key = DecodingKey::from_ed_pem(public_pem)?;
    let mut validation = Validation::new(Algorithm::EdDSA);
    // Only require exp (expiry). sub and iat are custom fields, not JWT spec strings.
    validation.set_required_spec_claims(&["exp"]);

    let claims = jsonwebtoken::decode::<Claims>(token, &key, &validation)?.claims;
    if claims.scope.as_deref() != scope {
        return Err(AuthError::Other("token not valid here".into()));
    }
    Ok(claims)
}

// ---------------------------------------------------------------------------
// Account changes
// ---------------------------------------------------------------------------

/// Changes to accounts, as a running count, and the count at each account's
/// latest change.
///
/// A request is authenticated once, but a socket outlives it and asks the
/// database nothing afterwards. So every change that can narrow what an
/// account's sockets hold — a new role or password, a revoked key or app
/// password, the account deleted — is announced here, and a socket closes when
/// its account's is. The client reconnects and is authenticated as things now
/// stand.
///
/// Revoking a refresh token is not such a change: no socket rests on one, and
/// closing every socket on the account at each sign-out would only make them
/// all reconnect.
///
/// Announced by the queries that make the change, so no caller can forget to.
/// Only within this process: a change made by another, such as the CLI's,
/// reaches sockets when they next reconnect.
struct AccountChanges {
    count: tokio::sync::watch::Sender<u64>,
    latest: parking_lot::Mutex<std::collections::HashMap<i64, u64>>,
}

fn account_changes() -> &'static AccountChanges {
    static CHANGES: std::sync::OnceLock<AccountChanges> = std::sync::OnceLock::new();
    CHANGES.get_or_init(|| AccountChanges {
        count: tokio::sync::watch::Sender::new(0),
        latest: Default::default(),
    })
}

/// Announce a change to `user_id`'s account.
pub fn account_changed(user_id: i64) {
    let changes = account_changes();
    changes.count.send_modify(|count| {
        *count += 1;
        changes.latest.lock().insert(user_id, *count);
    });
}

/// Where the count of changes stands. Taken before a credential is checked,
/// so a change made while it is being checked still counts against it.
pub fn account_mark() -> u64 {
    *account_changes().count.borrow()
}

/// Resolves once `user_id`'s account has changed after `mark`.
pub async fn account_changed_since(user_id: i64, mark: u64) {
    let changes = account_changes();
    let mut count = changes.count.subscribe();
    loop {
        if changes
            .latest
            .lock()
            .get(&user_id)
            .is_some_and(|&at| at > mark)
        {
            return;
        }
        if count.changed().await.is_err() {
            return std::future::pending().await;
        }
    }
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
    fn a_scoped_token_is_good_only_where_its_scope_is_asked_for() {
        let (private, public) = generate_keypair_pem().unwrap();
        let scoped = mint_scoped_token(
            private.as_bytes(),
            1,
            "user",
            Role::Admin,
            900,
            Some(MCP_SCOPE),
        )
        .unwrap();
        assert!(validate_access_token(public.as_bytes(), &scoped).is_err());
        assert!(validate_scoped_token(public.as_bytes(), &scoped, Some(MCP_SCOPE)).is_ok());
        let session = mint_access_token(private.as_bytes(), 1, "user", Role::Admin, 900).unwrap();
        assert!(validate_scoped_token(public.as_bytes(), &session, Some(MCP_SCOPE)).is_err());
    }

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

    /// Hashed by argon2 0.5. Every stored password was, so this is what a
    /// dependency bump must never stop accepting.
    #[test]
    fn password_verify_hash_from_argon2_0_5() {
        let hash = "$argon2id$v=19$m=19456,t=2,p=1$M/zwWdjjbwOvNCjzP+5t5A$pflXrbL1iOYPBlbgtK59wr2PkBaH7UVLKoBisvJ+Yfk";
        verify_password("correct horse", hash).unwrap();
        assert!(matches!(
            verify_password("wrong horse", hash),
            Err(AuthError::InvalidPassword)
        ));
    }

    #[test]
    fn keypair_generate_and_jwt_roundtrip() {
        let (priv_pem, pub_pem) = generate_keypair_pem().unwrap();

        let token =
            mint_access_token(priv_pem.as_bytes(), 42, "testuser", Role::Admin, 3600).unwrap();
        let claims = validate_access_token(pub_pem.as_bytes(), &token).unwrap();

        assert_eq!(claims.sub, 42);
        assert_eq!(claims.username, "testuser");
        assert_eq!(claims.role, "admin");
    }

    #[test]
    fn expired_token_rejected() {
        let (priv_pem, pub_pem) = generate_keypair_pem().unwrap();
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
            scope: None,
        };
        let key = jsonwebtoken::EncodingKey::from_ed_pem(priv_pem.as_bytes()).unwrap();
        let header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::EdDSA);
        let token = jsonwebtoken::encode(&header, &claims, &key).unwrap();
        let result = validate_access_token(pub_pem.as_bytes(), &token);
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
