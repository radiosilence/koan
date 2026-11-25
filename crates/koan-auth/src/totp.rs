//! Time-based one-time passwords, as RFC 6238 (over RFC 4226's HOTP) has them
//! and every authenticator app accepts them: HMAC-SHA-1, six digits, a
//! 30-second step.
//!
//! A code is accepted one step either side of now, for a clock that drifts or
//! a code typed as it turned over. A step already used is refused, as RFC 6238
//! §5.2 requires; remembering it is [`crate::second_factor`]'s.

use ring::hmac;

/// Seconds a code lasts.
pub const STEP_SECS: i64 = 30;

/// Digits in a code.
pub const DIGITS: u32 = 6;

/// Steps either side of now a code is accepted in.
const DRIFT: i64 = 1;

/// The bytes of a new secret: 160 bits, the length RFC 4226 recommends.
pub const SECRET_LEN: usize = 20;

/// The step `unix_secs` falls in.
pub fn step(unix_secs: i64) -> i64 {
    unix_secs.div_euclid(STEP_SECS)
}

/// HOTP (RFC 4226 §5.3): the code for `counter` under `secret`.
pub fn hotp(secret: &[u8], counter: u64) -> u32 {
    let key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, secret);
    let mac = hmac::sign(&key, &counter.to_be_bytes());
    let mac = mac.as_ref();
    let at = (mac[mac.len() - 1] & 0x0f) as usize;
    let truncated = u32::from_be_bytes([mac[at] & 0x7f, mac[at + 1], mac[at + 2], mac[at + 3]]);
    truncated % 10u32.pow(DIGITS)
}

/// The step within the drift `code` is the code for, if any, newest first so
/// that the step remembered as used is the latest one the code could be.
pub fn matching_step(secret: &[u8], code: &str, unix_secs: i64) -> Option<i64> {
    let code = code.trim();
    if code.len() != DIGITS as usize || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let given: u32 = code.parse().ok()?;
    let now = step(unix_secs);
    // Every step is computed, matched or not, so the time taken does not say
    // which one matched.
    let mut found = None;
    for s in (now - DRIFT..=now + DRIFT).rev() {
        let Ok(counter) = u64::try_from(s) else {
            continue;
        };
        let expected = hotp(secret, counter);
        if bool::from(subtle::ConstantTimeEq::ct_eq(
            &expected.to_be_bytes()[..],
            &given.to_be_bytes()[..],
        )) && found.is_none()
        {
            found = Some(s);
        }
    }
    found
}

/// RFC 4648 base32 without padding: how a secret is shown to be typed and
/// carried in a Key URI.
pub fn base32(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut out = String::with_capacity(bytes.len().div_ceil(5) * 8);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for &b in bytes {
        buffer = (buffer << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((buffer >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

/// [`base32`] read back, ignoring case, spaces and padding.
#[cfg(test)]
fn from_base32(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 5 / 8);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for c in text.bytes().filter(|c| !matches!(c, b' ' | b'=' | b'-')) {
        let v = match c.to_ascii_uppercase() {
            c @ b'A'..=b'Z' => c - b'A',
            c @ b'2'..=b'7' => c - b'2' + 26,
            _ => return None,
        };
        buffer = (buffer << 5) | v as u32;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Some(out)
}

/// The Key URI an authenticator app reads from a QR code
/// (`otpauth://totp/Issuer:account?secret=…`). `issuer` names the service, as
/// the app lists it; `account` the account on it. Neither may hold a colon,
/// which separates them in the label, so any is dropped.
pub fn key_uri(issuer: &str, account: &str, secret: &[u8]) -> String {
    let enc = |s: &str| {
        url::form_urlencoded::byte_serialize(s.replace(':', "").as_bytes())
            .collect::<String>()
            .replace('+', "%20")
    };
    format!(
        "otpauth://totp/{}:{}?secret={}&issuer={}&algorithm=SHA1&digits={DIGITS}&period={STEP_SECS}",
        enc(issuer),
        enc(account),
        base32(secret),
        enc(issuer),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const RFC_SECRET: &[u8] = b"12345678901234567890";

    #[test]
    fn hotp_matches_rfc_4226() {
        let expected = [
            755224, 287082, 359152, 969429, 338314, 254676, 287922, 162583, 399871, 520489,
        ];
        for (counter, code) in expected.into_iter().enumerate() {
            assert_eq!(hotp(RFC_SECRET, counter as u64), code);
        }
    }

    #[test]
    fn totp_matches_rfc_6238() {
        // Appendix B's SHA-1 column, cut to six digits.
        for (time, code) in [
            (59, "287082"),
            (1111111109, "081804"),
            (1111111111, "050471"),
            (1234567890, "005924"),
            (2000000000, "279037"),
        ] {
            assert_eq!(matching_step(RFC_SECRET, code, time), Some(step(time)));
        }
    }

    #[test]
    fn a_code_is_good_one_step_either_side_and_no_further() {
        let now = 1_700_000_000;
        let code = |s: i64| format!("{:06}", hotp(RFC_SECRET, s as u64));
        let at = step(now);
        assert_eq!(matching_step(RFC_SECRET, &code(at - 1), now), Some(at - 1));
        assert_eq!(matching_step(RFC_SECRET, &code(at + 1), now), Some(at + 1));
        assert_eq!(matching_step(RFC_SECRET, &code(at - 2), now), None);
        assert_eq!(matching_step(RFC_SECRET, &code(at + 2), now), None);
        assert_eq!(matching_step(RFC_SECRET, "12345", now), None);
        assert_eq!(matching_step(RFC_SECRET, "abcdef", now), None);
    }

    #[test]
    fn base32_round_trips() {
        assert_eq!(base32(b"foobar"), "MZXW6YTBOI");
        assert_eq!(from_base32("mzxw 6ytb oi==").unwrap(), b"foobar");
        let secret: Vec<u8> = (0..SECRET_LEN as u8).collect();
        assert_eq!(from_base32(&base32(&secret)).unwrap(), secret);
        assert_eq!(from_base32("not base32!"), None);
    }

    #[test]
    fn the_key_uri_label_holds_one_colon() {
        assert!(
            key_uri("kōan ([::1])", "a:b", b"x")
                .starts_with("otpauth://totp/k%C5%8Dan%20%28%5B1%5D%29:ab?")
        );
    }

    #[test]
    fn the_key_uri_names_the_issuer_and_account() {
        assert_eq!(
            key_uri("kōan", "mate b", b"foobar"),
            "otpauth://totp/k%C5%8Dan:mate%20b?secret=MZXW6YTBOI&issuer=k%C5%8Dan&algorithm=SHA1&digits=6&period=30"
        );
    }
}
