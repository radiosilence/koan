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

use koan_relay_client::{Assertion, Client, Error, Registered, Slot, client_data};
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
    /// Revokers whose revocation the relay has not yet confirmed. Nothing
    /// registers until they are settled: a handle minted before a revocation
    /// lands is retired by it.
    #[serde(default)]
    revoking: Vec<String>,
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

/// Written as the configuration is: it holds revokers.
fn save(store: &Store) {
    let Ok(json) = serde_json::to_vec_pretty(store) else {
        return;
    };
    if let Err(e) = crate::config::write_private(&path(), &json) {
        log::warn!("push relay: saving registrations: {e}");
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
    if !settle_revocations(&mut store) {
        save(&store);
        return None;
    }
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
    let registered = register(&client, &mut store.key, &token_bytes, &slot, sandbox);
    // The slot, and a key attested on the way, are kept whatever the
    // registration's fate: Apple rations attestations.
    save(&store);
    let registered = match registered? {
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

/// Register `token` under `slot`: attested where the app can attest, and
/// unattested on the sandbox gateway where it cannot. `None` when it cannot
/// be asked at all.
fn register(
    client: &Client,
    held: &mut Option<Key>,
    token: &[u8],
    slot: &Slot,
    sandbox: bool,
) -> Option<Result<Registered, Error>> {
    let attested = (|| {
        let attester = ATTESTER.lock().clone()?;
        let key = attested_key(client, &attester, held, sandbox)?;
        let challenge = client.challenge().ok()?;
        let data = client_data(&challenge, token, slot, sandbox);
        let assertion = attester.assert(hex::decode(&key.key_id).ok()?, sha256(&data))?;
        Some(client.register_attested(
            token,
            slot,
            sandbox,
            &Assertion {
                key: &key.sealed,
                challenge: &challenge,
                assertion: &assertion,
            },
        ))
    })();
    // The relay no longer takes this key (its sealing key rotated, or the key
    // was refused): attest a new one next time.
    if let Some(Err(Error::Bad(why))) = &attested {
        log::info!("push relay: the attested key was refused: {why}");
        *held = None;
    }
    match attested {
        Some(registered) => Some(registered),
        None if sandbox => Some(client.register(token, slot, true)),
        None => {
            log::warn!("push relay: App Attest is unavailable, so this device cannot register");
            None
        }
    }
}

/// The server whose slot is `slot` (hex): which server a notification came
/// through, by what the relay stamped on it.
/// Read without waiting for a registration: the file is replaced whole.
pub fn server_of_slot(slot: &str) -> Option<String> {
    load()
        .servers
        .into_iter()
        .find(|(_, r)| r.slot.eq_ignore_ascii_case(slot))
        .map(|(server, _)| server)
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
    // Apple attests a key once, so an attestation lost on the way to the
    // relay costs a key: try it again while the challenge is good.
    let mut verified = client.attest(&key_id, &attestation, &challenge);
    for wait in [2, 10, 30] {
        match &verified {
            Err(Error::Transport(_) | Error::Unavailable(_) | Error::Limited(_)) => {
                std::thread::sleep(std::time::Duration::from_secs(wait));
                verified = client.attest(&key_id, &attestation, &challenge);
            }
            _ => break,
        }
    }
    match verified {
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
/// it: what signing out of that server, or into another account, does. Its
/// other servers' handles are retired with it, so they register again on
/// their next link. A revocation the relay does not confirm now is kept and
/// sent again before the next registration.
pub fn revoke(server: &str) {
    let _working = WORKING.lock();
    let mut store = load();
    let Some(entry) = store.servers.remove(server) else {
        return;
    };
    if !entry.revoker.is_empty() {
        store.revoking.push(entry.revoker);
    }
    // One revocation retires every handle the device holds.
    for other in store.servers.values_mut() {
        other.handle.clear();
        other.revoker.clear();
    }
    save(&store);
    settle_revocations(&mut store);
    save(&store);
}

/// Send the revocations the relay has not confirmed. Whether none is left.
fn settle_revocations(store: &mut Store) -> bool {
    if store.revoking.is_empty() {
        return true;
    }
    let Ok(client) = Client::new(RELAY) else {
        return false;
    };
    // `Gone`: an earlier attempt landed and retired this revoker with the rest.
    store
        .revoking
        .retain(|revoker| match client.revoke(revoker) {
            Ok(()) | Err(Error::Gone) => false,
            Err(e) => {
                log::info!("push relay: revoking: {e}");
                true
            }
        });
    store.revoking.is_empty()
}
