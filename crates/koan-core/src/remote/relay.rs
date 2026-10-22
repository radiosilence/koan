//! kōan's push relay, from the app's side.
//!
//! A server pushes to this device through the relay, by a handle the app
//! gives it; the device token itself goes to the relay alone
//! (`docs/reference/protocol/push-relay.md`). For each server, the app keeps
//! a slot (sixteen random bytes it chose for that server and keeps across
//! renewals, which the relay counts the server's allowance by and stamps on
//! what it sends), the handle it last registered, and the revoker that
//! retires them. A Live Activity's token is registered the same way, with
//! the revoker.
//!
//! On the production gateway a registration needs an App Attest assertion.
//! The app does the attesting (`Attester`): only it can reach the Secure
//! Enclave. A development build registers on the sandbox gateway, with an
//! assertion when the app can make one and without when it cannot (the
//! simulator).
//!
//! Registering is a round trip to the relay, and to Apple through it, so it
//! runs off the link's thread; the link sends the handle once there is one.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use koan_relay_client::{Assertion, Client, Slot, client_data};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The relay every kōan app registers with.
pub const RELAY: &str = koan_relay_client::DEFAULT_URL;

/// What the app does for App Attest: `DCAppAttestService`, which only it can
/// reach. Each call blocks until the system answers; neither runs on the main
/// thread.
pub trait Attester: Send + Sync {
    /// Make a new key and have Apple attest it over `client_data_hash`:
    /// the key's id and the attestation, or `None` where App Attest is not
    /// available.
    fn attest(&self, client_data_hash: Vec<u8>) -> Option<(Vec<u8>, Vec<u8>)>;
    /// An assertion by the key `key_id` over `client_data_hash`.
    fn assert(&self, key_id: Vec<u8>, client_data_hash: Vec<u8>) -> Option<Vec<u8>>;
}

static ATTESTER: Mutex<Option<Arc<dyn Attester>>> = Mutex::new(None);

/// Let the app attest for registrations.
pub fn set_attester(attester: Arc<dyn Attester>) {
    *ATTESTER.lock() = Some(attester);
}

/// One server's registration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Registration {
    /// Hex.
    slot: String,
    /// The device token and gateway the handle is for: a new token, or a
    /// build on the other gateway, registers again.
    token: String,
    sandbox: bool,
    handle: String,
    revoker: String,
    /// Unix seconds.
    expires: u64,
}

/// The App Attest key the relay verified for this install.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Key {
    /// Hex.
    key_id: String,
    sealed: String,
    production: bool,
    expires: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Store {
    /// By server fingerprint (`link::library_fingerprint`).
    servers: HashMap<String, Registration>,
    key: Option<Key>,
}

fn path() -> PathBuf {
    crate::config::config_dir().join("push-relay.json")
}

fn load() -> Store {
    std::fs::read(path())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save(store: &Store) {
    let Ok(json) = serde_json::to_vec_pretty(store) else {
        return;
    };
    let tmp = path().with_extension("json.tmp");
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(&tmp, path());
    }
}

/// One registration at a time: the store is read, changed and written whole.
static WORKING: Mutex<()> = Mutex::new(());

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn sha256(data: &[u8]) -> Vec<u8> {
    Sha256::digest(data).to_vec()
}

fn slot_of(hex_slot: &str) -> Option<Slot> {
    let bytes = hex::decode(hex_slot).ok()?;
    bytes.try_into().ok()
}

/// The handle for `server` to push to this device by, registering `token`
/// (hex) with the relay if there is none, it is for another token or
/// gateway, or it is past half its life. `None` when the relay or App
/// Attest refuse, or cannot be reached; the link asks again when it next
/// opens.
pub fn handle_for(server: &str, token: &str, sandbox: bool) -> Option<String> {
    let _working = WORKING.lock();
    let mut store = load();
    let entry = store.servers.entry(server.to_string()).or_default();
    if entry.slot.is_empty() {
        let mut slot = [0u8; 16];
        getrandom::fill(&mut slot).ok()?;
        entry.slot = hex::encode(slot);
    }
    let fresh = entry.token == token
        && entry.sandbox == sandbox
        && !entry.handle.is_empty()
        && entry.expires > now() + 30 * 24 * 60 * 60;
    if fresh {
        return Some(entry.handle.clone());
    }
    let slot = slot_of(&entry.slot)?;
    let token_bytes = hex::decode(token).ok()?;
    let client = Client::new(RELAY).ok()?;
    let attester = ATTESTER.lock().clone();
    let registered = match attester {
        Some(attester) => {
            let key = attested_key(&client, &attester, &mut store.key, sandbox);
            match key {
                Some(key) => {
                    let challenge = client.challenge().ok()?;
                    let data = client_data(&challenge, &token_bytes, &slot, sandbox);
                    let assertion = attester.assert(hex::decode(&key.key_id).ok()?, sha256(&data));
                    match assertion {
                        Some(assertion) => client.register_attested(
                            &token_bytes,
                            &slot,
                            sandbox,
                            &Assertion {
                                key: &key.sealed,
                                challenge: &challenge,
                                assertion: &assertion,
                            },
                        ),
                        None if sandbox => client.register(&token_bytes, &slot, true),
                        None => return None,
                    }
                }
                None if sandbox => client.register(&token_bytes, &slot, true),
                None => {
                    log::warn!(
                        "push relay: App Attest is unavailable, so this device cannot register"
                    );
                    return None;
                }
            }
        }
        None if sandbox => client.register(&token_bytes, &slot, true),
        None => return None,
    };
    // A key attested on the way is kept whatever the registration's fate.
    save(&store);
    let registered = match registered {
        Ok(r) => r,
        Err(e) => {
            log::info!("push relay: registering: {e}");
            return None;
        }
    };
    let entry = store.servers.entry(server.to_string()).or_default();
    entry.token = token.to_string();
    entry.sandbox = sandbox;
    entry.handle = registered.handle.clone();
    entry.revoker = registered.revoker;
    entry.expires = registered.expires;
    save(&store);
    Some(registered.handle)
}

/// The key the relay verified, attesting a new one when there is none for
/// this gateway or it is near its end.
fn attested_key(
    client: &Client,
    attester: &Arc<dyn Attester>,
    held: &mut Option<Key>,
    sandbox: bool,
) -> Option<Key> {
    if let Some(key) = held.as_ref()
        && key.production == !sandbox
        && key.expires > now() + 24 * 60 * 60
    {
        return Some(key.clone());
    }
    let challenge = client.challenge().ok()?;
    let (key_id, attestation) = attester.attest(sha256(challenge.as_bytes()))?;
    match client.attest(&key_id, &attestation, &challenge) {
        Ok(attested) => {
            let key = Key {
                key_id: hex::encode(key_id),
                sealed: attested.key,
                production: attested.production,
                expires: attested.expires,
            };
            *held = Some(key.clone());
            Some(key)
        }
        Err(e) => {
            log::info!("push relay: attesting: {e}");
            None
        }
    }
}

/// A handle for the Live Activity whose token is `token` (hex), for `server`
/// to update it by. Needs the device registered for that server first.
pub fn activity_handle(server: &str, token: &str) -> Option<String> {
    let _working = WORKING.lock();
    let store = load();
    let entry = store.servers.get(server)?;
    let slot = slot_of(&entry.slot)?;
    let token_bytes = hex::decode(token).ok()?;
    match Client::new(RELAY)
        .ok()?
        .register_activity(&entry.revoker, &token_bytes, &slot)
    {
        Ok(a) => Some(a.handle),
        Err(e) => {
            log::info!("push relay: registering a Live Activity: {e}");
            None
        }
    }
}

/// Retire every handle this device has been given for `server`, and forget
/// it: what signing out of that server does. Its other servers' handles are
/// retired with it, so they register again on their next link.
pub fn revoke(server: &str) {
    let _working = WORKING.lock();
    let mut store = load();
    let Some(entry) = store.servers.remove(server) else {
        return;
    };
    if !entry.revoker.is_empty()
        && let Ok(client) = Client::new(RELAY)
        && let Err(e) = client.revoke(&entry.revoker)
    {
        log::info!("push relay: revoking: {e}");
    }
    // One revocation retires every handle the device holds.
    for other in store.servers.values_mut() {
        other.handle.clear();
        other.revoker.clear();
    }
    save(&store);
}
