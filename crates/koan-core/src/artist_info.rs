//! An artist's biography and photograph.
//!
//! Resolved by identity rather than by name: the artist's MusicBrainz id, the
//! Wikidata item MusicBrainz links it to, and from there the Wikipedia article
//! and the Commons image. A name search picks the wrong one of several bands
//! sharing a name, and a biography of the wrong band is worse than none.
//!
//! Cached per artist, misses included, so a page draws from the database and
//! never waits on the network, and an artist with no article is not asked
//! about again on every visit. What was found is refreshed after a month.

use std::sync::LazyLock;
use std::time::{Duration, Instant};

use rusqlite::{Connection, OptionalExtension, params};
use thiserror::Error;

use crate::remote::musicbrainz::{self, MusicBrainzError};
use crate::remote::wikimedia::{self, WikimediaError};

const REFRESH_AFTER_SECS: i64 = 30 * 24 * 60 * 60;

/// Wide enough for a header at 2×; Commons serves the nearest standard size.
const IMAGE_WIDTH: u32 = 800;

#[derive(Debug, Error)]
pub enum ArtistInfoError {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("musicbrainz: {0}")]
    MusicBrainz(#[from] MusicBrainzError),
    #[error("wikimedia: {0}")]
    Wikimedia(#[from] WikimediaError),
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
    fn is_stale(&self, now: i64) -> bool {
        now - self.fetched_at > REFRESH_AFTER_SECS
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

/// The cache while it is fresh, otherwise the network, stored for next time.
///
/// A network failure is not remembered as a miss — it says nothing about the
/// artist — so a stale answer is served instead, if there is one.
pub fn fetch(conn: &Connection, artist_id: i64) -> Result<Option<ArtistInfo>, ArtistInfoError> {
    let now = now();
    let held = cached(conn, artist_id)?;
    if let Some(held) = &held
        && !held.is_stale(now)
    {
        return Ok(Some(held.clone()));
    }

    match look_up(conn, artist_id, now) {
        Ok(Some(info)) => {
            store(conn, artist_id, &info)?;
            Ok(Some(info))
        }
        Ok(None) => Ok(held),
        Err(e) => {
            log::warn!("artist info for {artist_id}: {e}");
            match held {
                Some(held) => Ok(Some(held)),
                None => Err(e),
            }
        }
    }
}

/// The photograph's bytes. Network, every time: the front end caches images,
/// and the database is no place for them.
pub fn image(conn: &Connection, artist_id: i64) -> Result<Option<Vec<u8>>, ArtistInfoError> {
    let Some(url) = cached(conn, artist_id)?.and_then(|info| info.image_url) else {
        return Ok(None);
    };
    Ok(Some(wikimedia::download(&wikimedia::client(), &url)?))
}

/// `None` when the artist is not one this can look up at all; a found-nothing
/// answer is `Some` with every field empty, and is cached.
fn look_up(
    conn: &Connection,
    artist_id: i64,
    now: i64,
) -> Result<Option<ArtistInfo>, ArtistInfoError> {
    let Some((name, mbid)) = conn
        .query_row(
            "SELECT name, mbid FROM artists WHERE id = ?1",
            params![artist_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()?
    else {
        return Ok(None);
    };
    if is_placeholder_name(&name) {
        return Ok(None);
    }

    let mb = musicbrainz::default_client();
    let mbid = match mbid {
        Some(mbid) => Some(mbid),
        None => {
            let found = resolve_mbid(conn, &mb, artist_id, &name)?;
            if let Some(found) = &found {
                conn.execute(
                    "UPDATE artists SET mbid = COALESCE(mbid, ?1) WHERE id = ?2",
                    params![found, artist_id],
                )?;
            }
            found
        }
    };

    let mut info = ArtistInfo {
        fetched_at: now,
        ..Default::default()
    };
    let Some(mbid) = mbid else {
        return Ok(Some(info));
    };
    throttle();
    let Some(qid) = musicbrainz::wikidata_id(&mb, &mbid)? else {
        return Ok(Some(info));
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
    Ok(Some(info))
}

/// Find the artist's MusicBrainz id without one on the row.
///
/// Through one of their releases first: an album synced from a server, or
/// scanned from tags that carry `MUSICBRAINZ_ALBUMID`, names its artists by id,
/// so there is nothing to guess. A name search is the last resort, and only an
/// exact, unique match is taken.
fn resolve_mbid(
    conn: &Connection,
    mb: &reqwest::blocking::Client,
    artist_id: i64,
    name: &str,
) -> Result<Option<String>, ArtistInfoError> {
    let release: Option<String> = conn
        .query_row(
            "SELECT mbid FROM albums WHERE artist_id = ?1 AND mbid IS NOT NULL LIMIT 1",
            params![artist_id],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(release) = release {
        throttle();
        let credits = musicbrainz::release_artists(mb, &release)?;
        if let Some(mbid) = credited(&credits, name) {
            return Ok(Some(mbid));
        }
    }

    throttle();
    let results = musicbrainz::search_artist(mb, name, 5)?;
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

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
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

    #[test]
    fn a_fresh_answer_is_served_from_the_cache() {
        let conn = test_db();
        let id = get_or_create_artist(&conn, "Glass Candy", None).unwrap();
        let info = ArtistInfo {
            bio: Some("An American electronic music duo.".into()),
            fetched_at: now(),
            ..Default::default()
        };
        store(&conn, id, &info).unwrap();

        // No network in a test: a fresh row must be answered without one.
        assert_eq!(fetch(&conn, id).unwrap(), Some(info));
    }

    #[test]
    fn a_cached_miss_is_an_answer_too() {
        let conn = test_db();
        let id = get_or_create_artist(&conn, "Nobody In Particular", None).unwrap();
        let miss = ArtistInfo {
            fetched_at: now(),
            ..Default::default()
        };
        store(&conn, id, &miss).unwrap();
        assert_eq!(fetch(&conn, id).unwrap(), Some(miss));
    }

    #[test]
    fn placeholder_artists_are_never_looked_up() {
        let conn = test_db();
        let id = get_or_create_artist(&conn, "Various Artists", None).unwrap();
        assert_eq!(fetch(&conn, id).unwrap(), None);
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
    fn staleness_is_a_month() {
        let info = ArtistInfo {
            fetched_at: 0,
            ..Default::default()
        };
        assert!(!info.is_stale(REFRESH_AFTER_SECS));
        assert!(info.is_stale(REFRESH_AFTER_SECS + 1));
    }

    #[test]
    fn an_artist_leaving_the_library_takes_its_info() {
        let conn = test_db();
        let id = get_or_create_artist(&conn, "Glass Candy", None).unwrap();
        store(
            &conn,
            id,
            &ArtistInfo {
                fetched_at: now(),
                ..Default::default()
            },
        )
        .unwrap();
        conn.execute("DELETE FROM artists WHERE id = ?1", params![id])
            .unwrap();
        assert_eq!(cached(&conn, id).unwrap(), None);
    }
}
