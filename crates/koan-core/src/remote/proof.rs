//! Devices proving on the local network that they are the account's own,
//! without the server in the room.
//!
//! Each device signed in with an API key holds an Ed25519 keypair, kept with
//! its credentials in `config.local.toml` and made afresh at each sign-in. Its
//! public key goes up the link (`deviceKey`), and the server sends every
//! device the account's keys, and those of devices shared with it
//! (`LinkCommand::DeviceKeys`), which are kept here so that the check works
//! with the server out of reach.
//!
//! A nearby connection proves both ends: the listening end's `Hello` carries
//! a nonce, the dialling end answers with its own and a signature over both,
//! and the listening end signs back. Both signatures name both devices, both
//! nonces and which end made them, so a recorded handshake fails against a
//! fresh nonce and one end's signature cannot be passed off as the other's.
//! Nearby connections are not encrypted, so every command after a proven
//! handshake is signed too, over the handshake and a sequence number: one
//! slipped into the stream, replayed, or taken from another session is
//! refused.
//!
//! A peer that proves nothing, because it is older, signed out, or on another
//! server, is classed as it always was.

use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use parking_lot::Mutex;
use ring::rand::SecureRandom as _;
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair as _, UnparsedPublicKey};
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::remote::link::LinkDeviceKey;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// A kept key list older than this is not trusted: it may hold a device
/// revoked while this one was away from the server.
const KEYS_TRUSTED_FOR: i64 = 30 * 24 * 60 * 60;

const DIAL: &str = "koan-nearby-v1 dial";
const LISTEN: &str = "koan-nearby-v1 listen";
const SESSION: &str = "koan-nearby-v1 session";
const COMMAND: &str = "koan-nearby-v1 command";

/// A fresh keypair, as kept in `remote.device_key`: base64 of its PKCS#8.
pub fn new_device_key() -> Option<String> {
    let rng = ring::rand::SystemRandom::new();
    let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).ok()?;
    Some(B64.encode(pkcs8.as_ref()))
}

fn keypair_from(kept: &str) -> Option<Ed25519KeyPair> {
    let pkcs8 = B64.decode(kept).ok()?;
    Ed25519KeyPair::from_pkcs8(&pkcs8).ok()
}

/// This device's keypair, while it is signed in with an API key. One signed
/// in before keys existed is given one here, kept as a sign-in would.
fn keypair() -> Option<Ed25519KeyPair> {
    let cfg = Config::cached();
    if !cfg.remote.enabled || cfg.remote.api_key.is_empty() {
        return None;
    }
    if let Some(pair) = keypair_from(&cfg.remote.device_key) {
        return Some(pair);
    }
    let made = new_device_key()?;
    if let Err(e) = Config::persist(|c| c.remote.device_key = made.clone()) {
        log::warn!("proof: could not keep a device key: {e}");
        return None;
    }
    keypair_from(&made)
}

/// The public key the link registers, base64.
pub fn public_key() -> Option<String> {
    keypair().map(|pair| B64.encode(pair.public_key().as_ref()))
}

/// A fresh nonce, base64 of 32 random bytes. `None` if the system cannot
/// give random bytes: a nonce that is not fresh would let a recorded
/// handshake, and the commands signed after it, be replayed, so without one
/// nothing is proven.
pub fn nonce() -> Option<String> {
    let mut bytes = [0u8; 32];
    ring::rand::SystemRandom::new().fill(&mut bytes).ok()?;
    Some(B64.encode(bytes))
}

/// `label` and each field, each preceded by its length, so no two lists of
/// fields read the same.
fn message(label: &str, fields: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for field in std::iter::once(label.as_bytes()).chain(fields.iter().copied()) {
        out.extend_from_slice(&(field.len() as u32).to_be_bytes());
        out.extend_from_slice(field);
    }
    out
}

/// What the dialling end signs: it, the device it dialled, and both nonces.
fn dial_message(listener: &str, dialer: &str, listen_nonce: &str, dial_nonce: &str) -> Vec<u8> {
    message(
        DIAL,
        &[
            listener.as_bytes(),
            dialer.as_bytes(),
            listen_nonce.as_bytes(),
            dial_nonce.as_bytes(),
        ],
    )
}

/// What the listening end signs: the same, from its side, and whether it
/// took the dialler for the account's or a shared device.
fn listen_message(
    dialer: &str,
    listener: &str,
    dial_nonce: &str,
    listen_nonce: &str,
    verified: bool,
) -> Vec<u8> {
    message(
        LISTEN,
        &[
            dialer.as_bytes(),
            listener.as_bytes(),
            dial_nonce.as_bytes(),
            listen_nonce.as_bytes(),
            &[verified as u8],
        ],
    )
}

fn sign(pair: &Ed25519KeyPair, msg: &[u8]) -> String {
    B64.encode(pair.sign(msg).as_ref())
}

/// The dialling end's signature, or `None` with no keypair.
pub fn sign_dial(
    listener: &str,
    dialer: &str,
    listen_nonce: &str,
    dial_nonce: &str,
) -> Option<String> {
    let pair = keypair()?;
    Some(sign(
        &pair,
        &dial_message(listener, dialer, listen_nonce, dial_nonce),
    ))
}

/// The listening end's signature, or `None` with no keypair.
pub fn sign_listen(
    dialer: &str,
    listener: &str,
    dial_nonce: &str,
    listen_nonce: &str,
    verified: bool,
) -> Option<String> {
    let pair = keypair()?;
    Some(sign(
        &pair,
        &listen_message(dialer, listener, dial_nonce, listen_nonce, verified),
    ))
}

/// Who a peer proved to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Peer {
    /// One of this account's devices.
    Own,
    /// A device another account shares with this one.
    Shared(String),
}

/// A peer that proved itself: who, and the key it proved with, which its
/// signed commands are checked against.
#[derive(Debug, Clone)]
pub struct Proven {
    pub peer: Peer,
    key: Vec<u8>,
}

/// The peer claiming `id` that signed `msg` with `sig`, by the keys `keys`
/// lists for it. A device id is the client's own choice, so another account
/// can list one of this account's ids as its own: every key for the id is
/// tried, and one with an owner proves a shared device, never this account's.
fn verify_with(keys: &[LinkDeviceKey], id: &str, msg: &[u8], sig: &str) -> Option<Proven> {
    let sig = B64.decode(sig).ok()?;
    keys.iter().filter(|k| k.id == id).find_map(|k| {
        let key = B64.decode(&k.key).ok()?;
        UnparsedPublicKey::new(&ED25519, &key)
            .verify(msg, &sig)
            .ok()?;
        Some(Proven {
            peer: match &k.owner {
                None => Peer::Own,
                Some(owner) => Peer::Shared(owner.clone()),
            },
            key,
        })
    })
}

/// Whether the dialler `dialer` proved itself to this listener.
pub fn verify_dial(
    listener: &str,
    dialer: &str,
    listen_nonce: &str,
    dial_nonce: &str,
    sig: &str,
) -> Option<Proven> {
    verify_with(
        &kept(),
        dialer,
        &dial_message(listener, dialer, listen_nonce, dial_nonce),
        sig,
    )
}

/// Whether the listener `listener` proved itself to this dialler.
pub fn verify_listen(
    dialer: &str,
    listener: &str,
    dial_nonce: &str,
    listen_nonce: &str,
    verified: bool,
    sig: &str,
) -> Option<Proven> {
    verify_with(
        &kept(),
        listener,
        &listen_message(dialer, listener, dial_nonce, listen_nonce, verified),
        sig,
    )
}

/// The signed half of a proven connection: what binds each command to this
/// connection and puts it in order.
pub struct Session {
    transcript: [u8; 32],
    /// The dialler's: the last sequence number signed. The listener's: the
    /// last accepted.
    seq: u64,
}

impl Session {
    /// The session both ends compute from the handshake.
    pub fn new(listener: &str, dialer: &str, listen_nonce: &str, dial_nonce: &str) -> Self {
        let digest = ring::digest::digest(
            &ring::digest::SHA256,
            &message(
                SESSION,
                &[
                    listener.as_bytes(),
                    dialer.as_bytes(),
                    listen_nonce.as_bytes(),
                    dial_nonce.as_bytes(),
                ],
            ),
        );
        let mut transcript = [0u8; 32];
        transcript.copy_from_slice(digest.as_ref());
        Self { transcript, seq: 0 }
    }

    fn command_message(&self, seq: u64, command: &str) -> Vec<u8> {
        message(
            COMMAND,
            &[&self.transcript, &seq.to_be_bytes(), command.as_bytes()],
        )
    }

    /// Sign `command` (its JSON) as the next in this session. `None` with no
    /// keypair.
    pub fn sign(&mut self, command: &str) -> Option<(u64, String)> {
        let pair = keypair()?;
        self.sign_with(&pair, command)
    }

    fn sign_with(&mut self, pair: &Ed25519KeyPair, command: &str) -> Option<(u64, String)> {
        self.seq += 1;
        Some((
            self.seq,
            sign(pair, &self.command_message(self.seq, command)),
        ))
    }

    /// Whether `command` was signed by `by` for this session, later in it
    /// than anything accepted so far. Accepting it moves the session on.
    pub fn accept(&mut self, by: &Proven, seq: u64, sig: &str, command: &str) -> bool {
        if seq <= self.seq {
            return false;
        }
        let Ok(sig) = B64.decode(sig) else {
            return false;
        };
        let ok = UnparsedPublicKey::new(&ED25519, &by.key)
            .verify(&self.command_message(seq, command), &sig)
            .is_ok();
        if ok {
            self.seq = seq;
        }
        ok
    }
}

// --- The kept key list ------------------------------------------------------

/// The account's device keys as the server last sent them, for the account
/// they were sent to, and when.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Kept {
    /// The server and account they are for (`account_of`): keys from one
    /// prove nothing for another, a second account signed in to the same
    /// server on this device included.
    account: Option<String>,
    at: i64,
    keys: Vec<LinkDeviceKey>,
}

/// Which account on which server `cfg` signs in to, as kept keys are tagged
/// with: the server's `library_fingerprint` and the username, compared as the
/// server compares it, exactly.
pub fn account_of(cfg: &Config) -> Option<String> {
    let server = crate::remote::link::library_fingerprint(cfg)?;
    Some(format!("{server}/{}", cfg.remote.username))
}

static KEPT: Mutex<Option<Kept>> = Mutex::new(None);

fn kept_path() -> std::path::PathBuf {
    crate::config::config_dir().join("device-keys.json")
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

/// Keep the key list the server just sent to the link signed in as
/// `account`, replacing the last whole: a key revoked is a key no longer
/// listed. Tagged with the link's account, not whoever is signed in now, so a
/// list that reaches a link still open for the last account is never taken
/// for the next one's.
pub fn keep(keys: Vec<LinkDeviceKey>, account: Option<String>) {
    let kept = Kept {
        account,
        at: now(),
        keys,
    };
    if let Ok(json) = serde_json::to_string(&kept) {
        let _ = std::fs::write(kept_path(), json);
    }
    *KEPT.lock() = Some(kept);
}

/// The keys to check a peer against: the last list kept for the server this
/// device is signed in to, while it is recent enough to trust.
fn kept() -> Vec<LinkDeviceKey> {
    let mut held = KEPT.lock();
    if held.is_none() {
        *held = std::fs::read_to_string(kept_path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok());
    }
    let Some(kept) = held.as_ref() else {
        return Vec::new();
    };
    let account = account_of(&Config::cached());
    trusted(kept, account.as_deref(), now())
}

fn trusted(kept: &Kept, account: Option<&str>, now: i64) -> Vec<LinkDeviceKey> {
    if account.is_none() || kept.account.as_deref() != account || now - kept.at > KEYS_TRUSTED_FOR {
        return Vec::new();
    }
    kept.keys.clone()
}

/// Forget the kept keys: at sign-in and sign-out, so another account signed
/// in here never checks peers against the last one's devices.
pub fn forget() {
    *KEPT.lock() = Some(Kept::default());
    let _ = std::fs::remove_file(kept_path());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair() -> (Ed25519KeyPair, String) {
        let made = new_device_key().unwrap();
        let pair = keypair_from(&made).unwrap();
        let public = B64.encode(pair.public_key().as_ref());
        (pair, public)
    }

    fn key(id: &str, public: &str, owner: Option<&str>) -> LinkDeviceKey {
        LinkDeviceKey {
            id: id.into(),
            key: public.into(),
            owner: owner.map(Into::into),
        }
    }

    #[test]
    fn a_dialler_proves_itself_with_its_own_key_only() {
        let (phone, phone_pub) = pair();
        let (thief, thief_pub) = pair();
        let keys = vec![key("phone", &phone_pub, None)];
        let msg = dial_message("mac", "phone", "nl", "nd");
        let proven = verify_with(&keys, "phone", &msg, &sign(&phone, &msg)).unwrap();
        assert_eq!(proven.peer, Peer::Own);
        // Another key, an id not listed, a damaged signature.
        assert!(verify_with(&keys, "phone", &msg, &sign(&thief, &msg)).is_none());
        assert!(
            verify_with(
                &[key("x", &thief_pub, None)],
                "phone",
                &msg,
                &sign(&thief, &msg)
            )
            .is_none()
        );
        assert!(verify_with(&keys, "phone", &msg, "AAAA").is_none());
        assert!(verify_with(&keys, "phone", &msg, "not base64!").is_none());
    }

    #[test]
    fn a_recorded_handshake_fails_against_a_fresh_nonce() {
        let (phone, phone_pub) = pair();
        let keys = vec![key("phone", &phone_pub, None)];
        let recorded = sign(&phone, &dial_message("mac", "phone", "old", "nd"));
        let fresh = dial_message("mac", "phone", "new", "nd");
        assert!(verify_with(&keys, "phone", &fresh, &recorded).is_none());
        // Nor is it good for another listener.
        let elsewhere = dial_message("tv", "phone", "old", "nd");
        assert!(verify_with(&keys, "phone", &elsewhere, &recorded).is_none());
    }

    #[test]
    fn one_ends_signature_cannot_pass_as_the_others() {
        let (mac, mac_pub) = pair();
        let keys = vec![key("mac", &mac_pub, None)];
        // The Mac, listening, signed this for a phone dialling it; replayed
        // to the phone listening, with the Mac's id as the dialler, it fails.
        let listened = sign(&mac, &listen_message("phone", "mac", "nd", "nl", true));
        let as_dial = dial_message("phone", "mac", "nd", "nl");
        assert!(verify_with(&keys, "mac", &as_dial, &listened).is_none());
    }

    /// Another account can list one of this account's device ids as its own
    /// and share it. Its key proves a shared device, never this account's.
    #[test]
    fn a_key_with_an_owner_never_proves_an_own_device() {
        let (own, own_pub) = pair();
        let (theirs, theirs_pub) = pair();
        let keys = vec![
            key("phone", &own_pub, None),
            key("phone", &theirs_pub, Some("kim")),
        ];
        let msg = dial_message("mac", "phone", "nl", "nd");
        assert_eq!(
            verify_with(&keys, "phone", &msg, &sign(&theirs, &msg))
                .unwrap()
                .peer,
            Peer::Shared("kim".into())
        );
        assert_eq!(
            verify_with(&keys, "phone", &msg, &sign(&own, &msg))
                .unwrap()
                .peer,
            Peer::Own
        );
    }

    /// jo signs out and kim signs in, and a key list reaches the link still
    /// open as jo: kept as jo's, it proves nothing for kim.
    #[test]
    fn keys_from_the_last_accounts_link_prove_nothing_for_the_next() {
        let _guard = crate::config::tests::PERSIST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        crate::config::set_config_dir(dir.path());
        let sign_in = |user: &str| {
            Config::persist(|c| {
                c.remote.enabled = true;
                c.remote.url = "http://koan.test".into();
                c.remote.username = user.into();
                c.remote.api_key = "k".into();
            })
            .unwrap();
        };
        sign_in("jo");
        let jo = account_of(&Config::cached());
        sign_in("kim");
        keep(vec![key("phone", "K", None)], jo);
        assert!(kept().is_empty(), "jo's link, kim signed in");
        keep(vec![key("phone", "K", None)], account_of(&Config::cached()));
        assert_eq!(kept().len(), 1, "kim's own link");
        forget();
        assert!(kept().is_empty());
    }

    #[test]
    fn signed_commands_are_bound_to_their_session_and_order() {
        let (phone, phone_pub) = pair();
        let keys = vec![key("phone", &phone_pub, None)];
        let msg = dial_message("mac", "phone", "nl", "nd");
        let proven = verify_with(&keys, "phone", &msg, &sign(&phone, &msg)).unwrap();

        let mut sending = Session::new("mac", "phone", "nl", "nd");
        let mut receiving = Session::new("mac", "phone", "nl", "nd");
        let (s1, sig1) = sending.sign_with(&phone, r#"{"type":"pause"}"#).unwrap();
        let (s2, sig2) = sending.sign_with(&phone, r#"{"type":"next"}"#).unwrap();

        assert!(
            !receiving.accept(&proven, s1, &sig1, r#"{"type":"sync"}"#),
            "altered"
        );
        assert!(receiving.accept(&proven, s1, &sig1, r#"{"type":"pause"}"#));
        assert!(
            !receiving.accept(&proven, s1, &sig1, r#"{"type":"pause"}"#),
            "replayed"
        );
        assert!(receiving.accept(&proven, s2, &sig2, r#"{"type":"next"}"#));
        assert!(
            !receiving.accept(&proven, s1, &sig1, r#"{"type":"pause"}"#),
            "out of order"
        );

        // Signed for another session: refused.
        let mut other = Session::new("mac", "phone", "nl2", "nd");
        let (s, sig) = other.sign_with(&phone, r#"{"type":"pause"}"#).unwrap();
        let mut fresh = Session::new("mac", "phone", "nl", "nd");
        assert!(!fresh.accept(&proven, s, &sig, r#"{"type":"pause"}"#));
    }

    #[test]
    fn kept_keys_hold_for_their_account_and_a_month() {
        let cfg = |url: &str, user: &str| {
            let mut c = Config::default();
            c.remote.enabled = true;
            c.remote.url = url.into();
            c.remote.username = user.into();
            c.remote.api_key = "k".into();
            account_of(&c)
        };
        let jo = cfg("http://koan.test", "jo");
        let kept = Kept {
            account: jo.clone(),
            at: 1_000_000,
            keys: vec![key("phone", "K", None)],
        };
        assert_eq!(trusted(&kept, jo.as_deref(), 1_000_000 + 60).len(), 1);
        assert_eq!(
            trusted(&kept, cfg("http://KOAN.test/", "jo").as_deref(), 1_000_000).len(),
            1,
            "the same server, spelt differently"
        );
        assert!(
            trusted(&kept, cfg("http://koan.test", "Jo").as_deref(), 1_000_000).is_empty(),
            "usernames are matched exactly, as the server matches them"
        );
        let kim = cfg("http://koan.test", "kim");
        assert!(
            trusted(&kept, kim.as_deref(), 1_000_000).is_empty(),
            "another account"
        );
        let elsewhere = cfg("http://other.test", "jo");
        assert!(
            trusted(&kept, elsewhere.as_deref(), 1_000_000).is_empty(),
            "another server"
        );
        assert!(trusted(&kept, None, 1_000_000).is_empty(), "signed out");
        assert!(
            trusted(&kept, jo.as_deref(), 1_000_000 + KEYS_TRUSTED_FOR + 1).is_empty(),
            "too old"
        );
    }
}
