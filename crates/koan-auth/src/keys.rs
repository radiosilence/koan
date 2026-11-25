//! The Ed25519 keypair a server signs its tokens with, as PEM: PKCS#8 for the
//! private key, SubjectPublicKeyInfo for the public one.
//!
//! A server that keeps its key on disk keeps it in a directory of its own
//! ([`load_or_generate`]); one given its key as a secret derives the public
//! half from it ([`public_from_private`]).

use std::fs;
use std::path::{Path, PathBuf};

use ring::signature::KeyPair;

use crate::AuthError;

const PRIVATE: &str = "ed25519.pem";
const PUBLIC: &str = "ed25519.pub.pem";

fn private_key_path(dir: &Path) -> PathBuf {
    dir.join(PRIVATE)
}

fn public_key_path(dir: &Path) -> PathBuf {
    dir.join(PUBLIC)
}

/// The SubjectPublicKeyInfo PEM of an Ed25519 public key.
fn public_pem(public_key: &[u8]) -> String {
    // OID 1.3.101.112 is id-EdDSA (Ed25519); the DER is a fixed prefix and
    // the 32 bytes of the key.
    let mut spki = vec![
        0x30, 0x2a, // SEQUENCE, 42 bytes
        0x30, 0x05, // SEQUENCE (AlgorithmIdentifier), 5 bytes
        0x06, 0x03, 0x2b, 0x65, 0x70, // OID 1.3.101.112
        0x03, 0x21, 0x00, // BIT STRING, 33 bytes, no unused bits
    ];
    spki.extend_from_slice(public_key);
    pem::encode(&pem::Pem::new("PUBLIC KEY", spki))
}

/// A new keypair as PEM, private then public. Touches no file.
pub fn generate_pem() -> Result<(String, String), AuthError> {
    let rng = ring::rand::SystemRandom::new();
    let pkcs8 = ring::signature::Ed25519KeyPair::generate_pkcs8(&rng)
        .map_err(|e| AuthError::Other(format!("keypair generation failed: {e}")))?;
    let private = pem::encode(&pem::Pem::new("PRIVATE KEY", pkcs8.as_ref()));
    let public = public_from_private(private.as_bytes())?;
    Ok((private, public))
}

/// The public key PEM of a private key PEM: PKCS#8, with or without the
/// public key inside it, as `openssl genpkey` and most key generators write
/// one.
pub fn public_from_private(private_pem: &[u8]) -> Result<String, AuthError> {
    let parsed = pem::parse(private_pem)
        .map_err(|e| AuthError::Other(format!("the private key is not PEM: {e}")))?;
    let pair = ring::signature::Ed25519KeyPair::from_pkcs8_maybe_unchecked(parsed.contents())
        .map_err(|e| AuthError::Other(format!("the private key is not an Ed25519 key: {e}")))?;
    Ok(public_pem(pair.public_key().as_ref()))
}

/// Make a new keypair and write it to `dir`, the private key readable by its
/// owner alone. Returns the PEM, private then public.
pub fn generate(dir: &Path) -> Result<(Vec<u8>, Vec<u8>), AuthError> {
    let (private_pem, public_pem) = generate_pem()?;

    fs::create_dir_all(dir)?;

    // The directory is never committed, wherever it lies.
    let gitignore = dir.join(".gitignore");
    if !gitignore.exists() {
        let _ = fs::write(&gitignore, "*\n");
    }

    // The mode is set as the file is made, so there is no moment when it
    // exists readable by others.
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
            .open(private_key_path(dir))?;
        f.write_all(private_pem.as_bytes())?;

        let mut f = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o644)
            .open(public_key_path(dir))?;
        f.write_all(public_pem.as_bytes())?;

        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    }

    #[cfg(not(unix))]
    {
        fs::write(private_key_path(dir), &private_pem)?;
        fs::write(public_key_path(dir), &public_pem)?;
    }

    Ok((private_pem.into_bytes(), public_pem.into_bytes()))
}

/// The keypair in `dir`, private then public. A private key whose public
/// half is missing has it derived and written back: making a new pair in its
/// place would end every session and invite it signed.
pub fn load(dir: &Path) -> Result<(Vec<u8>, Vec<u8>), AuthError> {
    let (private, public) = (private_key_path(dir), public_key_path(dir));
    if !private.exists() {
        return Err(AuthError::NoKeypair);
    }
    let private = fs::read(private)?;
    if !public.exists() {
        let derived = public_from_private(&private)?;
        fs::write(&public, &derived)?;
        return Ok((private, derived.into_bytes()));
    }
    Ok((private, fs::read(public)?))
}

/// The keypair in `dir`, made there first if there is none.
pub fn load_or_generate(dir: &Path) -> Result<(Vec<u8>, Vec<u8>), AuthError> {
    match load(dir) {
        Ok(pair) => Ok(pair),
        Err(AuthError::NoKeypair) => generate(dir),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `openssl genpkey -algorithm ed25519`: PKCS#8 v1, with no public key
    /// inside, as most generators write one.
    const OPENSSL: &str = "-----BEGIN PRIVATE KEY-----
MC4CAQAwBQYDK2VwBCIEIN4okzaVFYllZz7zbIu3b7smTH+sqY27i7X3MuoWRfKw
-----END PRIVATE KEY-----
";

    #[test]
    fn the_public_half_of_a_generated_key_is_its_own() {
        let (private, public) = generate_pem().unwrap();
        assert_eq!(public_from_private(private.as_bytes()).unwrap(), public);
        assert!(public.starts_with("-----BEGIN PUBLIC KEY-----"));
    }

    #[test]
    fn a_key_without_its_public_half_still_signs() {
        let public = public_from_private(OPENSSL.as_bytes()).unwrap();
        let token =
            crate::token::mint_access_token(OPENSSL.as_bytes(), 7, "ann", crate::Role::User, 60)
                .unwrap();
        let claims = crate::token::validate_access_token(public.as_bytes(), &token).unwrap();
        assert_eq!(claims.sub, 7);
    }

    #[test]
    fn anything_else_is_refused() {
        assert!(public_from_private(b"not a key").is_err());
        let rsa = "-----BEGIN PRIVATE KEY-----\nMAA=\n-----END PRIVATE KEY-----\n";
        assert!(public_from_private(rsa.as_bytes()).is_err());
    }

    #[test]
    fn a_keypair_is_written_once_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let auth = dir.path().join("auth");
        assert!(matches!(load(&auth), Err(AuthError::NoKeypair)));
        let made = load_or_generate(&auth).unwrap();
        assert_eq!(load_or_generate(&auth).unwrap(), made);
        // A lost public half is derived again, never the pair remade.
        fs::remove_file(auth.join(PUBLIC)).unwrap();
        assert_eq!(load_or_generate(&auth).unwrap(), made);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(auth.join(PRIVATE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
