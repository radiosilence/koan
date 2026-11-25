//! WebAuthn (W3C Level 2) for security keys and passkeys: the options a
//! browser is given, and the checks on what it sends back.
//!
//! Only what a relying party that does not require attestation needs: a
//! registration's attestation statement is not verified (any `fmt` is taken,
//! `none` included), since kōan trusts whatever authenticator its account's
//! owner chooses. What is checked is that the response is for this server and
//! this challenge, that a person was present, and on sign-in that the
//! signature is the registered key's. ES256, EdDSA and RS256 keys are
//! accepted, which covers every FIDO2 key and platform passkey in use.
//!
//! As a second factor, user verification is asked for (`preferred`) but not
//! required: a key without a PIN is still a second factor beside the
//! password. A passkey signing in on its own ([`passkey_options`]) must
//! verify the person, by PIN or biometrics, since it then stands for both
//! factors: the device, and the person who unlocked it.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use serde_json::json;

use crate::cbor::{self, Value};

/// COSE algorithm ids, in the order a browser is asked to prefer them.
const ES256: i128 = -7;
const EDDSA: i128 = -8;
const RS256: i128 = -257;

/// How long a browser is given to finish a ceremony, in milliseconds.
const TIMEOUT_MS: u64 = 120_000;

/// Authenticator data flags.
const USER_PRESENT: u8 = 0x01;
const USER_VERIFIED: u8 = 0x04;
const BACKUP_ELIGIBLE: u8 = 0x08;
const BACKED_UP: u8 = 0x10;
const ATTESTED: u8 = 0x40;
const EXTENSIONS: u8 = 0x80;

/// The longest credential ID accepted (WebAuthn Level 3 §7.1).
const MAX_CREDENTIAL_ID: usize = 1023;

/// The smallest RSA modulus a key may have, in bytes: what ring verifies.
const MIN_RSA_MODULUS: usize = 256;

/// The server as WebAuthn knows it: its ID, the host credentials are scoped
/// to, and the origin pages are served from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelyingParty {
    pub id: String,
    pub origin: String,
    pub name: String,
}

impl RelyingParty {
    /// The relying party a server at `url` is: its host for the ID, its
    /// origin. `None` for an address browsers do not allow WebAuthn on, which
    /// is anything but HTTPS and `localhost`, or an IP address, which cannot
    /// be an RP ID.
    pub fn from_url(url: &str, name: &str) -> Option<Self> {
        let parsed = url::Url::parse(url).ok()?;
        let host = parsed.host()?;
        let url::Host::Domain(domain) = host else {
            return None;
        };
        let local = domain == "localhost" || domain.ends_with(".localhost");
        if parsed.scheme() != "https" && !(parsed.scheme() == "http" && local) {
            return None;
        }
        Some(Self {
            id: domain.to_ascii_lowercase(),
            origin: parsed.origin().ascii_serialization(),
            name: name.to_owned(),
        })
    }
}

/// Why a WebAuthn response was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WebAuthnError {
    #[error("the response could not be read")]
    Malformed,
    #[error("the response is for another ceremony, challenge or site")]
    Mismatch,
    #[error("the authenticator did not register a credential")]
    NoCredential,
    #[error("no person was present at the authenticator")]
    NotPresent,
    #[error("the authenticator did not verify the person with a PIN or biometrics")]
    NotVerified,
    #[error("the key uses an algorithm this server does not accept")]
    Unsupported,
    #[error("the signature does not verify")]
    BadSignature,
    #[error("the key's counter went backwards, as a cloned key's would")]
    Cloned,
}

/// What `navigator.credentials.create` is given as `publicKey`, with its
/// binary fields as base64url for the page to decode. `user_handle` is the
/// account's opaque handle, `exclude` the IDs of keys it already has.
pub fn creation_options(
    rp: &RelyingParty,
    user_handle: &[u8],
    username: &str,
    challenge: &[u8],
    exclude: &[Vec<u8>],
) -> serde_json::Value {
    let params =
        [ES256, EDDSA, RS256].map(|alg| json!({ "type": "public-key", "alg": alg as i64 }));
    json!({
        "rp": { "id": rp.id, "name": rp.name },
        "user": {
            "id": URL_SAFE_NO_PAD.encode(user_handle),
            "name": username,
            "displayName": username,
        },
        "challenge": URL_SAFE_NO_PAD.encode(challenge),
        "pubKeyCredParams": params,
        "timeout": TIMEOUT_MS,
        "excludeCredentials": descriptors(exclude),
        "authenticatorSelection": {
            "residentKey": "preferred",
            "userVerification": "preferred",
        },
        "attestation": "none",
    })
}

/// What `navigator.credentials.get` is given as `publicKey`: `allow` is the
/// IDs of the account's keys.
pub fn request_options(
    rp: &RelyingParty,
    challenge: &[u8],
    allow: &[Vec<u8>],
) -> serde_json::Value {
    json!({
        "rpId": rp.id,
        "challenge": URL_SAFE_NO_PAD.encode(challenge),
        "timeout": TIMEOUT_MS,
        "allowCredentials": descriptors(allow),
        "userVerification": "preferred",
    })
}

/// `navigator.credentials.get`'s options for signing in with a passkey alone:
/// no credentials listed, since the account is not known until the passkey
/// names it, and user verification required.
pub fn passkey_options(rp: &RelyingParty, challenge: &[u8]) -> serde_json::Value {
    json!({
        "rpId": rp.id,
        "challenge": URL_SAFE_NO_PAD.encode(challenge),
        "timeout": TIMEOUT_MS,
        "allowCredentials": [],
        "userVerification": "required",
    })
}

fn descriptors(ids: &[Vec<u8>]) -> Vec<serde_json::Value> {
    ids.iter()
        .map(|id| json!({ "type": "public-key", "id": URL_SAFE_NO_PAD.encode(id) }))
        .collect()
}

/// A browser's answer to `create`, its binary fields as base64url.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Registration {
    pub client_data_json: String,
    pub attestation_object: String,
}

/// A browser's answer to `get`, its binary fields as base64url.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Assertion {
    /// The credential's ID.
    pub id: String,
    pub client_data_json: String,
    pub authenticator_data: String,
    pub signature: String,
    /// The account's handle, which a discoverable credential returns: how a
    /// passkey names its account when none was given.
    #[serde(default)]
    pub user_handle: Option<String>,
}

/// A credential registered: what is kept to check its signatures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewCredential {
    pub id: Vec<u8>,
    /// The COSE key, as the authenticator encoded it.
    pub public_key: Vec<u8>,
    pub sign_count: u32,
}

fn b64(text: &str) -> Result<Vec<u8>, WebAuthnError> {
    URL_SAFE_NO_PAD
        .decode(text.trim_end_matches('='))
        .map_err(|_| WebAuthnError::Malformed)
}

/// The credential ID an assertion names.
pub fn assertion_id(assertion: &Assertion) -> Result<Vec<u8>, WebAuthnError> {
    b64(&assertion.id)
}

/// The user handle an assertion carries, if any.
pub fn assertion_user_handle(assertion: &Assertion) -> Result<Option<Vec<u8>>, WebAuthnError> {
    assertion
        .user_handle
        .as_deref()
        .filter(|h| !h.is_empty())
        .map(b64)
        .transpose()
}

#[derive(Deserialize)]
struct ClientData {
    #[serde(rename = "type")]
    kind: String,
    challenge: String,
    origin: String,
    #[serde(default, rename = "crossOrigin")]
    cross_origin: bool,
}

/// Check the client data is this ceremony's, for this challenge, from this
/// server's own pages.
fn check_client_data(
    rp: &RelyingParty,
    kind: &str,
    challenge: &[u8],
    json: &[u8],
) -> Result<(), WebAuthnError> {
    let data: ClientData = serde_json::from_slice(json).map_err(|_| WebAuthnError::Malformed)?;
    let given = b64(&data.challenge)?;
    let same_challenge = bool::from(subtle::ConstantTimeEq::ct_eq(&given[..], challenge));
    if data.kind != kind || !same_challenge || data.origin != rp.origin || data.cross_origin {
        return Err(WebAuthnError::Mismatch);
    }
    Ok(())
}

struct AuthData<'a> {
    verified: bool,
    sign_count: u32,
    /// The credential ID and COSE key, when the data carries one.
    attested: Option<(&'a [u8], &'a [u8])>,
}

fn auth_data<'a>(rp: &RelyingParty, data: &'a [u8]) -> Result<AuthData<'a>, WebAuthnError> {
    if data.len() < 37 {
        return Err(WebAuthnError::Malformed);
    }
    let rp_hash = ring::digest::digest(&ring::digest::SHA256, rp.id.as_bytes());
    if data[..32] != *rp_hash.as_ref() {
        return Err(WebAuthnError::Mismatch);
    }
    let flags = data[32];
    if flags & USER_PRESENT == 0 {
        return Err(WebAuthnError::NotPresent);
    }
    // A credential cannot be backed up without being eligible for it.
    if flags & BACKED_UP != 0 && flags & BACKUP_ELIGIBLE == 0 {
        return Err(WebAuthnError::Malformed);
    }
    let sign_count = u32::from_be_bytes(data[33..37].try_into().expect("four bytes"));
    let attested = if flags & ATTESTED != 0 {
        // AAGUID (16), then the ID's length (2) and the ID, then the key.
        let rest = data.get(37 + 16..).ok_or(WebAuthnError::Malformed)?;
        let len = u16::from_be_bytes(
            rest.get(..2)
                .ok_or(WebAuthnError::Malformed)?
                .try_into()
                .expect("two bytes"),
        ) as usize;
        if len == 0 || len > MAX_CREDENTIAL_ID {
            return Err(WebAuthnError::Malformed);
        }
        let id = rest.get(2..2 + len).ok_or(WebAuthnError::Malformed)?;
        let key_bytes = &rest[2 + len..];
        let (_, key_len) = cbor::decode(key_bytes).ok_or(WebAuthnError::Malformed)?;
        // Extensions may follow the key; nothing else may.
        if flags & EXTENSIONS == 0 && key_len != key_bytes.len() {
            return Err(WebAuthnError::Malformed);
        }
        Some((id, &key_bytes[..key_len]))
    } else {
        if flags & EXTENSIONS == 0 && data.len() != 37 {
            return Err(WebAuthnError::Malformed);
        }
        None
    };
    Ok(AuthData {
        verified: flags & USER_VERIFIED != 0,
        sign_count,
        attested,
    })
}

/// Check a registration made for `challenge`, and the credential it made.
pub fn register(
    rp: &RelyingParty,
    challenge: &[u8],
    response: &Registration,
) -> Result<NewCredential, WebAuthnError> {
    let client_data = b64(&response.client_data_json)?;
    check_client_data(rp, "webauthn.create", challenge, &client_data)?;
    let object_bytes = b64(&response.attestation_object)?;
    let (object, used) = cbor::decode(&object_bytes).ok_or(WebAuthnError::Malformed)?;
    // The statement's format and statement are read but not verified: this
    // relying party asks for no attestation (`none`), and takes any.
    let well_formed = used == object_bytes.len()
        && matches!(object.text("fmt"), Some(Value::Text(_)))
        && matches!(object.text("attStmt"), Some(Value::Map(_)));
    if !well_formed {
        return Err(WebAuthnError::Malformed);
    }
    let data = object
        .text("authData")
        .and_then(Value::as_bytes)
        .ok_or(WebAuthnError::Malformed)?;
    let parsed = auth_data(rp, data)?;
    let (id, key) = parsed.attested.ok_or(WebAuthnError::NoCredential)?;
    // Read now, so that a key of a kind or size this server cannot check is
    // refused at registration rather than at every sign-in after it.
    public_key(key)?;
    Ok(NewCredential {
        id: id.to_vec(),
        public_key: key.to_vec(),
        sign_count: parsed.sign_count,
    })
}

/// Check an assertion made for `challenge` by the credential whose COSE key
/// is `key` and whose counter last stood at `count`. The counter it stands at
/// now, to keep.
///
/// A counter that does not move past the last one marks a cloned key, except
/// when both are zero, which is how a passkey synced between devices reports.
/// `verified` requires the authenticator to have verified the person.
pub fn verify(
    rp: &RelyingParty,
    challenge: &[u8],
    assertion: &Assertion,
    key: &[u8],
    count: u32,
    verified: bool,
) -> Result<u32, WebAuthnError> {
    let client_data = b64(&assertion.client_data_json)?;
    check_client_data(rp, "webauthn.get", challenge, &client_data)?;
    let data = b64(&assertion.authenticator_data)?;
    let parsed = auth_data(rp, &data)?;
    if verified && !parsed.verified {
        return Err(WebAuthnError::NotVerified);
    }
    let mut signed = data.clone();
    signed.extend_from_slice(ring::digest::digest(&ring::digest::SHA256, &client_data).as_ref());
    public_key(key)?.verify(&signed, &b64(&assertion.signature)?)?;
    if (parsed.sign_count != 0 || count != 0) && parsed.sign_count <= count {
        return Err(WebAuthnError::Cloned);
    }
    Ok(parsed.sign_count)
}

enum PublicKey {
    Ecdsa(Vec<u8>),
    Ed25519(Vec<u8>),
    Rsa { n: Vec<u8>, e: Vec<u8> },
}

impl PublicKey {
    fn verify(&self, message: &[u8], signature: &[u8]) -> Result<(), WebAuthnError> {
        use ring::signature::{
            ECDSA_P256_SHA256_ASN1, ED25519, RSA_PKCS1_2048_8192_SHA256, RsaPublicKeyComponents,
            UnparsedPublicKey,
        };
        let checked = match self {
            PublicKey::Ecdsa(point) => {
                UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, point).verify(message, signature)
            }
            PublicKey::Ed25519(key) => {
                UnparsedPublicKey::new(&ED25519, key).verify(message, signature)
            }
            PublicKey::Rsa { n, e } => RsaPublicKeyComponents { n, e }.verify(
                &RSA_PKCS1_2048_8192_SHA256,
                message,
                signature,
            ),
        };
        checked.map_err(|_| WebAuthnError::BadSignature)
    }
}

/// A COSE key (RFC 9053) this server can check signatures with.
fn public_key(cose: &[u8]) -> Result<PublicKey, WebAuthnError> {
    let (key, used) = cbor::decode(cose).ok_or(WebAuthnError::Malformed)?;
    if used != cose.len() {
        return Err(WebAuthnError::Malformed);
    }
    let field = |k: i128| key.int(k).ok_or(WebAuthnError::Malformed);
    let bytes = |k: i128| field(k)?.as_bytes().ok_or(WebAuthnError::Malformed);
    let kty = field(1)?.as_int();
    let alg = field(3)?.as_int();
    match (kty, alg) {
        (Some(2), Some(ES256)) if field(-1)?.as_int() == Some(1) => {
            let (x, y) = (bytes(-2)?, bytes(-3)?);
            if x.len() != 32 || y.len() != 32 {
                return Err(WebAuthnError::Malformed);
            }
            Ok(PublicKey::Ecdsa([&[4u8][..], x, y].concat()))
        }
        (Some(1), Some(EDDSA)) if field(-1)?.as_int() == Some(6) => {
            let x = bytes(-2)?;
            if x.len() != 32 {
                return Err(WebAuthnError::Malformed);
            }
            Ok(PublicKey::Ed25519(x.to_vec()))
        }
        (Some(3), Some(RS256)) => {
            let n = bytes(-1)?;
            let n = &n[n.iter().take_while(|b| **b == 0).count()..];
            if n.len() < MIN_RSA_MODULUS {
                return Err(WebAuthnError::Unsupported);
            }
            Ok(PublicKey::Rsa {
                n: n.to_vec(),
                e: bytes(-2)?.to_vec(),
            })
        }
        _ => Err(WebAuthnError::Unsupported),
    }
}

/// An authenticator played in software, for tests here and in the services
/// built on this crate.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use ring::rand::SystemRandom;
    use ring::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair};

    pub(crate) struct Authenticator {
        pair: EcdsaKeyPair,
        pub(crate) id: Vec<u8>,
        pub(crate) count: u32,
        /// Whether it verifies the person, and the account handle it keeps.
        pub(crate) verifies: bool,
        pub(crate) handle: Option<Vec<u8>>,
    }

    impl Authenticator {
        pub(crate) fn new() -> Self {
            let rng = SystemRandom::new();
            let pkcs8 =
                EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).unwrap();
            Self {
                pair: EcdsaKeyPair::from_pkcs8(
                    &ECDSA_P256_SHA256_ASN1_SIGNING,
                    pkcs8.as_ref(),
                    &rng,
                )
                .unwrap(),
                id: vec![7; 16],
                count: 0,
                verifies: false,
                handle: None,
            }
        }

        fn cose(&self) -> Vec<u8> {
            let point = self.pair.public_key().as_ref();
            cbor::encode(&Value::Map(vec![
                (Value::Int(1), Value::Int(2)),
                (Value::Int(3), Value::Int(ES256)),
                (Value::Int(-1), Value::Int(1)),
                (Value::Int(-2), Value::Bytes(point[1..33].to_vec())),
                (Value::Int(-3), Value::Bytes(point[33..].to_vec())),
            ]))
        }

        fn client_data(kind: &str, origin: &str, challenge: &[u8]) -> Vec<u8> {
            json!({
                "type": kind,
                "challenge": URL_SAFE_NO_PAD.encode(challenge),
                "origin": origin,
            })
            .to_string()
            .into_bytes()
        }

        fn data(&self, rp_id: &str, flags: u8, attested: bool) -> Vec<u8> {
            let mut data = ring::digest::digest(&ring::digest::SHA256, rp_id.as_bytes())
                .as_ref()
                .to_vec();
            data.push(flags | if attested { ATTESTED } else { 0 });
            data.extend(self.count.to_be_bytes());
            if attested {
                data.extend([0u8; 16]);
                data.extend((self.id.len() as u16).to_be_bytes());
                data.extend(&self.id);
                data.extend(self.cose());
            }
            data
        }

        pub(crate) fn create(&self, rp: &RelyingParty, challenge: &[u8]) -> Registration {
            let object = cbor::encode(&Value::Map(vec![
                (Value::Text("fmt".into()), Value::Text("none".into())),
                (Value::Text("attStmt".into()), Value::Map(vec![])),
                (
                    Value::Text("authData".into()),
                    Value::Bytes(self.data(&rp.id, USER_PRESENT, true)),
                ),
            ]));
            Registration {
                client_data_json: URL_SAFE_NO_PAD.encode(Self::client_data(
                    "webauthn.create",
                    &rp.origin,
                    challenge,
                )),
                attestation_object: URL_SAFE_NO_PAD.encode(object),
            }
        }

        /// An assertion for `challenge` as `origin` and `rp_id` say, the
        /// counter moved on by one when `count` is set.
        pub(crate) fn get_as(
            &mut self,
            rp_id: &str,
            origin: &str,
            challenge: &[u8],
            count: bool,
        ) -> Assertion {
            if count {
                self.count += 1;
            }
            let client_data = Self::client_data("webauthn.get", origin, challenge);
            let flags = USER_PRESENT | if self.verifies { USER_VERIFIED } else { 0 };
            let data = self.data(rp_id, flags, false);
            let mut signed = data.clone();
            signed.extend(ring::digest::digest(&ring::digest::SHA256, &client_data).as_ref());
            let signature = self.pair.sign(&SystemRandom::new(), &signed).unwrap();
            Assertion {
                id: URL_SAFE_NO_PAD.encode(&self.id),
                client_data_json: URL_SAFE_NO_PAD.encode(client_data),
                authenticator_data: URL_SAFE_NO_PAD.encode(data),
                signature: URL_SAFE_NO_PAD.encode(signature.as_ref()),
                user_handle: self.handle.as_ref().map(|h| URL_SAFE_NO_PAD.encode(h)),
            }
        }

        pub(crate) fn get(&mut self, rp: &RelyingParty, challenge: &[u8]) -> Assertion {
            self.get_as(&rp.id.clone(), &rp.origin.clone(), challenge, true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::Authenticator;
    use super::*;

    fn rp() -> RelyingParty {
        RelyingParty::from_url("https://music.example.org/koan", "kōan").unwrap()
    }

    #[test]
    fn the_relying_party_is_the_servers_host() {
        assert_eq!(
            rp(),
            RelyingParty {
                id: "music.example.org".into(),
                origin: "https://music.example.org".into(),
                name: "kōan".into(),
            }
        );
        assert!(RelyingParty::from_url("http://localhost:4000", "k").is_some());
        assert!(RelyingParty::from_url("http://music.example.org", "k").is_none());
        assert!(RelyingParty::from_url("https://192.168.1.4", "k").is_none());
    }

    #[test]
    fn a_registered_key_signs_in() {
        let rp = rp();
        let mut key = Authenticator::new();
        let made = register(&rp, b"challenge one", &key.create(&rp, b"challenge one")).unwrap();
        assert_eq!(made.id, key.id);
        let assertion = key.get(&rp, b"challenge two");
        assert_eq!(assertion_id(&assertion).unwrap(), key.id);
        assert_eq!(
            verify(
                &rp,
                b"challenge two",
                &assertion,
                &made.public_key,
                0,
                false
            ),
            Ok(1)
        );
    }

    #[test]
    fn a_response_for_another_challenge_or_site_is_refused() {
        let rp = rp();
        let mut key = Authenticator::new();
        assert_eq!(
            register(&rp, b"mine", &key.create(&rp, b"theirs")),
            Err(WebAuthnError::Mismatch)
        );
        let made = register(&rp, b"mine", &key.create(&rp, b"mine")).unwrap();
        let wrong_challenge = key.get(&rp, b"theirs");
        assert_eq!(
            verify(&rp, b"mine", &wrong_challenge, &made.public_key, 0, false),
            Err(WebAuthnError::Mismatch)
        );
        let phished = key.get_as(&rp.id, "https://evil.example", b"mine", true);
        assert_eq!(
            verify(&rp, b"mine", &phished, &made.public_key, 0, false),
            Err(WebAuthnError::Mismatch)
        );
        let other_rp = key.get_as("evil.example", &rp.origin, b"mine", true);
        assert_eq!(
            verify(&rp, b"mine", &other_rp, &made.public_key, 0, false),
            Err(WebAuthnError::Mismatch)
        );
    }

    #[test]
    fn another_keys_signature_or_a_stale_counter_is_refused() {
        let rp = rp();
        let mut key = Authenticator::new();
        let mut other = Authenticator::new();
        let made = register(&rp, b"c", &key.create(&rp, b"c")).unwrap();
        let forged = other.get(&rp, b"c");
        assert_eq!(
            verify(&rp, b"c", &forged, &made.public_key, 0, false),
            Err(WebAuthnError::BadSignature)
        );
        let assertion = key.get(&rp, b"c");
        assert_eq!(
            verify(&rp, b"c", &assertion, &made.public_key, 5, false),
            Err(WebAuthnError::Cloned)
        );
        // A synced passkey's counter stays at zero.
        key.count = 0;
        let zero = key.get_as(&rp.id, &rp.origin, b"c", false);
        assert_eq!(verify(&rp, b"c", &zero, &made.public_key, 0, false), Ok(0));
    }

    #[test]
    fn a_passkey_alone_must_verify_the_person() {
        let rp = rp();
        let mut key = Authenticator::new();
        let made = register(&rp, b"c", &key.create(&rp, b"c")).unwrap();
        let unverified = key.get(&rp, b"c");
        assert_eq!(
            verify(&rp, b"c", &unverified, &made.public_key, 1, true),
            Err(WebAuthnError::NotVerified)
        );
        key.verifies = true;
        let verified = key.get(&rp, b"c");
        assert_eq!(
            verify(&rp, b"c", &verified, &made.public_key, 1, true),
            Ok(2)
        );
        let options = passkey_options(&rp, b"c");
        assert_eq!(options["userVerification"], "required");
        assert_eq!(options["allowCredentials"], json!([]));
    }

    #[test]
    fn options_carry_the_challenge_and_keys() {
        let rp = rp();
        let create = creation_options(&rp, &[1, 2], "mate", b"chal", &[vec![9]]);
        assert_eq!(create["rp"]["id"], "music.example.org");
        assert_eq!(create["challenge"], URL_SAFE_NO_PAD.encode(b"chal"));
        assert_eq!(create["excludeCredentials"][0]["id"], "CQ");
        let get = request_options(&rp, b"chal", &[vec![9]]);
        assert_eq!(get["allowCredentials"][0]["id"], "CQ");
        assert_eq!(get["rpId"], "music.example.org");
    }
}
