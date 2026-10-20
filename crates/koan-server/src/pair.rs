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

use crate::auth::password::FailureLimiter;
use crate::auth::routes::{RateLimiter, network};

/// How long a pairing waits for someone to approve it.
pub const TTL: Duration = Duration::from_secs(600);
/// Pairings waiting at once, across every address: each holds a socket.
const MAX_PENDING: usize = 256;
/// Pairings waiting at once from one address, or one IPv6 /64: a household
/// sets up a device or two at a time, and a few addresses must not be able to
/// fill `MAX_PENDING`.
const MAX_PENDING_PER_ADDRESS: usize = 3;
/// Crockford's base32: no I, L, O or U, so a code read off a screen cannot be
/// mistyped as another.
const CODE_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const CODE_LEN: usize = 8;
/// What keeps a waiting socket open through proxies that drop idle ones, and
/// what a live device answers.
const KEEPALIVE: Duration = Duration::from_secs(30);
/// Pairing codes and ids one account may look up and not find in a minute.
/// Someone typing a code gets it wrong a few times at most; a guesser
/// against 2^32 codes at this rate gets nowhere.
const MISSES_PER_MINUTE: u32 = 10;
/// How long an approval waits for the session to say the device was sent its
/// key, before taking it as lost.
const DELIVERY: Duration = Duration::from_secs(10);

struct Entry {
    /// Without its dash.
    code: String,
    device: String,
    /// Where the request came from, as the rate limits see it.
    from: IpAddr,
    created: Instant,
    outcome: oneshot::Sender<Settled>,
}

/// What settling a pairing hands its waiting session.
pub struct Settled {
    pub message: PairMessage,
    /// For an approval: where the session says whether the socket took it.
    sent: Option<std::sync::mpsc::SyncSender<bool>>,
}

impl From<PairMessage> for Settled {
    fn from(message: PairMessage) -> Self {
        Self {
            message,
            sent: None,
        }
    }
}

pub struct Pairings {
    ttl: Duration,
    keepalive: Duration,
    entries: Mutex<HashMap<String, Entry>>,
    /// Lookups that found nothing, by the account that made them.
    misses: FailureLimiter<i64>,
}

/// One set per process: the socket route, the Subsonic endpoints and the web
/// UI all reach the same pairings.
pub fn pairings() -> &'static Pairings {
    static PAIRINGS: LazyLock<Pairings> = LazyLock::new(|| Pairings::new(ttl_from_env()));
    &PAIRINGS
}

/// `KOAN_PAIR_TTL_SECS`, or `TTL`. For trying expiry without waiting ten
/// minutes; read once, when the first pairing is asked for.
fn ttl_from_env() -> Duration {
    std::env::var("KOAN_PAIR_TTL_SECS")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .filter(|&secs| secs > 0)
        .map_or(TTL, Duration::from_secs)
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
    outcome: oneshot::Receiver<Settled>,
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

/// RFC 1918, shared (CGNAT, which Tailscale addresses come from),
/// link-local, unique local and loopback addresses.
fn is_local(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            v4.is_private()
                || (a == 100 && (b & 0xc0) == 64)
                || v4.is_link_local()
                || v4.is_loopback()
        }
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
        self.entry
            .outcome
            .send(outcome.into())
            .map_err(|settled| settled.message)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum OpenError {
    Full,
    /// This address already has `MAX_PENDING_PER_ADDRESS` waiting.
    Busy,
    Entropy,
}

impl Pairings {
    fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            keepalive: KEEPALIVE,
            entries: Mutex::default(),
            misses: FailureLimiter::new(MISSES_PER_MINUTE),
        }
    }

    /// How long a socket may stay silent before its device is taken as gone:
    /// two pings unanswered, and some slack.
    fn silence(&self) -> Duration {
        self.keepalive * 5 / 2
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
        let from = from.to_canonical();
        let waiting = entries
            .values()
            .filter(|e| network(e.from) == network(from))
            .count();
        if waiting >= MAX_PENDING_PER_ADDRESS {
            return Err(OpenError::Busy);
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
                from,
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

    /// The device waiting on `pair`, an id or a code, and where it asked
    /// from, as the account `caller` asks.
    pub fn info(&self, pair: &str, caller: i64) -> Result<PairInfo, PairError> {
        let mut entries = self.entries.lock();
        self.sweep(&mut entries);
        let id = self.lookup(&entries, pair, caller)?;
        let e = &entries[&id];
        Ok(PairInfo {
            device: e.device.clone(),
            from: e.from,
        })
    }

    /// Take the pairing `pair` out, to settle it. No one else can settle it
    /// meanwhile; `put_back` returns it if it could not be.
    pub fn take(&self, pair: &str, caller: i64) -> Result<Taken, PairError> {
        let mut entries = self.entries.lock();
        self.sweep(&mut entries);
        let id = self.lookup(&entries, pair, caller)?;
        let entry = entries.remove(&id).ok_or(PairError::NotFound)?;
        Ok(Taken { id, entry })
    }

    /// The pairing `pair` names, counting a miss against `caller`. One that
    /// has missed too often finds nothing by code, right or wrong, until its
    /// window is out (RFC 8628 §5.1). An id is unguessable and always found,
    /// so a link followed after a few mistyped codes still works.
    fn lookup(
        &self,
        entries: &HashMap<String, Entry>,
        pair: &str,
        caller: i64,
    ) -> Result<String, PairError> {
        let pair = pair.trim();
        if entries.contains_key(pair) {
            return Ok(pair.to_owned());
        }
        if self.misses.exhausted(&caller) {
            return Err(PairError::Limited);
        }
        find(entries, pair).ok_or_else(|| {
            self.misses.record(caller);
            PairError::NotFound
        })
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
    ) -> Result<PairInfo, PairError> {
        use koan_core::db::queries::api_keys;
        let taken = self.take(pair, user_id)?;
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
                return Err(PairError::Internal(e.to_string()));
            }
        };
        let approved = PairMessage::Approved {
            username: username.to_owned(),
            api_key: api_key.clone(),
        };
        // Answered only once the session has written the key to the socket,
        // so the approver is never told of a sign-in whose key was lost.
        let (sent_tx, sent_rx) = std::sync::mpsc::sync_channel(1);
        let settled = Settled {
            message: approved,
            sent: Some(sent_tx),
        };
        let sent = taken.entry.outcome.send(settled).is_ok()
            && sent_rx.recv_timeout(DELIVERY).unwrap_or(false);
        if !sent {
            // The device went, or its socket would not take the key, which
            // would otherwise sit on the account unused.
            let _ = api_keys::revoke_api_key_value(conn, &api_key);
            log::warn!("pair: {device} went before its key reached it");
            return Err(PairError::NotFound);
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
                let _ = e.outcome.send(PairMessage::Expired.into());
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum PairError {
    /// No such pairing, it lapsed, or its device has gone.
    NotFound,
    /// The caller has looked up too many pairings that were not there.
    Limited,
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
    // Browsers send an Origin with every WebSocket and apps do not. Without
    // this, a page someone on the network visits could open a pairing from
    // their address, have them approve it as "on your network", and read the
    // key it was sent.
    if request.headers().contains_key(axum::http::header::ORIGIN) {
        return (StatusCode::FORBIDDEN, "pairing is not open to web pages").into_response();
    }
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
        Err(OpenError::Busy) => (
            StatusCode::TOO_MANY_REQUESTS,
            "too many pairings waiting from this address",
        )
            .into_response(),
        Err(OpenError::Entropy) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// Hold the device until its pairing is settled, then tell it the outcome,
/// and tell an approval whether the socket took it.
async fn session(mut socket: WebSocket, mut opened: Opened<'static>) {
    let waited = wait(&mut socket, &mut opened).await;
    // Nothing reads the outcome after this, so `settle` now finds the device
    // gone. One settled just before is read here; dropped unsent, as when
    // the device went first, its approval is revoked by `settle`.
    opened.outcome.close();
    let late = opened.outcome.try_recv().ok();
    let Some(waited) = waited else { return };
    // A lapse can race an approval, which then wins.
    let settled = late.unwrap_or(waited);
    log::info!(
        "pair: {} {}",
        opened.code,
        match settled.message {
            PairMessage::Approved { .. } => "approved",
            PairMessage::Declined => "declined",
            _ => "expired",
        }
    );
    let sent = send(&mut socket, &settled.message).await.is_ok();
    if let Some(done) = settled.sent {
        let _ = done.send(sent);
    }
    let _ = socket.send(Message::Close(None)).await;
}

/// Wait for the pairing's outcome. `None` when the device went first: it
/// closed the socket, or stopped answering pings.
async fn wait(socket: &mut WebSocket, opened: &mut Opened<'static>) -> Option<Settled> {
    let pending = PairMessage::Pending {
        id: opened.id.clone(),
        code: opened.code.clone(),
        expires_in: opened.pairings.ttl.as_secs(),
    };
    send(socket, &pending).await.ok()?;
    log::info!("pair: {} waiting", opened.code);
    let lapse = tokio::time::sleep(opened.pairings.ttl);
    tokio::pin!(lapse);
    let keepalive = opened.pairings.keepalive;
    let mut ping = tokio::time::interval_at(tokio::time::Instant::now() + keepalive, keepalive);
    let silence = opened.pairings.silence();
    let quiet = tokio::time::sleep(silence);
    tokio::pin!(quiet);
    loop {
        tokio::select! {
            // A dropped sender is a sweep that found it lapsed.
            outcome = &mut opened.outcome => {
                return Some(outcome.unwrap_or_else(|_| PairMessage::Expired.into()));
            }
            () = &mut lapse => return Some(PairMessage::Expired.into()),
            () = &mut quiet => {
                log::info!("pair: {} stopped answering", opened.code);
                return None;
            }
            _ = ping.tick() => socket.send(Message::Ping(Default::default())).await.ok()?,
            msg = socket.recv() => match msg {
                Some(Ok(Message::Close(_)) | Err(_)) | None => return None,
                Some(Ok(_)) => quiet.as_mut().reset(tokio::time::Instant::now() + silence),
            },
        }
    }
}

async fn send(socket: &mut WebSocket, message: &PairMessage) -> Result<(), axum::Error> {
    let text = serde_json::to_string(message).unwrap_or_default();
    socket.send(Message::Text(text.into())).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The account looking pairings up, where none is in a database.
    const ME: i64 = 1;
    const LAN: IpAddr = IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 1, 20));

    /// A distinct address per `i`, so the per-address cap stays out of the way.
    fn host(i: usize) -> IpAddr {
        IpAddr::V4(std::net::Ipv4Addr::new(
            10,
            9,
            (i / 256) as u8,
            (i % 256) as u8,
        ))
    }

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
        let opened: Vec<_> = (0..100).map(|i| p.open("tv", host(i)).unwrap()).collect();
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
            p.info(&o.id, ME).ok().map(|i| i.device).as_deref(),
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
                p.info(&typed, ME).ok().map(|i| i.device).as_deref(),
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
        let taken = p.take(&o.code, ME).unwrap();
        assert_eq!(taken.device(), "tv");
        // Taken: no one else can settle it.
        assert!(p.take(&o.id, ME).is_err());
        taken
            .settle(PairMessage::Approved {
                username: "alice".into(),
                api_key: "key".into(),
            })
            .unwrap();
        assert_eq!(
            (&mut o.outcome).await.unwrap().message,
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

    /// A device's session: takes its outcome, and says whether the socket
    /// took it.
    fn device(mut o: Opened<'static>, sent: bool) -> std::thread::JoinHandle<PairMessage> {
        std::thread::spawn(move || {
            // `Opened` is `Drop`, so its receiver is swapped out, not moved.
            let outcome = std::mem::replace(&mut o.outcome, oneshot::channel().1);
            let settled = outcome.blocking_recv().unwrap();
            if let Some(done) = settled.sent {
                done.send(sent).unwrap();
            }
            settled.message
        })
    }

    #[test]
    fn approving_mints_a_key_for_the_approver() {
        let (_dir, db, alice) = database();
        let p = leaked(TTL);
        let o = p.open("Living room TV", LAN).unwrap();
        let (id, code) = (o.id.clone(), o.code.clone());
        let device = device(o, true);
        let settled = p.settle(&db.conn, &code, alice, "alice", false).unwrap();
        assert_eq!(
            settled,
            PairInfo {
                device: "Living room TV".into(),
                from: LAN,
            }
        );
        let PairMessage::Approved { username, api_key } = device.join().unwrap() else {
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
            p.settle(&db.conn, &id, alice, "alice", false),
            Err(PairError::NotFound)
        );
    }

    #[test]
    fn a_key_the_socket_would_not_take_is_revoked() {
        let (_dir, db, alice) = database();
        let p = leaked(TTL);
        let o = p.open("tv", LAN).unwrap();
        let id = o.id.clone();
        let device = device(o, false);
        // The approver is told, rather than shown a sign-in that never happened.
        assert_eq!(
            p.settle(&db.conn, &id, alice, "alice", false),
            Err(PairError::NotFound)
        );
        let PairMessage::Approved { api_key, .. } = device.join().unwrap() else {
            panic!("not approved");
        };
        assert!(
            koan_core::db::queries::api_keys::authenticate_api_key(&db.conn, &api_key)
                .unwrap()
                .is_none()
        );
        let keys = koan_core::db::queries::api_keys::list_api_keys(&db.conn, Some(alice)).unwrap();
        assert!(keys.is_empty());
    }

    #[tokio::test]
    async fn declining_mints_nothing() {
        let (_dir, db, alice) = database();
        let p = leaked(TTL);
        let mut o = p.open("tv", LAN).unwrap();
        p.settle(&db.conn, &o.id, alice, "alice", true).unwrap();
        assert_eq!(
            (&mut o.outcome).await.unwrap().message,
            PairMessage::Declined
        );
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
        let taken = p.take(&id, ME).unwrap();
        drop(o);
        p.entries.lock().insert(taken.id, taken.entry);
        assert_eq!(
            p.settle(&db.conn, &id, alice, "alice", false),
            Err(PairError::NotFound)
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
            Err(PairError::NotFound)
        );
    }

    #[test]
    fn a_gone_device_gives_the_outcome_back() {
        let p = leaked(TTL);
        let o = p.open("tv", LAN).unwrap();
        let taken = p.take(&o.id, ME).unwrap();
        drop(o);
        assert!(taken.settle(PairMessage::Declined).is_err());
    }

    #[tokio::test]
    async fn a_lapsed_pairing_is_expired_and_unknown() {
        let p = leaked(Duration::ZERO);
        let mut o = p.open("tv", LAN).unwrap();
        assert!(p.info(&o.id, ME).is_err());
        assert!(p.take(&o.code, ME).is_err());
        assert_eq!(
            (&mut o.outcome).await.unwrap().message,
            PairMessage::Expired
        );
    }

    #[test]
    fn the_requesting_address_is_kept() {
        let p = leaked(TTL);
        let mapped: IpAddr = "::ffff:203.0.113.9".parse().unwrap();
        let o = p.open("tv", mapped).unwrap();
        let info = p.info(&o.code, ME).unwrap();
        assert_eq!(info.from.to_string(), "203.0.113.9");
        assert!(!info.local());
        assert_eq!(p.take(&o.id, ME).unwrap().info(), info);
    }

    #[test]
    fn private_addresses_are_local() {
        for local in [
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.20",
            "169.254.10.1",
            "127.0.0.1",
            "100.64.0.1",
            "100.101.102.103",
            "100.127.255.254",
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
            "100.63.255.255",
            "100.128.0.1",
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
        assert!(p.info(other, ME).is_err());
        assert!(p.take("not-a-pairing", ME).is_err());
        assert!(p.info("", ME).is_err());
    }

    #[test]
    fn misses_are_limited_per_account() {
        let p = leaked(TTL);
        let o = p.open("tv", LAN).unwrap();
        let other = if o.code == "0000-0000" {
            "1111-1111"
        } else {
            "0000-0000"
        };
        for _ in 0..MISSES_PER_MINUTE {
            assert_eq!(p.info(other, ME).err(), Some(PairError::NotFound));
        }
        // Spent: no code is found, the right one included.
        assert_eq!(p.info(&o.code, ME).err(), Some(PairError::Limited));
        assert_eq!(p.info(other, ME).err(), Some(PairError::Limited));
        // An id is unguessable: a link still works.
        assert!(p.info(&o.id, ME).is_ok());
        // Finding one does not count, and another account has its own.
        for _ in 0..=MISSES_PER_MINUTE {
            assert!(p.info(&o.code, ME + 1).is_ok());
        }
    }

    /// Serves pairings from `p` the way `route` does.
    async fn serve(p: &'static Pairings) -> std::net::SocketAddr {
        let app = axum::Router::new().route(
            "/",
            axum::routing::get(move |ws: WebSocketUpgrade| async move {
                let opened = p.open("tv", LAN).unwrap();
                ws.on_upgrade(move |socket| session(socket, opened))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await });
        addr
    }

    fn quick(keepalive: Duration) -> &'static Pairings {
        let mut p = Pairings::new(TTL);
        p.keepalive = keepalive;
        Box::leak(Box::new(p))
    }

    fn pending_id(
        socket: &mut tungstenite::WebSocket<impl std::io::Read + std::io::Write>,
    ) -> String {
        let first = socket.read().unwrap();
        match serde_json::from_str(first.to_text().unwrap()).unwrap() {
            PairMessage::Pending { id, .. } => id,
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_device_that_stops_answering_is_dropped() {
        // Seconds, not milliseconds: a busy runner must not make a device
        // that answers look silent.
        let p = quick(Duration::from_secs(1));
        let addr = serve(p).await;
        // Connected, then never read again, so pings go unanswered.
        let (socket, id) = tokio::task::spawn_blocking(move || {
            let (mut socket, _) = tungstenite::connect(format!("ws://{addr}/")).unwrap();
            let id = pending_id(&mut socket);
            (socket, id)
        })
        .await
        .unwrap();
        assert!(p.info(&id, ME).is_ok());
        tokio::time::sleep(p.silence() * 2).await;
        assert_eq!(p.info(&id, ME).err(), Some(PairError::NotFound));
        drop(socket);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_device_that_answers_waits_and_gets_its_key() {
        let (_dir, db, alice) = database();
        let p = quick(Duration::from_secs(1));
        let addr = serve(p).await;
        let (id_tx, id_rx) = oneshot::channel();
        let device = tokio::task::spawn_blocking(move || {
            let (mut socket, _) = tungstenite::connect(format!("ws://{addr}/")).unwrap();
            id_tx.send(pending_id(&mut socket)).unwrap();
            // Reading answers the pings.
            loop {
                if let tungstenite::Message::Text(text) = socket.read().unwrap() {
                    return serde_json::from_str::<PairMessage>(&text).unwrap();
                }
            }
        });
        let id = id_rx.await.unwrap();
        tokio::time::sleep(p.silence() * 2).await;
        // `settle` blocks until the session has written the key.
        let db = tokio::task::spawn_blocking(move || {
            p.settle(&db.conn, &id, alice, "alice", false).unwrap();
            db
        })
        .await
        .unwrap();
        let PairMessage::Approved { api_key, .. } = device.await.unwrap() else {
            panic!("not approved");
        };
        assert!(
            koan_core::db::queries::api_keys::authenticate_api_key(&db.conn, &api_key)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn a_closed_socket_removes_its_pairing() {
        let p = leaked(TTL);
        let o = p.open("tv", LAN).unwrap();
        let id = o.id.clone();
        drop(o);
        assert!(p.info(&id, ME).is_err());
    }

    #[test]
    fn pairings_are_capped() {
        let p = leaked(TTL);
        let held: Vec<_> = (0..MAX_PENDING)
            .map(|i| p.open("tv", host(i)).unwrap())
            .collect();
        assert_eq!(p.open("tv", LAN).err(), Some(OpenError::Full));
        drop(held);
        assert!(p.open("tv", LAN).is_ok());
    }

    #[test]
    fn one_address_holds_three_at_most() {
        let p = leaked(TTL);
        let held: Vec<_> = (0..3).map(|_| p.open("tv", LAN).unwrap()).collect();
        assert_eq!(p.open("tv", LAN).err(), Some(OpenError::Busy));
        // The same address written as IPv4-mapped IPv6 is the same address.
        let mapped: IpAddr = "::ffff:192.168.1.20".parse().unwrap();
        assert_eq!(p.open("tv", mapped).err(), Some(OpenError::Busy));
        assert!(p.open("tv", host(1)).is_ok());

        // IPv6 counts by /64: a client picks any address in its own.
        let v6: Vec<_> = ["2001:db8:1:2::1", "2001:db8:1:2::2", "2001:db8:1:2:ffff::9"]
            .into_iter()
            .map(|a| p.open("tv", a.parse().unwrap()).unwrap())
            .collect();
        assert_eq!(
            p.open("tv", "2001:db8:1:2:abcd::1".parse().unwrap()).err(),
            Some(OpenError::Busy)
        );
        assert!(p.open("tv", "2001:db8:1:3::1".parse().unwrap()).is_ok());

        drop(held);
        assert!(p.open("tv", LAN).is_ok());
        drop(v6);
    }
}
