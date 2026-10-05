//! Forwarding plays to ListenBrainz.
//!
//! A server records every play an account reports. Plays bound for a service
//! wait in `scrobble_outbox` (see `db::queries::scrobbling`), so they survive a
//! restart or an outage. One thread, `koan-scrobble`, sends them: it sleeps
//! until [`wake`] says something was queued, sends everything waiting in
//! batches, and goes back to sleep. When the service cannot be reached it
//! waits out a growing delay before trying again, and only then.
//!
//! Now-playing notices are not queued. One that cannot be sent is dropped:
//! by the time the service answers again, the track has moved on.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};
use serde_json::{Value, json};

use crate::db::pool::Pool;
use crate::db::queries::scrobbling::{self as queries, LISTENBRAINZ, Listen, ScrobbleTarget};

const API: &str = "https://api.listenbrainz.org";

/// Listens per submission. ListenBrainz takes up to 1000; a smaller batch
/// keeps one rejected listen from holding back many good ones for long.
const BATCH: usize = 100;

const FIRST_RETRY: Duration = Duration::from_secs(30);
const LAST_RETRY: Duration = Duration::from_secs(60 * 60);

const CLIENT: &str = "kōan";
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Default)]
struct Pending {
    /// Something was queued since the last pass over the outbox.
    queued: bool,
    /// `(user, track)` now-playing notices to send.
    notices: Vec<(i64, i64)>,
}

static PENDING: Mutex<Pending> = Mutex::new(Pending {
    queued: false,
    notices: Vec::new(),
});
static RING: Condvar = Condvar::new();
static STARTED: OnceLock<()> = OnceLock::new();

/// Start the sender for the database at `db_path`, once per process. It
/// begins by sending whatever was left queued.
pub fn start(db_path: PathBuf) {
    STARTED.get_or_init(|| {
        PENDING.lock().queued = true;
        let spawned = std::thread::Builder::new()
            .name("koan-scrobble".into())
            .spawn(move || run(Pool::new(db_path)));
        if let Err(e) = spawned {
            log::warn!("scrobbling: could not start the sender: {e}");
        }
    });
}

/// Plays were queued. Costs nothing where no sender runs.
pub fn wake() {
    PENDING.lock().queued = true;
    RING.notify_one();
}

/// Tell `user`'s services that `track_id` started playing.
pub fn now_playing(user: i64, track_id: i64) {
    if STARTED.get().is_none() {
        return;
    }
    let mut pending = PENDING.lock();
    // Only the latest notice per account means anything.
    pending.notices.retain(|&(u, _)| u != user);
    pending.notices.push((user, track_id));
    drop(pending);
    RING.notify_one();
}

fn run(pool: Pool) {
    let http = reqwest::blocking::Client::builder()
        .user_agent(concat!(
            "koan/",
            env!("CARGO_PKG_VERSION"),
            " (https://github.com/radiosilence/koan)"
        ))
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap_or_else(|_| reqwest::blocking::Client::new());
    let mut retry = FIRST_RETRY;
    let mut not_before: Option<Instant> = None;
    loop {
        let (drain, notices) = {
            let mut pending = PENDING.lock();
            loop {
                let ready = pending.queued && not_before.is_none_or(|t| Instant::now() >= t);
                if ready || !pending.notices.is_empty() {
                    break;
                }
                match not_before {
                    Some(t) if pending.queued => {
                        RING.wait_until(&mut pending, t);
                    }
                    _ => RING.wait(&mut pending),
                }
            }
            let drain = pending.queued && not_before.is_none_or(|t| Instant::now() >= t);
            if drain {
                pending.queued = false;
            }
            (drain, std::mem::take(&mut pending.notices))
        };
        for (user, track) in notices {
            send_notice(&pool, &http, user, track);
        }
        if !drain {
            continue;
        }
        match send_queued(&pool, &mut |token, kind, listens| {
            submit(&http, token, kind, listens)
        }) {
            Ok(()) => {
                retry = FIRST_RETRY;
                not_before = None;
            }
            Err(Unsent { wait }) => {
                let wait = wait.unwrap_or(retry).max(Duration::from_secs(1));
                log::info!("scrobbling: trying again in {}s", wait.as_secs());
                retry = (retry * 2).min(LAST_RETRY);
                not_before = Some(Instant::now() + wait);
                PENDING.lock().queued = true;
            }
        }
    }
}

/// Nothing more can be sent for now. `wait` is how long the service asked
/// for, when it said.
struct Unsent {
    wait: Option<Duration>,
}

impl From<crate::db::connection::DbError> for Unsent {
    fn from(e: crate::db::connection::DbError) -> Self {
        log::warn!("scrobbling: {e}");
        Unsent { wait: None }
    }
}

impl From<rusqlite::Error> for Unsent {
    fn from(e: rusqlite::Error) -> Self {
        log::warn!("scrobbling: {e}");
        Unsent { wait: None }
    }
}

/// Send everything queued, account by account. An account whose service
/// cannot be reached is passed over for the rest of the pass, so it holds up
/// no other; the pass then fails, and the sender tries again after a wait.
fn send_queued(
    pool: &Pool,
    send: &mut impl FnMut(&str, ListenType, &[&Listen]) -> Sent,
) -> Result<(), Unsent> {
    let targets = queries::targets(&pool.get()?.conn)?;
    let mut unreachable: Option<Unsent> = None;
    for target in targets {
        if let Err(wait) = drain(pool, &target, send)? {
            let longest = unreachable.and_then(|u| u.wait).max(wait);
            unreachable = Some(Unsent { wait: longest });
        }
    }
    unreachable.map_or(Ok(()), Err)
}

type Sent = Result<(), Failure>;

/// Send one account's queue until it is empty, the credential is refused, or
/// the service cannot be reached, which is the inner `Err` with how long the
/// service asked to be left.
fn drain(
    pool: &Pool,
    target: &ScrobbleTarget,
    send: &mut impl FnMut(&str, ListenType, &[&Listen]) -> Sent,
) -> Result<Result<(), Option<Duration>>, Unsent> {
    if target.service != LISTENBRAINZ {
        refuse(pool, target, "kōan cannot send to this service.")?;
        return Ok(Ok(()));
    }
    loop {
        let batch = queries::queued(&pool.get()?.conn, target, BATCH)?;
        if batch.is_empty() {
            return Ok(Ok(()));
        }
        // A play of a track with no artist or title tells the service nothing.
        let (sendable, blank): (Vec<_>, Vec<_>) = batch
            .into_iter()
            .partition(|q| !q.listen.artist.trim().is_empty() && !q.listen.title.trim().is_empty());
        if !blank.is_empty() {
            let ids: Vec<i64> = blank.iter().map(|q| q.outbox_id).collect();
            queries::dequeue(&pool.get()?.conn, &ids)?;
        }
        if sendable.is_empty() {
            continue;
        }
        let listens: Vec<&Listen> = sendable.iter().map(|q| &q.listen).collect();
        let ids: Vec<i64> = sendable.iter().map(|q| q.outbox_id).collect();
        match send(&target.token, ListenType::of(listens.len()), &listens) {
            Ok(()) => queries::dequeue(&pool.get()?.conn, &ids)?,
            Err(Failure::Refused) => {
                refuse(
                    pool,
                    target,
                    "ListenBrainz refused the token. Connect again with a current one.",
                )?;
                return Ok(Ok(()));
            }
            // One listen it will not take fails the batch: send them singly
            // to find it, and drop only what it rejects.
            Err(Failure::Rejected(why)) => {
                for (listen, id) in listens.iter().zip(&ids) {
                    match send(&target.token, ListenType::Single, &[listen]) {
                        Ok(()) => {}
                        Err(Failure::Rejected(_)) => {
                            log::info!(
                                "scrobbling: ListenBrainz rejected {} – {}: {why}",
                                listen.artist,
                                listen.title
                            );
                        }
                        Err(Failure::Refused) => {
                            refuse(pool, target, "ListenBrainz refused the token.")?;
                            return Ok(Ok(()));
                        }
                        Err(Failure::Unreachable(wait)) => return Ok(Err(wait)),
                    }
                    queries::dequeue(&pool.get()?.conn, &[*id])?;
                }
            }
            Err(Failure::Unreachable(wait)) => return Ok(Err(wait)),
        }
    }
}

fn refuse(pool: &Pool, target: &ScrobbleTarget, why: &str) -> Result<(), Unsent> {
    log::info!(
        "scrobbling: stopped sending for user {}: {why}",
        target.user_id
    );
    queries::refuse(&pool.get()?.conn, target.user_id, &target.service, why)?;
    Ok(())
}

fn send_notice(pool: &Pool, http: &reqwest::blocking::Client, user: i64, track: i64) {
    let found = pool.get().ok().and_then(|db| {
        let token = queries::target(&db.conn, user, LISTENBRAINZ).ok()??;
        let listen = queries::now_playing(&db.conn, track).ok()??;
        Some((token, listen))
    });
    let Some((token, listen)) = found else { return };
    if listen.artist.trim().is_empty() || listen.title.trim().is_empty() {
        return;
    }
    if let Err(Failure::Unreachable(_) | Failure::Rejected(_)) =
        submit(http, &token, ListenType::PlayingNow, &[&listen])
    {
        log::debug!("scrobbling: now playing for user {user} not sent");
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ListenType {
    Single,
    Import,
    PlayingNow,
}

impl ListenType {
    fn of(count: usize) -> Self {
        if count == 1 {
            Self::Single
        } else {
            Self::Import
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Single => "single",
            Self::Import => "import",
            Self::PlayingNow => "playing_now",
        }
    }
}

#[derive(Debug)]
enum Failure {
    /// The token is not accepted.
    Refused,
    /// The listens are not accepted as they stand.
    Rejected(String),
    /// No answer worth acting on; try again later, after the wait if given.
    Unreachable(Option<Duration>),
}

fn submit(
    http: &reqwest::blocking::Client,
    token: &str,
    kind: ListenType,
    listens: &[&Listen],
) -> Result<(), Failure> {
    let body = json!({
        "listen_type": kind.name(),
        "payload": listens
            .iter()
            .map(|l| listen_json(l, kind != ListenType::PlayingNow))
            .collect::<Vec<_>>(),
    });
    let resp = http
        .post(format!("{API}/1/submit-listens"))
        .header("Authorization", format!("Token {token}"))
        .json(&body)
        .send()
        .map_err(|e| {
            log::info!("scrobbling: ListenBrainz unreachable: {}", e.without_url());
            Failure::Unreachable(None)
        })?;
    let status = resp.status().as_u16();
    let wait = resp
        .headers()
        .get("X-RateLimit-Reset-In")
        .and_then(|v| v.to_str().ok()?.parse().ok())
        .map(Duration::from_secs);
    outcome(status, wait, || resp.text().unwrap_or_default())
}

/// What a submission's status means. Any other client error is the request
/// as it stands, which sending it again will not change; only rate limiting
/// and the server's own errors are worth waiting out.
fn outcome(status: u16, wait: Option<Duration>, body: impl FnOnce() -> String) -> Sent {
    match status {
        200..=299 => Ok(()),
        401 => Err(Failure::Refused),
        429 => Err(Failure::Unreachable(wait)),
        400..=499 => Err(Failure::Rejected(format!("{status}: {}", body()))),
        _ => Err(Failure::Unreachable(None)),
    }
}

fn listen_json(listen: &Listen, with_time: bool) -> Value {
    let mut info = json!({
        "media_player": CLIENT,
        "submission_client": CLIENT,
        "submission_client_version": VERSION,
    });
    if let Some(d) = listen.duration_ms.filter(|&d| d > 0) {
        info["duration_ms"] = json!(d);
    }
    if let Some(n) = listen.track_number.filter(|&n| n > 0) {
        info["tracknumber"] = json!(n);
    }
    if let Some(id) = &listen.recording_mbid {
        info["recording_mbid"] = json!(id);
    }
    if let Some(id) = &listen.release_mbid {
        info["release_mbid"] = json!(id);
    }
    let mut meta = json!({
        "artist_name": listen.artist,
        "track_name": listen.title,
        "additional_info": info,
    });
    if let Some(album) = &listen.album {
        meta["release_name"] = json!(album);
    }
    let mut out = json!({ "track_metadata": meta });
    if with_time {
        out["listened_at"] = json!(listen.played_at);
    }
    out
}

#[derive(Debug, thiserror::Error)]
pub enum TokenError {
    #[error("ListenBrainz does not accept this token.")]
    Invalid,
    #[error("ListenBrainz could not be reached. Try again later.")]
    Unreachable,
}

/// Check a ListenBrainz user token, returning the account name it belongs to.
pub fn validate_listenbrainz_token(token: &str) -> Result<String, TokenError> {
    let http = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|_| TokenError::Unreachable)?;
    let resp = http
        .get(format!("{API}/1/validate-token"))
        .header("Authorization", format!("Token {token}"))
        .send()
        .map_err(|_| TokenError::Unreachable)?;
    if resp.status().as_u16() == 401 {
        return Err(TokenError::Invalid);
    }
    if !resp.status().is_success() {
        return Err(TokenError::Unreachable);
    }
    let body: Value = resp.json().map_err(|_| TokenError::Unreachable)?;
    match (body["valid"].as_bool(), body["user_name"].as_str()) {
        (Some(true), Some(name)) => Ok(name.to_owned()),
        _ => Err(TokenError::Invalid),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listen() -> Listen {
        Listen {
            played_at: 1_700_000_000,
            title: "Archangel".into(),
            artist: "Burial".into(),
            album: Some("Untrue".into()),
            duration_ms: Some(238_000),
            track_number: Some(2),
            recording_mbid: Some("rec".into()),
            release_mbid: None,
        }
    }

    #[test]
    fn a_listen_carries_what_is_known() {
        let v = listen_json(&listen(), true);
        assert_eq!(v["listened_at"], 1_700_000_000);
        let meta = &v["track_metadata"];
        assert_eq!(meta["artist_name"], "Burial");
        assert_eq!(meta["track_name"], "Archangel");
        assert_eq!(meta["release_name"], "Untrue");
        let info = &meta["additional_info"];
        assert_eq!(info["duration_ms"], 238_000);
        assert_eq!(info["tracknumber"], 2);
        assert_eq!(info["recording_mbid"], "rec");
        assert!(info.get("release_mbid").is_none());
    }

    #[test]
    fn now_playing_has_no_time() {
        let mut l = listen();
        l.album = None;
        let v = listen_json(&l, false);
        assert!(v.get("listened_at").is_none());
        assert!(v["track_metadata"].get("release_name").is_none());
    }

    #[test]
    fn client_errors_reject_the_batch_and_server_errors_wait() {
        let body = String::new;
        assert!(matches!(
            outcome(403, None, body),
            Err(Failure::Rejected(_))
        ));
        assert!(matches!(
            outcome(413, None, body),
            Err(Failure::Rejected(_))
        ));
        assert!(matches!(
            outcome(400, None, body),
            Err(Failure::Rejected(_))
        ));
        assert!(matches!(outcome(401, None, body), Err(Failure::Refused)));
        let wait = Some(Duration::from_secs(9));
        assert!(matches!(outcome(429, wait, body), Err(Failure::Unreachable(w)) if w == wait));
        assert!(matches!(
            outcome(503, None, body),
            Err(Failure::Unreachable(None))
        ));
        assert!(outcome(200, None, body).is_ok());
    }

    /// Two accounts with a play waiting each, the first queued first.
    fn two_accounts() -> (tempfile::TempDir, Pool) {
        use crate::db::queries::{SOURCE_SUBSONIC, record_plays_at, sample_meta, upsert_track};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("koan.db");
        let db = crate::db::connection::Database::open(&path).unwrap();
        db.conn
            .execute_batch(
                "INSERT INTO users (id, username, password_hash, role) VALUES
                     (1, 'stuck', 'x', 'user'), (2, 'fine', 'x', 'user');",
            )
            .unwrap();
        upsert_track(&db.conn, &sample_meta("Archangel", "Burial", "Untrue")).unwrap();
        let track: i64 = db
            .conn
            .query_row("SELECT id FROM tracks", [], |r| r.get(0))
            .unwrap();
        for (user, token) in [(1, "stuck"), (2, "fine")] {
            queries::connect(&db.conn, user, LISTENBRAINZ, token, token).unwrap();
            record_plays_at(&db.conn, user, &[(track, 100 + user)], SOURCE_SUBSONIC).unwrap();
        }
        (dir, Pool::new(path))
    }

    fn pending(pool: &Pool, user: i64) -> i64 {
        queries::services(&pool.get().unwrap().conn, user).unwrap()[0].pending
    }

    #[test]
    fn an_unreachable_account_holds_up_no_other() {
        let (_dir, pool) = two_accounts();
        let mut sent = Vec::new();
        let result = send_queued(&pool, &mut |token, _, listens| {
            if token == "stuck" {
                return Err(Failure::Unreachable(Some(Duration::from_secs(5))));
            }
            sent.push((token.to_owned(), listens.len()));
            Ok(())
        });
        let Err(Unsent { wait }) = result else {
            panic!("the pass should fail for the stuck account")
        };
        assert_eq!(wait, Some(Duration::from_secs(5)));
        assert_eq!(sent, vec![("fine".to_owned(), 1)]);
        assert_eq!((pending(&pool, 1), pending(&pool, 2)), (1, 0));
    }

    #[test]
    fn a_rejected_listen_is_dropped_and_the_queue_moves_on() {
        let (_dir, pool) = two_accounts();
        let result = send_queued(&pool, &mut |token, _, _| {
            if token == "stuck" {
                return Err(Failure::Rejected("403: forbidden".into()));
            }
            Ok(())
        });
        assert!(result.is_ok());
        assert_eq!((pending(&pool, 1), pending(&pool, 2)), (0, 0));
    }

    #[test]
    fn several_listens_are_an_import() {
        assert_eq!(ListenType::of(1), ListenType::Single);
        assert_eq!(ListenType::of(2).name(), "import");
    }
}
