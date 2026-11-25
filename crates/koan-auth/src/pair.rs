//! Pairing: signing in a device with no keyboard, such as a television.
//!
//! The device asks, unauthenticated, and is held waiting with a pairing: an
//! unguessable id for a link and a short code to read off its screen. Someone
//! signed in approves it, and an API key made on their account is handed to
//! the waiting device. Pairings live in memory only: one outlives neither its
//! device's connection nor its time to live, so a restart loses nothing a
//! device cannot ask for again.
//!
//! The shape is the OAuth device authorization grant's (RFC 8628), without
//! its protocol: how the device waits (kōan holds a WebSocket) and what it is
//! told are the service's.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tokio::sync::oneshot;

use crate::limit::{FailureLimiter, network};

/// Crockford's base 32: no I, L, O or U, so a code read aloud or typed from a
/// screen has one spelling. A device's code is [`CODE_LEN`] of these.
pub const CODE_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
pub const CODE_LEN: usize = 8;

/// How long a pairing waits for someone to approve it.
pub const TTL: Duration = Duration::from_secs(600);
/// Pairings waiting at once, across every address: each holds a connection.
const MAX_PENDING: usize = 256;
/// Pairings waiting at once from one address, or one IPv6 /64: a household
/// sets up a device or two at a time, and a few addresses must not be able to
/// fill `MAX_PENDING`.
const MAX_PENDING_PER_ADDRESS: usize = 3;
/// How often a waiting connection is pinged, through proxies that drop idle
/// ones.
const KEEPALIVE: Duration = Duration::from_secs(30);
/// Pairing codes and ids one account may look up and not find in a minute.
/// Someone typing a code gets it wrong a few times at most; a guesser against
/// 2^40 codes at this rate gets nowhere.
pub const MISSES_PER_MINUTE: u32 = 10;
/// How long an approval waits for the device's connection to say it was sent
/// the key, before taking it as lost.
const DELIVERY: Duration = Duration::from_secs(10);

/// How a pairing ended, for its device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Approved { username: String, api_key: String },
    Declined,
    Expired,
}

/// What settling a pairing hands its waiting device's connection.
pub struct Settled {
    pub outcome: Outcome,
    /// For an approval: where the connection says whether it sent the key.
    sent: Option<std::sync::mpsc::SyncSender<bool>>,
}

impl Settled {
    /// Say whether the device was sent its outcome. An approval it was not
    /// sent is revoked by the approver's [`Pairings::settle`], which waits
    /// for this.
    pub fn delivered(self, sent: bool) {
        if let Some(done) = self.sent {
            let _ = done.send(sent);
        }
    }
}

impl From<Outcome> for Settled {
    fn from(outcome: Outcome) -> Self {
        Self {
            outcome,
            sent: None,
        }
    }
}

struct Entry {
    /// Without its dash.
    code: String,
    device: String,
    /// The id the device says it goes by: how an approver knows it among the
    /// account's devices once it has signed in.
    device_id: Option<String>,
    /// Where the request came from, as the rate limits see it.
    from: IpAddr,
    created: Instant,
    outcome: oneshot::Sender<Settled>,
}

pub struct Pairings {
    ttl: Duration,
    keepalive: Duration,
    entries: Mutex<HashMap<String, Entry>>,
    /// Lookups that found nothing, by the account that made them.
    misses: FailureLimiter<i64>,
}

/// A pairing open for a device, removed when this is dropped: when its
/// connection closes, or never opens.
pub struct Opened<'a> {
    pub id: String,
    /// `XXXX-XXXX`.
    pub code: String,
    /// Where the pairing's outcome arrives. A dropped sender is a sweep that
    /// found it lapsed.
    pub outcome: oneshot::Receiver<Settled>,
    pairings: &'a Pairings,
}

impl<'a> Opened<'a> {
    pub fn pairings(&self) -> &'a Pairings {
        self.pairings
    }
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
    pub device_id: Option<String>,
    pub from: IpAddr,
}

impl PairInfo {
    /// Whether the request came from a private network: the approver's own,
    /// most likely, rather than the internet.
    pub fn local(&self) -> bool {
        is_local(self.from)
    }
}

/// RFC 1918, shared (CGNAT, which Tailscale addresses come from), link-local,
/// unique local and loopback addresses.
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
            device_id: self.entry.device_id.clone(),
            from: self.entry.from,
        }
    }

    /// Send the device its outcome. Gives it back when the device has gone.
    pub fn settle(self, outcome: Outcome) -> Result<(), Outcome> {
        self.entry
            .outcome
            .send(outcome.into())
            .map_err(|settled| settled.outcome)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum OpenError {
    Full,
    /// This address already has as many waiting as one may.
    Busy,
    Entropy,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PairError {
    /// No such pairing, it lapsed, or its device has gone.
    NotFound,
    /// The caller has looked up too many pairings that were not there.
    Limited,
    Internal(String),
}

impl Pairings {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            keepalive: KEEPALIVE,
            entries: Mutex::default(),
            misses: FailureLimiter::new(MISSES_PER_MINUTE),
        }
    }

    /// Ping waiting devices this often instead.
    pub fn with_keepalive(self, keepalive: Duration) -> Self {
        Self { keepalive, ..self }
    }

    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    pub fn keepalive(&self) -> Duration {
        self.keepalive
    }

    /// How long a device's connection may stay silent before the device is
    /// taken as gone: two pings unanswered, and some slack.
    pub fn silence(&self) -> Duration {
        self.keepalive * 5 / 2
    }

    /// Open a pairing for the device called `device`, which goes by
    /// `device_id`, asked for from `from`. An id that is not one a device
    /// would choose (a UUID's characters, at most 64) is not kept.
    pub fn open(
        &self,
        device: &str,
        device_id: Option<&str>,
        from: IpAddr,
    ) -> Result<Opened<'_>, OpenError> {
        let device_id = device_id
            .map(str::trim)
            .filter(|id| {
                !id.is_empty()
                    && id.len() <= 64
                    && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            })
            .map(str::to_owned);
        let id = crate::secret::random_api_key().map_err(|_| OpenError::Entropy)?;
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
                device: crate::api_keys::device_name(device),
                device_id,
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
            device_id: e.device_id.clone(),
            from: e.from,
        })
    }

    /// Take the pairing `pair` out, to settle it. No one else can settle it
    /// meanwhile; [`Pairings::put_back`] returns it if it could not be.
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

    /// Approve the pairing `pair` as the account `user_id` (`username`), with
    /// an API key named after the device, or decline it. Answers with what it
    /// settled.
    ///
    /// Blocks until the device's connection says whether it sent the key, so
    /// the approver is never told of a sign-in whose key was lost: one that
    /// was not sent is revoked.
    pub fn settle(
        &self,
        conn: &rusqlite::Connection,
        pair: &str,
        user_id: i64,
        username: &str,
        decline: bool,
    ) -> Result<PairInfo, PairError> {
        use crate::api_keys;
        let taken = self.take(pair, user_id)?;
        let info = taken.info();
        let device = info.device.clone();
        if decline {
            let _ = taken.settle(Outcome::Declined);
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
        let (sent_tx, sent_rx) = std::sync::mpsc::sync_channel(1);
        let settled = Settled {
            outcome: Outcome::Approved {
                username: username.to_owned(),
                api_key: api_key.clone(),
            },
            sent: Some(sent_tx),
        };
        let sent = taken.entry.outcome.send(settled).is_ok()
            && sent_rx.recv_timeout(DELIVERY).unwrap_or(false);
        if !sent {
            // The device went, or its connection would not take the key,
            // which would otherwise sit on the account unused.
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
                let _ = e.outcome.send(Outcome::Expired.into());
            }
        }
    }
}

fn new_code() -> Result<String, OpenError> {
    use ring::rand::SecureRandom;

    let mut bytes = [0u8; CODE_LEN];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| OpenError::Entropy)?;
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

    impl Opened<'static> {
        /// Stands in for the device's connection, on a thread of its own:
        /// takes the outcome and says whether it was sent, as `sent`.
        fn device(mut self, sent: bool) -> std::thread::JoinHandle<Outcome> {
            std::thread::spawn(move || {
                // `Opened` is `Drop`, so its receiver is swapped out, not moved.
                let outcome = std::mem::replace(&mut self.outcome, oneshot::channel().1);
                let settled = outcome.blocking_recv().unwrap();
                let outcome = settled.outcome.clone();
                settled.delivered(sent);
                outcome
            })
        }
    }

    fn database() -> (rusqlite::Connection, i64) {
        let conn = crate::schema::test_db();
        let id =
            crate::users::create_user(&conn, "alice", "hunter2", crate::Role::Readonly).unwrap();
        (conn, id)
    }

    #[test]
    fn codes_are_crockford_and_dashed() {
        let p = leaked(TTL);
        for _ in 0..50 {
            let o = p.open("tv", None, LAN).unwrap();
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
        let opened: Vec<_> = (0..100)
            .map(|i| p.open("tv", None, host(i)).unwrap())
            .collect();
        let ids: std::collections::HashSet<_> = opened.iter().map(|o| o.id.clone()).collect();
        assert_eq!(ids.len(), 100);
        assert!(opened.iter().all(|o| o.id.len() == 43));
        let codes: std::collections::HashSet<_> = opened.iter().map(|o| o.code.clone()).collect();
        assert_eq!(codes.len(), 100);
    }

    #[test]
    fn a_code_is_found_however_it_is_typed() {
        let p = leaked(TTL);
        let o = p.open("Living room\u{7} TV", None, LAN).unwrap();
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
        let mut o = p.open("tv", None, LAN).unwrap();
        let taken = p.take(&o.code, ME).unwrap();
        assert_eq!(taken.device(), "tv");
        // Taken: no one else can settle it.
        assert!(p.take(&o.id, ME).is_err());
        let approved = Outcome::Approved {
            username: "alice".into(),
            api_key: "key".into(),
        };
        taken.settle(approved.clone()).unwrap();
        assert_eq!((&mut o.outcome).await.unwrap().outcome, approved);
    }

    #[test]
    fn approving_mints_a_key_for_the_approver() {
        let (conn, alice) = database();
        let p = leaked(TTL);
        let o = p.open("Living room TV", Some("0192-tv"), LAN).unwrap();
        let (id, code) = (o.id.clone(), o.code.clone());
        let device = o.device(true);
        let settled = p.settle(&conn, &code, alice, "alice", false).unwrap();
        assert_eq!(
            settled,
            PairInfo {
                device: "Living room TV".into(),
                device_id: Some("0192-tv".into()),
                from: LAN,
            }
        );
        let Outcome::Approved { username, api_key } = device.join().unwrap() else {
            panic!("not approved");
        };
        assert_eq!(username, "alice");
        let user = crate::api_keys::authenticate_api_key(&conn, &api_key)
            .unwrap()
            .unwrap();
        assert_eq!(user.id, alice);
        let keys = crate::api_keys::list_api_keys(&conn, Some(alice)).unwrap();
        assert_eq!(keys[0].name, "Living room TV");
        // Settled once only.
        assert_eq!(
            p.settle(&conn, &id, alice, "alice", false),
            Err(PairError::NotFound)
        );
    }

    #[test]
    fn a_key_the_connection_would_not_take_is_revoked() {
        let (conn, alice) = database();
        let p = leaked(TTL);
        let o = p.open("tv", None, LAN).unwrap();
        let id = o.id.clone();
        let device = o.device(false);
        // The approver is told, rather than shown a sign-in that never happened.
        assert_eq!(
            p.settle(&conn, &id, alice, "alice", false),
            Err(PairError::NotFound)
        );
        let Outcome::Approved { api_key, .. } = device.join().unwrap() else {
            panic!("not approved");
        };
        assert!(
            crate::api_keys::authenticate_api_key(&conn, &api_key)
                .unwrap()
                .is_none()
        );
        assert!(
            crate::api_keys::list_api_keys(&conn, Some(alice))
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn declining_mints_nothing() {
        let (conn, alice) = database();
        let p = leaked(TTL);
        let mut o = p.open("tv", None, LAN).unwrap();
        p.settle(&conn, &o.id, alice, "alice", true).unwrap();
        assert_eq!((&mut o.outcome).await.unwrap().outcome, Outcome::Declined);
        assert!(
            crate::api_keys::list_api_keys(&conn, Some(alice))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_key_for_a_gone_device_is_revoked() {
        let (conn, alice) = database();
        let p = leaked(TTL);
        let o = p.open("tv", None, LAN).unwrap();
        let id = o.id.clone();
        // The connection closes while an approval holds the pairing.
        let taken = p.take(&id, ME).unwrap();
        drop(o);
        p.entries.lock().insert(taken.id, taken.entry);
        assert_eq!(
            p.settle(&conn, &id, alice, "alice", false),
            Err(PairError::NotFound)
        );
        assert!(
            crate::api_keys::list_api_keys(&conn, Some(alice))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn approving_an_unknown_pairing_is_not_found() {
        let (conn, alice) = database();
        let p = leaked(TTL);
        assert_eq!(
            p.settle(&conn, "ABCD-EFGH", alice, "alice", false),
            Err(PairError::NotFound)
        );
    }

    #[test]
    fn a_gone_device_gives_the_outcome_back() {
        let p = leaked(TTL);
        let o = p.open("tv", None, LAN).unwrap();
        let taken = p.take(&o.id, ME).unwrap();
        drop(o);
        assert!(taken.settle(Outcome::Declined).is_err());
    }

    #[tokio::test]
    async fn a_lapsed_pairing_is_expired_and_unknown() {
        let p = leaked(Duration::ZERO);
        let mut o = p.open("tv", None, LAN).unwrap();
        assert!(p.info(&o.id, ME).is_err());
        assert!(p.take(&o.code, ME).is_err());
        assert_eq!((&mut o.outcome).await.unwrap().outcome, Outcome::Expired);
    }

    #[test]
    fn the_requesting_address_is_kept() {
        let p = leaked(TTL);
        let mapped: IpAddr = "::ffff:203.0.113.9".parse().unwrap();
        let o = p.open("tv", None, mapped).unwrap();
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
        let o = p.open("tv", None, LAN).unwrap();
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
        let o = p.open("tv", None, LAN).unwrap();
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

    #[test]
    fn a_closed_connection_removes_its_pairing() {
        let p = leaked(TTL);
        let o = p.open("tv", None, LAN).unwrap();
        let id = o.id.clone();
        drop(o);
        assert!(p.info(&id, ME).is_err());
    }

    #[test]
    fn pairings_are_capped() {
        let p = leaked(TTL);
        let held: Vec<_> = (0..MAX_PENDING)
            .map(|i| p.open("tv", None, host(i)).unwrap())
            .collect();
        assert_eq!(p.open("tv", None, LAN).err(), Some(OpenError::Full));
        drop(held);
        assert!(p.open("tv", None, LAN).is_ok());
    }

    #[test]
    fn one_address_holds_three_at_most() {
        let p = leaked(TTL);
        let held: Vec<_> = (0..3).map(|_| p.open("tv", None, LAN).unwrap()).collect();
        assert_eq!(p.open("tv", None, LAN).err(), Some(OpenError::Busy));
        // The same address written as IPv4-mapped IPv6 is the same address.
        let mapped: IpAddr = "::ffff:192.168.1.20".parse().unwrap();
        assert_eq!(p.open("tv", None, mapped).err(), Some(OpenError::Busy));
        assert!(p.open("tv", None, host(1)).is_ok());

        // IPv6 counts by /64: a client picks any address in its own.
        let v6: Vec<_> = ["2001:db8:1:2::1", "2001:db8:1:2::2", "2001:db8:1:2:ffff::9"]
            .into_iter()
            .map(|a| p.open("tv", None, a.parse().unwrap()).unwrap())
            .collect();
        assert_eq!(
            p.open("tv", None, "2001:db8:1:2:abcd::1".parse().unwrap())
                .err(),
            Some(OpenError::Busy)
        );
        assert!(
            p.open("tv", None, "2001:db8:1:3::1".parse().unwrap())
                .is_ok()
        );

        drop(held);
        assert!(p.open("tv", None, LAN).is_ok());
        drop(v6);
    }
}
