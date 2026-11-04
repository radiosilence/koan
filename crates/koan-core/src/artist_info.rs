//! An artist's biography and photograph.
//!
//! Resolved by identity rather than by name: the artist's MusicBrainz id, the
//! Wikidata item MusicBrainz links it to, and from there the Wikipedia article
//! and the Commons image. A name search picks the wrong one of several bands
//! sharing a name, and a biography of the wrong band is worse than none.
//!
//! What was found is cached per artist, so a page draws from the database and
//! never waits on the network, and refreshed after a month. A miss is cached
//! for a week: the id may resolve, or the article appear, by then. Nothing
//! here holds a database connection across a network call.
//!
//! An artist synced from a koan server offering `koanArtistInfo` is asked of
//! the server, which resolves its whole library ahead of time; the device
//! resolves it itself only when the server cannot be reached. Everything else
//! is resolved here, ahead of time too: the walk (`start`) goes through the
//! library's artists that have a MusicBrainz id, newest first, at a pace
//! MusicBrainz and Wikimedia allow, and keeps each photograph on disk.

use std::path::PathBuf;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};
use thiserror::Error;

use crate::db::connection::DbError;
use crate::db::pool::Pool;
use crate::remote::client::{SubsonicClient, SubsonicError};
use crate::remote::musicbrainz::{self, MusicBrainzError};
use crate::remote::wikimedia::{self, WikimediaError};

const REFRESH_AFTER_SECS: i64 = 30 * 24 * 60 * 60;
/// How long a lookup that found nothing, or a server's answer, holds before
/// it is asked again.
const ASK_AGAIN_AFTER_SECS: i64 = 7 * 24 * 60 * 60;

/// Wide enough for a header at 2×, and one of Commons' standard thumbnail
/// widths: it refuses others.
const IMAGE_WIDTH: u32 = 960;

#[derive(Debug, Error)]
pub enum ArtistInfoError {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("database error: {0}")]
    Pool(#[from] DbError),
    #[error("musicbrainz: {0}")]
    MusicBrainz(#[from] MusicBrainzError),
    #[error("wikimedia: {0}")]
    Wikimedia(#[from] WikimediaError),
    #[error("server: {0}")]
    Server(#[from] SubsonicError),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ArtistInfo {
    pub bio: Option<String>,
    /// The article the biography is the opening of.
    pub bio_url: Option<String>,
    pub image_url: Option<String>,
    /// Photographer and licence, as the image's licence asks.
    pub image_credit: Option<String>,
    pub fetched_at: i64,
}

impl ArtistInfo {
    /// Whether the lookup found nothing to show.
    pub fn is_empty(&self) -> bool {
        self.bio.is_none() && self.image_url.is_none()
    }

    fn is_stale(&self, now: i64, served: bool) -> bool {
        let holds = if served || self.is_empty() {
            ASK_AGAIN_AFTER_SECS
        } else {
            REFRESH_AFTER_SECS
        };
        now - self.fetched_at > holds
    }
}

/// What the database holds, without touching the network.
pub fn cached(conn: &Connection, artist_id: i64) -> rusqlite::Result<Option<ArtistInfo>> {
    conn.query_row(
        "SELECT bio, bio_url, image_url, image_credit, fetched_at
           FROM artist_info WHERE artist_id = ?1",
        params![artist_id],
        |row| {
            Ok(ArtistInfo {
                bio: row.get(0)?,
                bio_url: row.get(1)?,
                image_url: row.get(2)?,
                image_credit: row.get(3)?,
                fetched_at: row.get(4)?,
            })
        },
    )
    .optional()
}

/// The cache while it is fresh, otherwise the server or the network, stored
/// for next time.
///
/// A network failure is not remembered as a miss — it says nothing about the
/// artist — so a stale answer is served instead, if there is one.
///
/// Takes the pool rather than a connection: the lookups take seconds, mostly
/// waiting on MusicBrainz's rate limit, and a connection held through them is
/// one every other reader goes without.
pub fn fetch(pool: &Pool, artist_id: i64) -> Result<Option<ArtistInfo>, ArtistInfoError> {
    fetch_since(pool, artist_id, false, Asker::Page)
}

/// `fetch` for a client of this server (`getArtistInfo2`): by MusicBrainz id
/// only, never a name search, since the answer goes out as resolved by id.
pub fn fetch_by_id(pool: &Pool, artist_id: i64) -> Result<Option<ArtistInfo>, ArtistInfoError> {
    fetch_since(pool, artist_id, false, Asker::Client)
}

/// Who asks, which decides how far a lookup goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Asker {
    /// A page: a name search as the last resort, and a stale answer rather
    /// than a failure.
    Page,
    /// A client of this server: by id only, and a stale answer rather than
    /// a failure.
    Client,
    /// The walk: by id only, and a failure is said, so the walk can tell the
    /// network is down.
    Walk,
}

/// `fetch`, looking the artist up again even while the cache is fresh when
/// `again` is set, as far as `asker` allows.
fn fetch_since(
    pool: &Pool,
    artist_id: i64,
    again: bool,
    asker: Asker,
) -> Result<Option<ArtistInfo>, ArtistInfoError> {
    let now = crate::clock::unix_secs();
    let (held, subject, served) = {
        let db = pool.get()?;
        (
            cached(&db.conn, artist_id)?,
            subject(&db.conn, artist_id)?,
            served(&db.conn, artist_id)?,
        )
    };
    // A miss found by id alone (the walk's, a client's) leaves a page its
    // name search while the artist's id is still unknown.
    let searchable = asker == Asker::Page
        && held.as_ref().is_some_and(ArtistInfo::is_empty)
        && subject.as_ref().is_some_and(|s| s.mbid.is_none());
    if let Some(held) = &held
        && !again
        && !searchable
        && !held.is_stale(now, served.is_some())
    {
        return Ok(Some(held.clone()));
    }

    if let Some((client, id)) = &served {
        match from_server(client, id, now) {
            Ok(info) => {
                store(&pool.get()?.conn, artist_id, &info)?;
                return Ok(Some(info));
            }
            // Resolved here instead, as a device on its own would.
            Err(e) => log::info!("artist info for {artist_id} from the server: {e}"),
        }
    }

    let Some(subject) = subject else {
        return Ok(held);
    };

    let found = look_up(&subject, now, asker == Asker::Page);
    if let Ok((_, Some(mbid))) = &found
        && subject.mbid.is_none()
    {
        pool.get()?.conn.execute(
            "UPDATE artists SET mbid = COALESCE(mbid, ?1) WHERE id = ?2",
            params![mbid, artist_id],
        )?;
    }
    match found {
        Ok((info, _)) => {
            store(&pool.get()?.conn, artist_id, &info)?;
            Ok(Some(info))
        }
        Err(e) => {
            log::warn!("artist info for {artist_id}: {e}");
            match held {
                Some(held) if asker != Asker::Walk => Ok(Some(held)),
                _ => Err(e),
            }
        }
    }
}

/// The server's answer for the artist it calls `id`, as this device stores it.
fn from_server(client: &SubsonicClient, id: &str, now: i64) -> Result<ArtistInfo, ArtistInfoError> {
    let answer = client.get_artist_info2(id)?;
    let bio = answer.biography.filter(|b| !b.trim().is_empty());
    Ok(ArtistInfo {
        bio_url: answer.koan_bio_url.filter(|_| bio.is_some()),
        bio,
        // Where the photograph came from, standing for it: it is fetched
        // from the server (`image`).
        image_url: answer
            .large_image_url
            .filter(|u| answer.koan_image && !u.is_empty()),
        image_credit: answer.koan_image_credit.filter(|_| answer.koan_image),
        fetched_at: now,
    })
}

/// The client and the server's id for an artist the signed-in server
/// resolves itself: one synced from a koan server offering `koanArtistInfo`.
fn served(
    conn: &Connection,
    artist_id: i64,
) -> rusqlite::Result<Option<(std::sync::Arc<SubsonicClient>, String)>> {
    let Some(client) = serving_client() else {
        return Ok(None);
    };
    let remote_id: Option<String> = conn
        .query_row(
            "SELECT remote_id FROM artists WHERE id = ?1",
            params![artist_id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    Ok(remote_id.filter(|id| !id.is_empty()).map(|id| (client, id)))
}

/// The signed-in server, where it offers `koanArtistInfo`.
fn serving_client() -> Option<std::sync::Arc<SubsonicClient>> {
    let cfg = crate::config::Config::cached();
    if !cfg.remote.enabled {
        return None;
    }
    let client = crate::helpers::subsonic_client(&cfg)?;
    crate::remote::profile::probed(client.auth())
        .is_some_and(|p| p.offers(crate::remote::profile::ARTIST_INFO))
        .then_some(client)
}

/// How long after a lookup a refused photo address is not looked up again.
const RELOOK_AFTER_REFUSAL_SECS: i64 = 24 * 60 * 60;

/// The photograph's bytes: from the signed-in server for an artist it
/// resolves, otherwise as kept on disk, otherwise downloaded and kept. No
/// connection is held while it downloads.
///
/// A stored address Commons refuses (it serves only its standard thumbnail
/// widths, and files are renamed) is looked up again, and the new one
/// downloaded; at most once a day, so an address the lookup still names is
/// not looked up on every request.
pub fn image(pool: &Pool, artist_id: i64) -> Result<Option<Vec<u8>>, ArtistInfoError> {
    let (held, served, uid) = {
        let db = pool.get()?;
        (
            cached(&db.conn, artist_id)?,
            served(&db.conn, artist_id)?,
            uid(&db.conn, artist_id)?,
        )
    };
    let address = held.as_ref().and_then(|info| info.image_url.clone());
    if let Some((client, id)) = &served {
        if let (Some(uid), Some(url)) = (&uid, &address)
            && let Some(kept) = kept(uid, url)
        {
            return Ok(Some(kept));
        }
        match client.koan_artist_image(id) {
            Ok(Some(bytes)) => {
                if let (Some(uid), Some(url)) = (&uid, &address) {
                    keep(uid, url, &bytes);
                }
                return Ok(Some(bytes));
            }
            // The server has none after all: the page gives its box back
            // until the server is next asked.
            Ok(None) => {
                pool.get()?.conn.execute(
                    "UPDATE artist_info SET image_url = NULL, image_credit = NULL
                      WHERE artist_id = ?1",
                    params![artist_id],
                )?;
                return Ok(None);
            }
            Err(e) => log::info!("artist {artist_id}'s photo from the server: {e}"),
        }
    }
    let looked_up = held.as_ref().map_or(0, |info| info.fetched_at);
    let Some(url) = held
        .and_then(|info| info.image_url)
        .filter(|u| !u.is_empty())
    else {
        return Ok(None);
    };
    if let Some(uid) = &uid
        && let Some(kept) = kept(uid, &url)
    {
        return Ok(Some(kept));
    }
    let http = wikimedia::client();
    // Looked up again at most once a day: a lookup that found the same
    // refused address, or none newer, is not repeated on every request.
    let recent = crate::clock::unix_secs() - looked_up < RELOOK_AFTER_REFUSAL_SECS;
    // A refused address with nothing to replace it is no photo: the page
    // gives its box back, and nobody asks again this session.
    let (url, bytes) = match wikimedia::download(&http, &url) {
        Err(e) if e.refused() && recent => {
            log::debug!("artist {artist_id}'s photo was refused ({e}); looked up within the day");
            return Ok(None);
        }
        Err(e) if e.refused() => {
            log::info!("artist {artist_id}'s photo was refused ({e}); looking it up again");
            let fresh = fetch_since(pool, artist_id, true, Asker::Page);
            // Marked as looked up whatever the lookup did (throttled, offline,
            // nothing found, which store nothing), so the daily bound holds.
            touch(pool, artist_id);
            match fresh?.and_then(|info| info.image_url) {
                Some(fresh) if fresh != url => {
                    let bytes = wikimedia::download(&http, &fresh)?;
                    (fresh, bytes)
                }
                Some(_) | None => return Ok(None),
            }
        }
        found => (url, found?),
    };
    if let Some(uid) = &uid {
        keep(uid, &url, &bytes);
    }
    Ok(Some(bytes))
}

/// Moves an artist's info's lookup time to now, where it has a row.
fn touch(pool: &Pool, artist_id: i64) {
    let now = crate::clock::unix_secs();
    if let Ok(db) = pool.get()
        && let Err(e) = db.conn.execute(
            "UPDATE artist_info SET fetched_at = ?1 WHERE artist_id = ?2",
            params![now, artist_id],
        )
    {
        log::warn!("artist {artist_id}'s info: {e}");
    }
}

fn uid(conn: &Connection, artist_id: i64) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT uid FROM artists WHERE id = ?1",
        params![artist_id],
        |row| row.get(0),
    )
    .optional()
    .map(Option::flatten)
}

// --- Photographs on disk ---------------------------------------------------

/// Where photographs are kept: one directory per artist, by uid, holding the
/// photograph at the address last looked up, named for that address.
fn photos() -> PathBuf {
    crate::places::cache().join("artists")
}

fn photo_path(uid: &str, url: &str) -> PathBuf {
    photos()
        .join(uid)
        .join(format!("{:x}", md5::compute(url.as_bytes())))
}

/// The photograph kept for the artist at `url`.
pub fn kept(uid: &str, url: &str) -> Option<Vec<u8>> {
    std::fs::read(photo_path(uid, url)).ok()
}

/// Keeps the photograph at `url` as the artist's only one.
fn keep(uid: &str, url: &str, bytes: &[u8]) {
    let path = photo_path(uid, url);
    let Some(dir) = path.parent() else { return };
    if let Ok(read) = std::fs::read_dir(dir) {
        for entry in read.flatten() {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    let partial = path.with_extension("part");
    let written = std::fs::create_dir_all(dir)
        .and_then(|()| std::fs::write(&partial, bytes))
        .and_then(|()| std::fs::rename(&partial, &path));
    if let Err(e) = written {
        log::warn!("keeping {uid}'s photo: {e}");
    }
}

/// Removes the photographs of artists no longer in the library.
fn sweep_photos(conn: &Connection) {
    let Ok(read) = std::fs::read_dir(photos()) else {
        return;
    };
    let mut gone = conn
        .prepare_cached("SELECT 1 FROM artists WHERE uid = ?1")
        .ok();
    for entry in read.flatten() {
        let name = entry.file_name();
        let Some(uid) = name.to_str() else { continue };
        let known = gone
            .as_mut()
            .is_none_or(|q| q.exists(params![uid]).unwrap_or(true));
        if !known {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

// --- The walk ----------------------------------------------------------------

/// How far the walk has got through the artists it found to look up, while
/// it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Walk {
    pub done: u64,
    pub total: u64,
}

static PROGRESS: Mutex<Option<Walk>> = Mutex::new(None);
static STARTED: AtomicBool = AtomicBool::new(false);
/// Rung when the library changes, so the walk looks for new artists, and when
/// the network stops being constrained.
static WAKE: LazyLock<crate::signal::Wake> = LazyLock::new(crate::signal::Wake::new);

/// Between one artist and the next: the lookups themselves keep to
/// MusicBrainz's one request a second, and this leaves the pace well inside
/// Wikimedia's guidance for a client that names itself.
const PACE: Duration = Duration::from_secs(1);
/// How long the library must rest before a walk starts: a scan writes in
/// bursts, and each would otherwise start a walk of its own.
const SETTLE: Duration = Duration::from_secs(10);
/// How often the walk looks again with nothing rung, for what went stale.
const INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
/// How long a walk that kept failing waits before trying again.
const AFTER_FAILURE: Duration = Duration::from_secs(60 * 60);
/// Failures in a row that end a walk: the network is down, or a service is
/// refusing.
const FAILURES: u32 = 3;

/// The walk's progress while it runs; `None` otherwise.
pub fn progress() -> Option<Walk> {
    *PROGRESS.lock()
}

/// Something the walk waits on changed: the library's artists, or whether
/// the network is constrained.
pub fn wake() {
    if STARTED.load(Ordering::Relaxed) {
        WAKE.bump();
    }
}

fn set_progress(progress: Option<Walk>) {
    let mut held = PROGRESS.lock();
    if *held != progress {
        *held = progress;
        drop(held);
        crate::signal::engine_changed().bump();
    }
}

/// Starts the walk's thread, once per process. It sleeps until the library
/// changes, or a day passes.
pub fn start() {
    if STARTED.swap(true, Ordering::Relaxed) {
        return;
    }
    std::thread::Builder::new()
        .name("koan-artist-info".into())
        .spawn(run)
        .ok();
}

/// Why a walk ended.
#[derive(Debug, PartialEq, Eq)]
enum Ended {
    /// Every artist found was looked up, or none needed it.
    Through,
    /// The app went quiet, or the network is constrained.
    NotNow,
    /// Lookups kept failing.
    Failed,
}

fn run() {
    loop {
        crate::quiet::wait_until_awake();
        let generation = WAKE.generation();
        let ended = walk(crate::db::pool::shared());
        match ended {
            // Not woken early by the library: the next walk would only fail
            // again.
            Ended::Failed => {
                std::thread::sleep(AFTER_FAILURE);
                continue;
            }
            // Quiet: back to waiting for the app to wake.
            Ended::NotNow if !crate::remote::covers::constrained() => continue,
            Ended::NotNow | Ended::Through => {}
        }
        let mut seen = WAKE.wait_until(generation, INTERVAL);
        // Rung: wait for the library to rest.
        while seen != generation {
            let rang = seen;
            seen = WAKE.wait_until(rang, SETTLE);
            if seen == rang {
                break;
            }
        }
    }
}

fn may_walk() -> bool {
    crate::quiet::awake() && !crate::remote::covers::constrained()
}

/// Looks up every artist due a lookup, newest first.
fn walk(pool: &Pool) -> Ended {
    if !may_walk() {
        return Ended::NotNow;
    }
    // Signed in to a server: its artists are its to resolve, unless it is
    // known not to. Until it is known, they wait.
    let skip_remote = {
        let cfg = crate::config::Config::cached();
        cfg.remote.enabled
            && crate::helpers::subsonic_client(&cfg).is_none_or(|client| {
                crate::remote::profile::probed(client.auth())
                    .is_none_or(|p| p.offers(crate::remote::profile::ARTIST_INFO))
            })
    };
    let due = match pool.get() {
        Ok(db) => match due(&db.conn, crate::clock::unix_secs(), skip_remote) {
            Ok(due) => due,
            Err(e) => {
                log::warn!("artist info: {e}");
                return Ended::Failed;
            }
        },
        Err(e) => {
            log::warn!("artist info: {e}");
            return Ended::Failed;
        }
    };
    if due.is_empty() {
        if let Ok(db) = pool.get() {
            sweep_photos(&db.conn);
        }
        return Ended::Through;
    }
    let total = due.len() as u64;
    let started = Instant::now();
    log::info!("artist info: {total} artists to look up");
    set_progress(Some(Walk { done: 0, total }));
    let mut failures = 0;
    let mut ended = Ended::Through;
    for (done, artist_id) in due.into_iter().enumerate() {
        if !may_walk() {
            ended = Ended::NotNow;
            break;
        }
        match look_up_ahead(pool, artist_id) {
            Ok(()) => failures = 0,
            Err(e) => {
                log::info!("artist info for {artist_id}: {e}");
                failures += 1;
                if failures >= FAILURES {
                    ended = Ended::Failed;
                    break;
                }
            }
        }
        set_progress(Some(Walk {
            done: done as u64 + 1,
            total,
        }));
        std::thread::sleep(PACE);
    }
    log::info!(
        "artist info: walked {total} artists in {:.0} s ({ended:?})",
        started.elapsed().as_secs_f64()
    );
    if ended == Ended::Through
        && let Ok(db) = pool.get()
    {
        sweep_photos(&db.conn);
    }
    set_progress(None);
    ended
}

/// One artist, as the walk looks them up: by id alone, the photograph kept,
/// and a miss stored even where the artist is not one to look up, so the next
/// walk passes them by.
fn look_up_ahead(pool: &Pool, artist_id: i64) -> Result<(), ArtistInfoError> {
    let info = fetch_since(pool, artist_id, false, Asker::Walk)?;
    let Some(info) = info else {
        store(
            &pool.get()?.conn,
            artist_id,
            &ArtistInfo {
                fetched_at: crate::clock::unix_secs(),
                ..Default::default()
            },
        )?;
        return Ok(());
    };
    let Some(url) = info.image_url.filter(|u| !u.is_empty()) else {
        return Ok(());
    };
    let Some(uid) = uid(&pool.get()?.conn, artist_id)? else {
        return Ok(());
    };
    if kept(&uid, &url).is_none() {
        match wikimedia::download(&wikimedia::client(), &url) {
            Ok(bytes) => keep(&uid, &url, &bytes),
            // Looked up again when a page asks for it (`image`).
            Err(e) if e.refused() => log::debug!("artist {artist_id}'s photo was refused: {e}"),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

/// The artists due a lookup, the most recently added first: those with a
/// MusicBrainz id of their own or on one of their records, never looked up,
/// or looked up long enough ago. `skip_remote` leaves out artists synced from
/// a server.
fn due(conn: &Connection, now: i64, skip_remote: bool) -> rusqlite::Result<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT a.id FROM artists a
           LEFT JOIN albums al ON al.artist_id = a.id
           LEFT JOIN artist_info i ON i.artist_id = a.id
          WHERE (?1 = 0 OR a.remote_id IS NULL)
            AND (i.artist_id IS NULL
                 OR i.fetched_at < CASE WHEN i.bio IS NULL AND i.image_url IS NULL
                                        THEN ?2 ELSE ?3 END)
          GROUP BY a.id
         HAVING COALESCE(a.mbid, '') <> '' OR MAX(al.mbid) IS NOT NULL
          ORDER BY MAX(al.added_at) DESC, a.id DESC",
    )?;
    stmt.query_map(
        params![
            skip_remote,
            now - ASK_AGAIN_AFTER_SECS,
            now - REFRESH_AFTER_SECS
        ],
        |row| row.get(0),
    )?
    .collect()
}

/// What a lookup needs from the database, read before it goes to the network.
struct Subject {
    name: String,
    mbid: Option<String>,
    /// One of the artist's releases, for finding their id through its credits.
    release: Option<String>,
}

/// `None` when the artist is not one this can look up at all.
fn subject(conn: &Connection, artist_id: i64) -> rusqlite::Result<Option<Subject>> {
    let Some((name, mbid)) = conn
        .query_row(
            "SELECT name, mbid FROM artists WHERE id = ?1",
            params![artist_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()?
        // An empty id is no id: resolve one rather than look "" up.
        .map(|(name, mbid)| (name, mbid.filter(|m| !m.is_empty())))
    else {
        return Ok(None);
    };
    if is_placeholder_name(&name) {
        return Ok(None);
    }
    let release = match mbid {
        Some(_) => None,
        None => conn
            .query_row(
                "SELECT mbid FROM albums WHERE artist_id = ?1 AND mbid IS NOT NULL LIMIT 1",
                params![artist_id],
                |row| row.get(0),
            )
            .optional()?,
    };
    Ok(Some(Subject {
        name,
        mbid,
        release,
    }))
}

/// The network half: what was found, with every field empty when nothing
/// was, and the MusicBrainz id it resolved.
fn look_up(
    subject: &Subject,
    now: i64,
    by_name: bool,
) -> Result<(ArtistInfo, Option<String>), ArtistInfoError> {
    let mb = musicbrainz::default_client();
    let mbid = match &subject.mbid {
        Some(mbid) => Some(mbid.clone()),
        None => resolve_mbid(&mb, subject, by_name)?,
    };

    let mut info = ArtistInfo {
        fetched_at: now,
        ..Default::default()
    };
    let Some(mbid) = mbid else {
        return Ok((info, None));
    };
    throttle();
    let Some(qid) = when_busy(|| musicbrainz::wikidata_id(&mb, &mbid))? else {
        return Ok((info, Some(mbid)));
    };

    let wm = wikimedia::client();
    let entity = wikimedia::entity(&wm, &qid)?;
    if let Some(article) = &entity.article
        && let Some(intro) = wikimedia::intro(&wm, article)?
    {
        info.bio = Some(intro.text);
        info.bio_url = Some(intro.url).filter(|url| !url.is_empty());
    }
    // A vector file is a logo or a diagram, never a photograph.
    if let Some(file) = entity
        .image
        .as_ref()
        .filter(|f| !f.to_lowercase().ends_with(".svg"))
        && let Some(image) = wikimedia::image(&wm, file, IMAGE_WIDTH)?
    {
        info.image_url = Some(image.url);
        info.image_credit = image.credit;
    }
    Ok((info, Some(mbid)))
}

/// Find the artist's MusicBrainz id without one on the row.
///
/// Through one of their releases first: an album synced from a server, or
/// scanned from tags that carry `MUSICBRAINZ_ALBUMID`, names its artists by id,
/// so there is nothing to guess. A name search is the last resort, made only
/// when `by_name` allows it, and only an exact, unique match is taken.
fn resolve_mbid(
    mb: &reqwest::blocking::Client,
    subject: &Subject,
    by_name: bool,
) -> Result<Option<String>, ArtistInfoError> {
    let name = &subject.name;
    if let Some(release) = &subject.release {
        throttle();
        let credits = when_busy(|| musicbrainz::release_artists(mb, release))?;
        if let Some(mbid) = credited(&credits, name) {
            return Ok(Some(mbid));
        }
    }
    if !by_name {
        return Ok(None);
    }

    throttle();
    let results = when_busy(|| musicbrainz::search_artist(mb, name, 5))?;
    let mut exact = results
        .into_iter()
        .filter(|r| r.score >= 95 && same_name(&r.name, name));
    Ok(match (exact.next(), exact.next()) {
        (Some(only), None) => Some(only.mbid),
        _ => None,
    })
}

/// The credit on a release that is this artist: by name, or the only one.
fn credited(credits: &[(String, String)], name: &str) -> Option<String> {
    let only = match credits {
        [only] => Some(only),
        _ => None,
    };
    credits
        .iter()
        .find(|(credit, _)| same_name(credit, name))
        .or(only)
        .map(|(_, mbid)| mbid.clone())
}

/// Names as the sources spell them. MusicBrainz writes typographic hyphens and
/// apostrophes where tags almost always carry the ASCII ones: "At the Drive‐In".
fn same_name(a: &str, b: &str) -> bool {
    fn fold(name: &str) -> String {
        name.chars()
            .map(|c| match c {
                '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
                '\u{2018}' | '\u{2019}' | '\u{02BC}' => '\'',
                '\u{201C}' | '\u{201D}' => '"',
                c => c,
            })
            .flat_map(char::to_lowercase)
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }
    fold(a) == fold(b)
}

fn is_placeholder_name(name: &str) -> bool {
    ["various artists", "unknown artist", "[unknown]"]
        .iter()
        .any(|p| name.eq_ignore_ascii_case(p))
}

fn store(conn: &Connection, artist_id: i64, info: &ArtistInfo) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO artist_info (artist_id, bio, bio_url, image_url, image_credit, fetched_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(artist_id) DO UPDATE SET
             bio = excluded.bio, bio_url = excluded.bio_url,
             image_url = excluded.image_url, image_credit = excluded.image_credit,
             fetched_at = excluded.fetched_at",
        params![
            artist_id,
            info.bio,
            info.bio_url,
            info.image_url,
            info.image_credit,
            info.fetched_at
        ],
    )?;
    Ok(())
}

/// One MusicBrainz request, asked again after 2 s and then 4 s while
/// MusicBrainz answers that it is busy (503), as it often is for seconds at a
/// time: a page opened then would otherwise show nothing until its next
/// visit. Only the request that was refused is repeated, not the look-up
/// around it.
fn when_busy<T>(request: impl Fn() -> Result<T, MusicBrainzError>) -> Result<T, MusicBrainzError> {
    let mut answer = request();
    for wait in [2, 4] {
        if !matches!(answer, Err(MusicBrainzError::RateLimited)) {
            break;
        }
        std::thread::sleep(Duration::from_secs(wait));
        throttle();
        answer = request();
    }
    answer
}

/// MusicBrainz allows one request a second per client, and paging through
/// artists quickly would otherwise exceed it.
fn throttle() {
    static LAST: LazyLock<parking_lot::Mutex<Option<Instant>>> =
        LazyLock::new(|| parking_lot::Mutex::new(None));
    let mut last = LAST.lock();
    if let Some(at) = *last {
        let wait = Duration::from_secs(1).saturating_sub(at.elapsed());
        if !wait.is_zero() {
            std::thread::sleep(wait);
        }
    }
    *last = Some(Instant::now());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::queries::get_or_create_artist;

    fn test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        conn
    }

    fn test_pool() -> (tempfile::TempDir, Pool) {
        let dir = tempfile::tempdir().unwrap();
        let pool = Pool::new(dir.path().join("koan.db"));
        (dir, pool)
    }

    #[test]
    fn a_fresh_answer_is_served_from_the_cache() {
        let (_dir, pool) = test_pool();
        let conn = pool.get().unwrap();
        let conn = &conn.conn;
        let id = get_or_create_artist(conn, "Glass Candy", None).unwrap();
        let info = ArtistInfo {
            bio: Some("An American electronic music duo.".into()),
            fetched_at: crate::clock::unix_secs(),
            ..Default::default()
        };
        store(conn, id, &info).unwrap();

        // No network in a test: a fresh row must be answered without one.
        assert_eq!(fetch(&pool, id).unwrap(), Some(info));
    }

    #[test]
    fn placeholder_artists_are_never_looked_up() {
        let (_dir, pool) = test_pool();
        let id = get_or_create_artist(&pool.get().unwrap().conn, "Various Artists", None).unwrap();
        assert_eq!(fetch(&pool, id).unwrap(), None);
    }

    #[test]
    fn the_credit_is_found_by_name_or_by_being_the_only_one() {
        let credits = vec![
            ("Ida No".to_string(), "a".to_string()),
            ("Johnny Jewel".to_string(), "b".to_string()),
        ];
        assert_eq!(credited(&credits, "johnny jewel").as_deref(), Some("b"));
        assert_eq!(credited(&credits, "Glass Candy"), None);
        let solo = vec![(
            "Glass Candy & The Shattered Theatre".to_string(),
            "c".to_string(),
        )];
        assert_eq!(credited(&solo, "Glass Candy").as_deref(), Some("c"));
    }

    #[test]
    fn names_match_across_typographic_punctuation() {
        assert!(same_name("At the Drive\u{2010}In", "At the Drive-In"));
        assert!(same_name("Can\u{2019}t Maintain", "can't  maintain"));
        assert!(!same_name("Azure Ray", "Ray Charles"));
    }

    #[test]
    fn staleness_is_a_month_for_a_find_and_a_week_otherwise() {
        let info = ArtistInfo {
            fetched_at: 0,
            ..Default::default()
        };
        assert!(info.is_empty());
        assert!(!info.is_stale(ASK_AGAIN_AFTER_SECS, false));
        assert!(info.is_stale(ASK_AGAIN_AFTER_SECS + 1, false));
        let found = ArtistInfo {
            bio: Some("A band.".into()),
            ..info
        };
        assert!(!found.is_stale(REFRESH_AFTER_SECS, false));
        assert!(found.is_stale(REFRESH_AFTER_SECS + 1, false));
        assert!(found.is_stale(ASK_AGAIN_AFTER_SECS + 1, true));
    }

    #[test]
    fn the_walk_takes_artists_with_ids_newest_first_and_passes_fresh_ones_by() {
        let conn = test_db();
        let artist = |name: &str, mbid: Option<&str>| {
            let id = get_or_create_artist(&conn, name, None).unwrap();
            conn.execute(
                "UPDATE artists SET mbid = ?1 WHERE id = ?2",
                params![mbid, id],
            )
            .unwrap();
            id
        };
        let album = |artist: i64, title: &str, mbid: Option<&str>, added: &str| {
            conn.execute(
                "INSERT INTO albums (title, artist_id, mbid, added_at) VALUES (?1, ?2, ?3, ?4)",
                params![title, artist, mbid, added],
            )
            .unwrap();
        };
        let old = artist("Glass Candy", Some("mb-old"));
        album(old, "B/E/A/T/B/O/X", None, "2020-01-01");
        let new = artist("Chromatics", None);
        album(new, "Night Drive", Some("rel-new"), "2024-01-01");
        let nameless = artist("Desire", None);
        album(nameless, "II", None, "2025-01-01");
        let empty = artist("Farah", Some(""));
        album(empty, "Farah", None, "2025-06-01");
        let fresh = artist("Symmetry", Some("mb-fresh"));
        album(fresh, "Themes", None, "2023-01-01");

        let now = crate::clock::unix_secs();
        store(
            &conn,
            fresh,
            &ArtistInfo {
                bio: Some("A side project.".into()),
                fetched_at: now,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(due(&conn, now, false).unwrap(), vec![new, old]);

        // A miss holds for a week, a find for a month.
        store(
            &conn,
            new,
            &ArtistInfo {
                fetched_at: now - ASK_AGAIN_AFTER_SECS - 1,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(due(&conn, now, false).unwrap(), vec![new, old]);
        assert_eq!(
            due(&conn, now + REFRESH_AFTER_SECS + 1, false).unwrap(),
            vec![new, fresh, old]
        );

        conn.execute(
            "UPDATE artists SET remote_id = 'r' WHERE id = ?1",
            params![old],
        )
        .unwrap();
        assert_eq!(due(&conn, now, true).unwrap(), vec![new]);
    }

    #[test]
    fn an_artist_leaving_the_library_takes_its_info() {
        let conn = test_db();
        let id = get_or_create_artist(&conn, "Glass Candy", None).unwrap();
        store(
            &conn,
            id,
            &ArtistInfo {
                fetched_at: crate::clock::unix_secs(),
                ..Default::default()
            },
        )
        .unwrap();
        conn.execute("DELETE FROM artists WHERE id = ?1", params![id])
            .unwrap();
        assert_eq!(cached(&conn, id).unwrap(), None);
    }
}
