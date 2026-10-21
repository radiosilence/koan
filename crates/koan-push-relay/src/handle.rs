//! Handles: a device token sealed so that only this relay can read it.
//!
//! The app gives servers a handle instead of its token, so a server can reach
//! the device only through the relay, and the relay keeps no record of
//! devices: everything it needs to push is inside the handle.
//!
//! Layout, base64url without padding:
//!
//! ```text
//! version (1) | key id (1) | nonce (12) | AES-256-GCM-SIV( flags (1) | minted ms (8, BE) | token ) | tag (16)
//! ```
//!
//! The version and key id are authenticated as associated data. GCM-SIV
//! because the nonces are random: a repeated nonce costs it nothing beyond
//! revealing that two handles are identical.

use std::collections::HashMap;

use aws_lc_rs::aead::{AES_256_GCM_SIV, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use aws_lc_rs::hmac;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

const VERSION: u8 = 1;
const TAG_LEN: usize = 16;
const HEADER_LEN: usize = 2 + NONCE_LEN;
const FIXED_LEN: usize = 1 + 8;
const SANDBOX: u8 = 1;

/// The shortest and longest device token accepted, in bytes. Apple's are 32
/// today and documented as variable in length.
pub const MIN_TOKEN: usize = 8;
pub const MAX_TOKEN: usize = 100;

/// The longest handle, encoded, so a request can be refused before decoding.
pub const MAX_HANDLE_LEN: usize = (HEADER_LEN + FIXED_LEN + MAX_TOKEN + TAG_LEN).div_ceil(3) * 4;

/// The longest a handle may be configured to open for. Revocations are kept
/// this long, so changing the configured lifetime never lets a revoked handle
/// open again.
pub const MAX_LIFETIME_MS: u64 = 365 * 24 * 60 * 60 * 1000;

/// What a handle holds.
#[derive(Clone, PartialEq, Eq)]
pub struct Device {
    pub token: Vec<u8>,
    pub sandbox: bool,
    pub minted_ms: u64,
    /// The handle's nonce, random at sealing: what tells one of a device's
    /// handles from another.
    pub id: [u8; NONCE_LEN],
}

/// Never shows the token: a `{:?}` in a log line must not leak one.
impl std::fmt::Debug for Device {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Device")
            .field("sandbox", &self.sandbox)
            .field("minted_ms", &self.minted_ms)
            .finish_non_exhaustive()
    }
}

/// Why a handle does not open. Callers are told only that it is gone.
#[derive(Debug, PartialEq, Eq)]
pub enum Invalid {
    Malformed,
    UnknownKey,
    Forged,
    Expired,
}

pub struct Handles {
    keys: HashMap<u8, LessSafeKey>,
    current: u8,
    lifetime_ms: u64,
}

impl Handles {
    pub fn new(keys: &[(u8, [u8; 32])], current: u8, lifetime_ms: u64) -> Result<Self, String> {
        let mut map = HashMap::new();
        for (id, key) in keys {
            if *id == 0 {
                return Err("handle key ids start at 1".into());
            }
            let unbound =
                UnboundKey::new(&AES_256_GCM_SIV, key).map_err(|_| "bad handle key".to_string())?;
            if map.insert(*id, LessSafeKey::new(unbound)).is_some() {
                return Err(format!("handle key {id} is listed twice"));
            }
        }
        if !map.contains_key(&current) {
            return Err(format!(
                "handle key {current}, the current one, is not listed"
            ));
        }
        Ok(Self {
            keys: map,
            current,
            lifetime_ms,
        })
    }

    pub fn lifetime_ms(&self) -> u64 {
        self.lifetime_ms
    }

    /// A handle for `token`, minted at `now_ms`.
    pub fn seal(&self, token: &[u8], sandbox: bool, now_ms: u64) -> String {
        let mut nonce = [0u8; NONCE_LEN];
        aws_lc_rs::rand::fill(&mut nonce).expect("the system's random source failed");
        let header = [VERSION, self.current];

        let mut body = Vec::with_capacity(FIXED_LEN + token.len() + TAG_LEN);
        body.push(if sandbox { SANDBOX } else { 0 });
        body.extend_from_slice(&now_ms.to_be_bytes());
        body.extend_from_slice(token);
        self.keys[&self.current]
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(header),
                &mut body,
            )
            .expect("sealing a handle cannot fail at this size");

        let mut out = Vec::with_capacity(HEADER_LEN + body.len());
        out.extend_from_slice(&header);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&body);
        URL_SAFE_NO_PAD.encode(out)
    }

    /// What `handle` holds, if this relay sealed it and it has not expired.
    pub fn open(&self, handle: &str, now_ms: u64) -> Result<Device, Invalid> {
        if handle.len() > MAX_HANDLE_LEN {
            return Err(Invalid::Malformed);
        }
        let mut raw = URL_SAFE_NO_PAD
            .decode(handle)
            .map_err(|_| Invalid::Malformed)?;
        if raw.len() < HEADER_LEN + FIXED_LEN + MIN_TOKEN + TAG_LEN || raw[0] != VERSION {
            return Err(Invalid::Malformed);
        }
        let header = [raw[0], raw[1]];
        let key = self.keys.get(&header[1]).ok_or(Invalid::UnknownKey)?;
        let id: [u8; NONCE_LEN] = raw[2..HEADER_LEN].try_into().expect("nonce length");
        let nonce = Nonce::assume_unique_for_key(id);
        let plain = key
            .open_in_place(nonce, Aad::from(header), &mut raw[HEADER_LEN..])
            .map_err(|_| Invalid::Forged)?;

        let sandbox = plain[0] & SANDBOX != 0;
        let minted_ms = u64::from_be_bytes(plain[1..FIXED_LEN].try_into().expect("eight bytes"));
        let token = plain[FIXED_LEN..].to_vec();
        // A mint time somewhat ahead of the clock is no forgery, since only
        // the relay seals: the clock was stepped back, or the handle was
        // minted past a revocation's window. One a whole lifetime ahead was
        // never minted here, and would outlive expiry and revocation alike.
        if minted_ms > now_ms.saturating_add(self.lifetime_ms)
            || now_ms >= minted_ms.saturating_add(self.lifetime_ms)
        {
            return Err(Invalid::Expired);
        }
        Ok(Device {
            token,
            sandbox,
            minted_ms,
            id,
        })
    }
}

/// What stands for a device token wherever the relay keeps or counts
/// something: a keyed hash, so a table of them cannot be checked against a
/// token someone already holds without the key.
pub struct TokenHasher(hmac::Key);

pub type TokenHash = [u8; 16];

impl TokenHasher {
    pub fn new(key: &[u8; 32]) -> Self {
        Self(hmac::Key::new(hmac::HMAC_SHA256, key))
    }

    pub fn hash(&self, token: &[u8]) -> TokenHash {
        let tag = hmac::sign(&self.0, token);
        tag.as_ref()[..16].try_into().expect("sixteen bytes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u64 = 86_400_000;

    fn handles() -> Handles {
        Handles::new(&[(1, [7; 32]), (2, [9; 32])], 2, 60 * DAY).unwrap()
    }

    #[test]
    fn a_handle_opens_to_what_was_sealed() {
        let h = handles();
        let token = [0xab; 32];
        let sealed = h.seal(&token, true, 1_000);
        assert!(sealed.len() <= MAX_HANDLE_LEN);
        let device = h.open(&sealed, 2_000).unwrap();
        assert_eq!(device.token, token);
        assert!(device.sandbox);
        assert_eq!(device.minted_ms, 1_000);
        assert_ne!(
            device.id,
            h.open(&h.seal(&token, true, 1_000), 2_000).unwrap().id
        );
    }

    #[test]
    fn two_handles_for_one_token_differ() {
        let h = handles();
        assert_ne!(h.seal(&[1; 32], false, 0), h.seal(&[1; 32], false, 0));
    }

    #[test]
    fn every_altered_byte_is_refused() {
        let h = handles();
        let sealed = h.seal(&[3; 32], false, 0);
        let raw = URL_SAFE_NO_PAD.decode(&sealed).unwrap();
        for i in 0..raw.len() {
            let mut bad = raw.clone();
            bad[i] ^= 0x01;
            assert!(
                h.open(&URL_SAFE_NO_PAD.encode(&bad), 1).is_err(),
                "byte {i} altered and still opened"
            );
        }
    }

    #[test]
    fn a_retired_key_opens_nothing_and_an_old_one_still_opens() {
        let old = Handles::new(&[(1, [7; 32])], 1, 60 * DAY).unwrap();
        let sealed = old.seal(&[4; 32], false, 0);
        assert!(handles().open(&sealed, 1).is_ok());
        let retired = Handles::new(&[(2, [9; 32])], 2, 60 * DAY).unwrap();
        assert_eq!(retired.open(&sealed, 1), Err(Invalid::UnknownKey));
    }

    #[test]
    fn another_relays_key_is_a_forgery() {
        let other = Handles::new(&[(2, [1; 32])], 2, 60 * DAY).unwrap();
        let sealed = other.seal(&[4; 32], false, 0);
        assert_eq!(handles().open(&sealed, 1), Err(Invalid::Forged));
    }

    #[test]
    fn a_handle_expires_after_its_lifetime() {
        let h = handles();
        let sealed = h.seal(&[5; 32], false, 10 * DAY);
        assert!(h.open(&sealed, 70 * DAY - 1).is_ok());
        assert_eq!(h.open(&sealed, 70 * DAY), Err(Invalid::Expired));
    }

    #[test]
    fn a_handle_minted_ahead_of_the_clock_opens_up_to_a_lifetime_ahead() {
        let h = handles();
        let sealed = h.seal(&[5; 32], false, 10 * DAY);
        assert!(h.open(&sealed, DAY).is_ok());
        let far = h.seal(&[5; 32], false, 100 * DAY);
        assert_eq!(h.open(&far, DAY), Err(Invalid::Expired));
    }

    #[test]
    fn a_device_never_prints_its_token() {
        let h = handles();
        let device = h.open(&h.seal(&[0xab; 32], false, 0), 0).unwrap();
        assert!(!format!("{device:?}").contains("171"));
        assert!(!format!("{device:?}").to_lowercase().contains("ab, ab"));
    }

    #[test]
    fn noise_never_opens_and_never_panics() {
        let h = handles();
        let mut seed = [0u8; 4096];
        aws_lc_rs::rand::fill(&mut seed).unwrap();
        for len in 0..seed.len().min(400) {
            let bytes = &seed[..len];
            assert!(h.open(&URL_SAFE_NO_PAD.encode(bytes), 0).is_err());
            assert!(h.open(&String::from_utf8_lossy(bytes), 0).is_err());
        }
        let mut versioned = seed[..100].to_vec();
        versioned[0] = VERSION;
        versioned[1] = 2;
        assert_eq!(
            h.open(&URL_SAFE_NO_PAD.encode(&versioned), 0),
            Err(Invalid::Forged)
        );
        assert_eq!(
            h.open(&"A".repeat(MAX_HANDLE_LEN + 1), 0),
            Err(Invalid::Malformed)
        );
    }

    #[test]
    fn the_longest_token_fits_the_longest_handle() {
        let h = handles();
        let sealed = h.seal(&[6; MAX_TOKEN], false, 0);
        assert!(sealed.len() <= MAX_HANDLE_LEN);
        assert!(h.open(&sealed, 0).is_ok());
    }

    #[test]
    fn a_key_id_of_zero_or_a_missing_current_key_is_refused() {
        assert!(Handles::new(&[(0, [1; 32])], 0, DAY).is_err());
        assert!(Handles::new(&[(1, [1; 32])], 2, DAY).is_err());
    }

    #[test]
    fn token_hashes_depend_on_the_key() {
        let a = TokenHasher::new(&[1; 32]);
        let b = TokenHasher::new(&[2; 32]);
        assert_eq!(a.hash(b"token"), a.hash(b"token"));
        assert_ne!(a.hash(b"token"), b.hash(b"token"));
        assert_ne!(a.hash(b"token"), a.hash(b"other"));
    }
}
