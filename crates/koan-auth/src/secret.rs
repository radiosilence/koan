//! Random secrets, and the digest bearer secrets are stored under.

use ring::rand::{SecureRandom, SystemRandom};

use crate::AuthError;

fn fill(bytes: &mut [u8]) -> Result<(), AuthError> {
    SystemRandom::new()
        .fill(bytes)
        .map_err(|_| AuthError::Hash("rng failure".into()))
}

/// `len` characters drawn evenly from `alphabet`, with a dash between groups
/// of `group` when there is one.
fn draw(alphabet: &[u8], len: usize, group: Option<usize>) -> Result<String, AuthError> {
    let mut out = String::with_capacity(len + len / group.unwrap_or(len).max(1));
    let mut byte = [0u8; 1];
    let mut drawn = 0;
    // Rejecting the top of the range keeps every character equally likely.
    let limit = 256 - 256 % alphabet.len();
    while drawn < len {
        fill(&mut byte)?;
        if byte[0] as usize >= limit {
            continue;
        }
        if group.is_some_and(|g| drawn > 0 && drawn % g == 0) {
            out.push('-');
        }
        out.push(alphabet[byte[0] as usize % alphabet.len()] as char);
        drawn += 1;
    }
    Ok(out)
}

/// A 256-bit random secret, hex encoded.
///
/// For bearer secrets compared or looked up verbatim (sessions, an
/// introspection key, a shared secret), so the entropy carries the whole
/// security argument.
pub fn random_token() -> Result<String, AuthError> {
    let mut bytes = [0u8; 32];
    fill(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// A new API key: 32 random bytes, base64url without padding, so it travels in
/// a query string unescaped.
pub fn random_api_key() -> Result<String, AuthError> {
    use base64::Engine as _;

    let mut bytes = [0u8; 32];
    fill(&mut bytes)?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

/// SHA-256 of `input`, hex encoded: what a bearer secret is stored as, so a
/// read of the database yields nothing that signs in.
pub fn sha256_hex(input: &str) -> String {
    ring::digest::digest(&ring::digest::SHA256, input.as_bytes())
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A password someone can type from an email: 20 characters with no
/// lookalikes (0/O, 1/l/I), about 116 bits.
pub fn generate_password() -> Result<String, AuthError> {
    draw(
        b"abcdefghijkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789",
        20,
        None,
    )
}

/// The alphabet of codes typed on a phone or read off a screen: no character
/// a keyboard or a reader confuses with another.
pub(crate) const TYPEABLE: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";

/// A new app password: four groups of five from [`TYPEABLE`], about 98 bits.
/// It is typed into an app once, so it is made to be typed.
pub fn random_app_password() -> Result<String, AuthError> {
    draw(TYPEABLE, 20, Some(5))
}

/// `len` characters of [`TYPEABLE`] in groups of `group`.
pub(crate) fn typeable(len: usize, group: usize) -> Result<String, AuthError> {
    draw(TYPEABLE, len, Some(group))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_passwords_are_typeable() {
        let p = generate_password().unwrap();
        assert_eq!(p.len(), 20);
        assert!(p.chars().all(|c| c.is_ascii_alphanumeric()));
        assert!(!p.contains(['0', 'O', '1', 'l', 'I']));
        assert_ne!(p, generate_password().unwrap());
    }

    #[test]
    fn app_passwords_come_in_four_groups_of_five() {
        let p = random_app_password().unwrap();
        let groups: Vec<&str> = p.split('-').collect();
        assert_eq!(groups.len(), 4);
        assert!(groups.iter().all(|g| g.len() == 5));
        assert!(p.bytes().all(|b| b == b'-' || TYPEABLE.contains(&b)));
    }

    #[test]
    fn tokens_and_keys_are_long_and_fresh() {
        let token = random_token().unwrap();
        assert_eq!(token.len(), 64);
        assert_ne!(token, random_token().unwrap());
        let key = random_api_key().unwrap();
        assert_eq!(key.len(), 43);
        assert!(
            key.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        );
    }

    #[test]
    fn the_digest_is_sha256_in_hex() {
        assert_eq!(
            sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
