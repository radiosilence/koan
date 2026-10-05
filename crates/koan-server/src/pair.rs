//! Pairing: signing in a device with no keyboard, such as a television.
//!
//! The device opens `/rest/koanPair`, unauthenticated, and is held there with
//! a pairing: an unguessable id for a link and a short code to read off the
//! screen. Someone signed in approves it through `/rest/koanPairApprove` or
//! the web UI's `/pair`, and an API key made on their account goes down the
//! waiting socket. Pairings live in memory only: one outlives neither its
//! socket nor ten minutes, so a restart loses nothing a device cannot ask for
//! again. See `koan_core::remote::pair` for the device's side.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use axum::extract::RawQuery;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use koan_core::remote::pair::PairMessage;
use parking_lot::Mutex;
use tokio::sync::oneshot;

use crate::auth::routes::RateLimiter;

/// How long a pairing waits for someone to approve it.
pub const TTL: Duration = Duration::from_secs(600);
/// Pairings waiting at once, across every address: each holds a socket.
const MAX_PENDING: usize = 256;
/// Crockford's base32: no I, L, O or U, so a code read off a screen cannot be
/// mistyped as another.
const CODE_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const CODE_LEN: usize = 8;
/// What keeps a waiting socket open through proxies that drop idle ones.
const KEEPALIVE: Duration = Duration::from_secs(30);

struct Entry {
    /// Without its dash.
    code: String,
    device: String,
    /// Where the request came from, as the rate limits see it.
    from: IpAddr,
    created: Instant,
    outcome: oneshot::Sender<PairMessage>,
}

pub struct Pairings {
    ttl: Duration,
    entries: Mutex<HashMap<String, Entry>>,
}

/// One set per process: the socket route, the Subsonic endpoints and the web
/// UI all reach the same pairings.
pub fn pairings() -> &'static Pairings {
    static PAIRINGS: LazyLock<Pairings> = LazyLock::new(|| Pairings::new(TTL));
    &PAIRINGS
}

/// Pairings opened per address in a minute. Each one is a held socket, and a
/// device needs one.
static OPENS: LazyLock<RateLimiter> = LazyLock::new(|| RateLimiter::new(60, 10));

/// A pairing open for a device, removed when this is dropped: when its socket
/// closes, or the upgrade never happens.
pub struct Opened<'a> {
    pub id: String,
    /// `XXXX-XXXX`.
    pub code: String,
    outcome: oneshot::Receiver<PairMessage>,
    pairings: &'a Pairings,
}

impl Drop for Opened<'_> {
    fn drop(&mut self) {
        self.pairings.entries.lock().remove(&self.id);
    }
}

/// What an approver is shown of a pairing before saying yes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairInfo {
    pub device: String,
    pub from: IpAddr,
}

impl PairInfo {
    /// Whether the request came from a private network: the approver's own,
    /// most likely, rather than the internet.
    pub fn local(&self) -> bool {
        is_local(self.from)
    }
}

/// RFC 1918, link-local, unique local and loopback addresses.
fn is_local(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(v4) => v4.is_private() || v4.is_link_local() || v4.is_loopback(),
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                || (v6.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

/// A pairing taken out to be settled.
pub struct Taken {
    id: String,
    entry: Entry,
}

impl Taken {
    pub fn device(&self) -> &str {
        &self.entry.device
    }

    pub fn info(&self) -> PairInfo {
        PairInfo {
            device: self.entry.device.clone(),
            from: self.entry.from,
        }
    }

    /// Send the device its outcome. Gives it back when the device has gone.
    pub fn settle(self, outcome: PairMessage) -> Result<(), PairMessage> {
        self.entry.outcome.send(outcome)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum OpenError {
    Full,
    Entropy,
}

impl Pairings {
    fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            entries: Mutex::default(),
        }
    }

    /// Open a pairing for the device called `device`, asked for from `from`.
    pub fn open(&self, device: &str, from: IpAddr) -> Result<Opened<'_>, OpenError> {
        let id = koan_core::auth::random_api_key().map_err(|_| OpenError::Entropy)?;
        let (tx, rx) = oneshot::channel();
        let mut entries = self.entries.lock();
        self.sweep(&mut entries);
        if entries.len() >= MAX_PENDING {
            return Err(OpenError::Full);
        }
        let code = loop {
            let code = new_code()?;
            if !entries.values().any(|e| e.code == code) {
                break code;
            }
        };
        entries.insert(
            id.clone(),
            Entry {
                code: code.clone(),
                device: koan_core::invite::device_name(device),
                from: from.to_canonical(),
                created: Instant::now(),
                outcome: tx,
            },
        );
        Ok(Opened {
            id,
            code: format!("{}-{}", &code[..4], &code[4..]),
            outcome: rx,
            pairings: self,
        })
    }

    /// The device waiting on `pair`, an id or a code, and where it asked from.
    pub fn info(&self, pair: &str) -> Option<PairInfo> {
        let mut entries = self.entries.lock();
        self.sweep(&mut entries);
        let id = find(&entries, pair)?;
        entries.get(&id).map(|e| PairInfo {
            device: e.device.clone(),
            from: e.from,
        })
    }

    /// Take the pairing `pair` out, to settle it. No one else can settle it
    /// meanwhile; `put_back` returns it if it could not be.
    pub fn take(&self, pair: &str) -> Option<Taken> {
        let mut entries = self.entries.lock();
        self.sweep(&mut entries);
        let id = find(&entries, pair)?;
        let entry = entries.remove(&id)?;
        Some(Taken { id, entry })
    }

    /// Unless its device went while it was out.
    pub fn put_back(&self, taken: Taken) {
        if !taken.entry.outcome.is_closed() {
            self.entries.lock().insert(taken.id, taken.entry);
        }
    }

    /// Approve the pairing `pair` as the account `user_id` (`username`), with an
    /// API key named after the device, or decline it. Answers with what it
    /// settled.
    pub fn settle(
        &self,
        conn: &rusqlite::Connection,
        pair: &str,
        user_id: i64,
        username: &str,
        decline: bool,
    ) -> Result<PairInfo, SettleError> {
        use koan_core::db::queries::api_keys;
        let taken = self.take(pair).ok_or(SettleError::NotFound)?;
        let info = taken.info();
        let device = info.device.clone();
        if decline {
            let _ = taken.settle(PairMessage::Declined);
            log::info!("pair: {device} declined by {username}");
            return Ok(info);
        }
        let api_key = match api_keys::create_api_key(conn, user_id, &device) {
            Ok((_, key)) => key,
            Err(e) => {
                self.put_back(taken);
                return Err(SettleError::Internal(e.to_string()));
            }
        };
        let approved = PairMessage::Approved {
            username: username.to_owned(),
            api_key: api_key.clone(),
        };
        if taken.settle(approved).is_err() {
            // The device went between the lookup and now: its key would sit unused.
            let _ = api_keys::revoke_api_key_value(conn, &api_key);
            return Err(SettleError::NotFound);
        }
        log::info!("pair: {device} signed in as {username}");
        Ok(info)
    }

    /// Tell every lapsed pairing so, and drop it.
    fn sweep(&self, entries: &mut HashMap<String, Entry>) {
        let lapsed: Vec<String> = entries
            .iter()
            .filter(|(_, e)| e.created.elapsed() >= self.ttl)
            .map(|(id, _)| id.clone())
            .collect();
        for id in lapsed {
            if let Some(e) = entries.remove(&id) {
                let _ = e.outcome.send(PairMessage::Expired);
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum SettleError {
    /// No such pairing, it lapsed, or its device has gone.
    NotFound,
    Internal(String),
}

fn new_code() -> Result<String, OpenError> {
    let mut bytes = [0u8; CODE_LEN];
    getrandom::fill(&mut bytes).map_err(|_| OpenError::Entropy)?;
    // 32 symbols divide 256 evenly, so every one is equally likely.
    Ok(bytes
        .iter()
        .map(|b| CODE_ALPHABET[(b % 32) as usize] as char)
        .collect())
}

/// A code as someone typed it: any case, dashes and spaces anywhere, and the
/// letters Crockford reads as digits.
fn normalise_code(typed: &str) -> Option<String> {
    let code: String = typed
        .chars()
        .filter(|c| !matches!(c, '-' | ' '))
        .map(|c| match c.to_ascii_uppercase() {
            'I' | 'L' => '1',
            'O' => '0',
            c => c,
        })
        .collect();
    (code.len() == CODE_LEN && code.bytes().all(|b| CODE_ALPHABET.contains(&b))).then_some(code)
}

fn find(entries: &HashMap<String, Entry>, pair: &str) -> Option<String> {
    let pair = pair.trim();
    if entries.contains_key(pair) {
        return Some(pair.to_owned());
    }
    let code = normalise_code(pair)?;
    entries
        .iter()
        .find(|(_, e)| e.code == code)
        .map(|(id, _)| id.clone())
}

/// `/rest/koanPair?name=…`: a device asking to be signed in. Unauthenticated,
/// since it has nothing to sign in with yet.
pub(crate) async fn route(
    RawQuery(raw): RawQuery,
    ws: WebSocketUpgrade,
    request: axum::extract::Request,
) -> Response {
    let from = crate::auth::routes::client_ip(&request);
    if !OPENS.allow(from) {
        return (StatusCode::TOO_MANY_REQUESTS, "too many pairings").into_response();
    }
    let name = form_urlencoded::parse(raw.unwrap_or_default().as_bytes())
        .find(|(k, _)| k == "name")
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default();
    match pairings().open(&name, from) {
        Ok(opened) => ws.on_upgrade(move |socket| session(socket, opened)),
        Err(OpenError::Full) => {
            (StatusCode::SERVICE_UNAVAILABLE, "too many pairings waiting").into_response()
        }
        Err(OpenError::Entropy) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn session(mut socket: WebSocket, mut opened: Opened<'static>) {
    let pending = PairMessage::Pending {
        id: opened.id.clone(),
        code: opened.code.clone(),
        expires_in: TTL.as_secs(),
    };
    if send(&mut socket, &pending).await.is_err() {
        return;
    }
    log::info!("pair: {} waiting", opened.code);
    let lapse = tokio::time::sleep(TTL);
    tokio::pin!(lapse);
    let mut keepalive =
        tokio::time::interval_at(tokio::time::Instant::now() + KEEPALIVE, KEEPALIVE);
    let outcome = loop {
        tokio::select! {
            // A dropped sender is a sweep that found it lapsed.
            outcome = &mut opened.outcome => break outcome.unwrap_or(PairMessage::Expired),
            () = &mut lapse => break PairMessage::Expired,
            _ = keepalive.tick() => {
                if socket.send(Message::Ping(Default::default())).await.is_err() {
                    return;
                }
            }
            msg = socket.recv() => match msg {
                Some(Ok(Message::Close(_)) | Err(_)) | None => return,
                Some(Ok(_)) => {}
            },
        }
    };
    log::info!(
        "pair: {} {}",
        opened.code,
        match outcome {
            PairMessage::Approved { .. } => "approved",
            PairMessage::Declined => "declined",
            _ => "expired",
        }
    );
    let _ = send(&mut socket, &outcome).await;
    let _ = socket.send(Message::Close(None)).await;
}

async fn send(socket: &mut WebSocket, message: &PairMessage) -> Result<(), axum::Error> {
    let text = serde_json::to_string(message).unwrap_or_default();
    socket.send(Message::Text(text.into())).await
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAN: IpAddr = IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 1, 20));

    fn leaked(ttl: Duration) -> &'static Pairings {
        Box::leak(Box::new(Pairings::new(ttl)))
    }

    #[test]
    fn codes_are_crockford_and_dashed() {
        let p = leaked(TTL);
        for _ in 0..50 {
            let o = p.open("tv", LAN).unwrap();
            let (a, b) = o.code.split_once('-').unwrap();
            assert_eq!((a.len(), b.len()), (4, 4));
            assert!(
                format!("{a}{b}")
                    .bytes()
                    .all(|c| CODE_ALPHABET.contains(&c) && !b"ILOU".contains(&c))
            );
        }
    }

    #[test]
    fn ids_are_unguessable_and_distinct() {
        let p = leaked(TTL);
        let opened: Vec<_> = (0..100).map(|_| p.open("tv", LAN).unwrap()).collect();
        let ids: std::collections::HashSet<_> = opened.iter().map(|o| o.id.clone()).collect();
        assert_eq!(ids.len(), 100);
        assert!(opened.iter().all(|o| o.id.len() == 43));
        let codes: std::collections::HashSet<_> = opened.iter().map(|o| o.code.clone()).collect();
        assert_eq!(codes.len(), 100);
    }

    #[test]
    fn a_code_is_found_however_it_is_typed() {
        let p = leaked(TTL);
        let o = p.open("Living room\u{7} TV", LAN).unwrap();
        assert_eq!(
            p.info(&o.id).map(|i| i.device).as_deref(),
            Some("Living room TV")
        );
        let bare = o.code.replace('-', "");
        for typed in [
            o.code.clone(),
            o.code.to_lowercase(),
            bare.clone(),
            format!(" {} {} ", &bare[..4], &bare[4..]),
        ] {
            assert_eq!(
                p.info(&typed).map(|i| i.device).as_deref(),
                Some("Living room TV"),
                "{typed}"
            );
        }
        assert_eq!(normalise_code("o1l1-IOAB").as_deref(), Some("011110AB"));
        assert_eq!(normalise_code("ABCD-EFGU"), None);
        assert_eq!(normalise_code("ABC"), None);
    }

    #[tokio::test]
    async fn approving_delivers_the_key_to_the_waiting_device() {
        let p = leaked(TTL);
        let mut o = p.open("tv", LAN).unwrap();
        let taken = p.take(&o.code).unwrap();
        assert_eq!(taken.device(), "tv");
        // Taken: no one else can settle it.
        assert!(p.take(&o.id).is_none());
        taken
            .settle(PairMessage::Approved {
                username: "alice".into(),
                api_key: "key".into(),
            })
            .unwrap();
        assert_eq!(
            (&mut o.outcome).await.unwrap(),
            PairMessage::Approved {
                username: "alice".into(),
                api_key: "key".into(),
            }
        );
    }

    fn database() -> (tempfile::TempDir, koan_core::db::connection::Database, i64) {
        let dir = tempfile::tempdir().unwrap();
        let db = koan_core::db::connection::Database::open(&dir.path().join("koan.db")).unwrap();
        let id = koan_core::db::queries::auth::create_user(
            &db.conn,
            "alice",
            "hunter2",
            koan_core::auth::Role::Readonly,
        )
        .unwrap();
        (dir, db, id)
    }

    #[tokio::test]
    async fn approving_mints_a_key_for_the_approver() {
        let (_dir, db, alice) = database();
        let p = leaked(TTL);
        let mut o = p.open("Living room TV", LAN).unwrap();
        let settled = p.settle(&db.conn, &o.code, alice, "alice", false).unwrap();
        assert_eq!(
            settled,
            PairInfo {
                device: "Living room TV".into(),
                from: LAN,
            }
        );
        let PairMessage::Approved { username, api_key } = (&mut o.outcome).await.unwrap() else {
            panic!("not approved");
        };
        assert_eq!(username, "alice");
        let user = koan_core::db::queries::api_keys::authenticate_api_key(&db.conn, &api_key)
            .unwrap()
            .unwrap();
        assert_eq!(user.id, alice);
        let keys = koan_core::db::queries::api_keys::list_api_keys(&db.conn, Some(alice)).unwrap();
        assert_eq!(keys[0].name, "Living room TV");
        // Settled once only.
        assert_eq!(
            p.settle(&db.conn, &o.id, alice, "alice", false),
            Err(SettleError::NotFound)
        );
    }

    #[tokio::test]
    async fn declining_mints_nothing() {
        let (_dir, db, alice) = database();
        let p = leaked(TTL);
        let mut o = p.open("tv", LAN).unwrap();
        p.settle(&db.conn, &o.id, alice, "alice", true).unwrap();
        assert_eq!((&mut o.outcome).await.unwrap(), PairMessage::Declined);
        let keys = koan_core::db::queries::api_keys::list_api_keys(&db.conn, Some(alice)).unwrap();
        assert!(keys.is_empty());
    }

    #[test]
    fn a_key_for_a_gone_device_is_revoked() {
        let (_dir, db, alice) = database();
        let p = leaked(TTL);
        let o = p.open("tv", LAN).unwrap();
        let id = o.id.clone();
        // The socket closes while an approval holds the pairing.
        let taken = p.take(&id).unwrap();
        drop(o);
        p.entries.lock().insert(taken.id, taken.entry);
        assert_eq!(
            p.settle(&db.conn, &id, alice, "alice", false),
            Err(SettleError::NotFound)
        );
        let keys = koan_core::db::queries::api_keys::list_api_keys(&db.conn, Some(alice)).unwrap();
        assert!(keys.is_empty());
    }

    #[test]
    fn approving_an_unknown_pairing_is_not_found() {
        let (_dir, db, alice) = database();
        let p = leaked(TTL);
        assert_eq!(
            p.settle(&db.conn, "ABCD-EFGH", alice, "alice", false),
            Err(SettleError::NotFound)
        );
    }

    #[test]
    fn a_gone_device_gives_the_outcome_back() {
        let p = leaked(TTL);
        let o = p.open("tv", LAN).unwrap();
        let taken = p.take(&o.id).unwrap();
        drop(o);
        assert!(taken.settle(PairMessage::Declined).is_err());
    }

    #[tokio::test]
    async fn a_lapsed_pairing_is_expired_and_unknown() {
        let p = leaked(Duration::ZERO);
        let mut o = p.open("tv", LAN).unwrap();
        assert!(p.info(&o.id).is_none());
        assert!(p.take(&o.code).is_none());
        assert_eq!((&mut o.outcome).await.unwrap(), PairMessage::Expired);
    }

    #[test]
    fn the_requesting_address_is_kept() {
        let p = leaked(TTL);
        let mapped: IpAddr = "::ffff:203.0.113.9".parse().unwrap();
        let o = p.open("tv", mapped).unwrap();
        let info = p.info(&o.code).unwrap();
        assert_eq!(info.from.to_string(), "203.0.113.9");
        assert!(!info.local());
        assert_eq!(p.take(&o.id).unwrap().info(), info);
    }

    #[test]
    fn private_addresses_are_local() {
        for local in [
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.20",
            "169.254.10.1",
            "127.0.0.1",
            "::1",
            "fd12:3456::1",
            "fe80::1",
            "::ffff:192.168.0.5",
        ] {
            assert!(is_local(local.parse().unwrap()), "{local}");
        }
        for public in [
            "203.0.113.9",
            "8.8.8.8",
            "172.32.0.1",
            "2001:db8::1",
            "2a00:1450::1",
            "::ffff:8.8.8.8",
        ] {
            assert!(!is_local(public.parse().unwrap()), "{public}");
        }
    }

    #[test]
    fn unknown_pairings_are_not_found() {
        let p = leaked(TTL);
        let o = p.open("tv", LAN).unwrap();
        let other = if o.code == "0000-0000" {
            "1111-1111"
        } else {
            "0000-0000"
        };
        assert!(p.info(other).is_none());
        assert!(p.take("not-a-pairing").is_none());
        assert!(p.info("").is_none());
    }

    #[test]
    fn a_closed_socket_removes_its_pairing() {
        let p = leaked(TTL);
        let o = p.open("tv", LAN).unwrap();
        let id = o.id.clone();
        drop(o);
        assert!(p.info(&id).is_none());
    }

    #[test]
    fn pairings_are_capped() {
        let p = leaked(TTL);
        let held: Vec<_> = (0..MAX_PENDING)
            .map(|_| p.open("tv", LAN).unwrap())
            .collect();
        assert_eq!(p.open("tv", LAN).err(), Some(OpenError::Full));
        drop(held);
        assert!(p.open("tv", LAN).is_ok());
    }
}
