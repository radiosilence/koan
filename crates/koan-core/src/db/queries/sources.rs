//! Where a track's data comes from, and which sources are the same track.
//!
//! Each file on disk is a row of `local_files` and each entry on the server a
//! row of `remote_entries`. A source row holds that source's tags as it gave
//! them, and nothing else writes to it. A track holds at most one source of
//! each kind, and its own columns — names, audio properties, path, server id —
//! are derived from its sources by fixed precedence: the file's, then the
//! server's. Nothing outside this module writes those columns, so neither
//! source can overwrite the other's view of the track.
//!
//! [`link`] alone decides which track a source belongs to. It runs when a
//! source is added, when its tags change and when its partner goes, and matches
//! on one normalised key: MusicBrainz recording + release, or album, album
//! artist, disc, number and title, with the track artist as a tie-break. A
//! match it cannot make unambiguously it declines.

use std::collections::HashSet;

use rusqlite::{Connection, OptionalExtension, params};
use unicode_normalization::UnicodeNormalization;

use crate::db::connection::DbError;

use super::TrackMeta;
use super::albums::get_or_create_album;
use super::artists::get_or_create_artist;

/// The two kinds of source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Local,
    Remote,
}

impl Kind {
    fn table(self) -> &'static str {
        match self {
            Self::Local => "local_files",
            Self::Remote => "remote_entries",
        }
    }

    /// The column naming one source of this kind.
    fn key(self) -> &'static str {
        match self {
            Self::Local => "path",
            Self::Remote => "remote_id",
        }
    }

    fn other(self) -> Self {
        match self {
            Self::Local => Self::Remote,
            Self::Remote => Self::Local,
        }
    }
}

/// The DDL for both source tables. Both carry every `TrackMeta` column, so one
/// reader and one writer serve each; the columns a kind has no use for stay
/// NULL.
pub(crate) const SOURCE_TABLES: &str = "
    CREATE TABLE IF NOT EXISTS local_files (
        path             TEXT PRIMARY KEY,
        track_id         INTEGER NOT NULL UNIQUE REFERENCES tracks(id) ON DELETE CASCADE,
        slot_key         TEXT NOT NULL,
        artist_key       TEXT NOT NULL,
        title            TEXT NOT NULL,
        artist           TEXT NOT NULL,
        album_artist     TEXT,
        album            TEXT NOT NULL,
        date             TEXT,
        disc             INTEGER,
        track_number     INTEGER,
        genre            TEXT,
        label            TEXT,
        duration_ms      INTEGER,
        codec            TEXT,
        sample_rate      INTEGER,
        bit_depth        INTEGER,
        channels         INTEGER,
        bitrate          INTEGER,
        size_bytes       INTEGER,
        mtime            INTEGER,
        remote_id        TEXT,
        remote_url       TEXT,
        album_remote_id  TEXT,
        artist_remote_id TEXT,
        mbid             TEXT,
        album_mbid       TEXT,
        album_added_at   TEXT
    );
    CREATE TABLE IF NOT EXISTS remote_entries (
        remote_id        TEXT PRIMARY KEY,
        track_id         INTEGER NOT NULL UNIQUE REFERENCES tracks(id) ON DELETE CASCADE,
        slot_key         TEXT NOT NULL,
        artist_key       TEXT NOT NULL,
        title            TEXT NOT NULL,
        artist           TEXT NOT NULL,
        album_artist     TEXT,
        album            TEXT NOT NULL,
        date             TEXT,
        disc             INTEGER,
        track_number     INTEGER,
        genre            TEXT,
        label            TEXT,
        duration_ms      INTEGER,
        codec            TEXT,
        sample_rate      INTEGER,
        bit_depth        INTEGER,
        channels         INTEGER,
        bitrate          INTEGER,
        size_bytes       INTEGER,
        mtime            INTEGER,
        path             TEXT,
        remote_url       TEXT,
        album_remote_id  TEXT,
        artist_remote_id TEXT,
        mbid             TEXT,
        album_mbid       TEXT,
        album_added_at   TEXT
    );
    CREATE INDEX IF NOT EXISTS idx_local_files_slot ON local_files(slot_key);
    CREATE INDEX IF NOT EXISTS idx_local_files_mbid ON local_files(mbid, album_mbid)
        WHERE mbid IS NOT NULL;
    CREATE INDEX IF NOT EXISTS idx_remote_entries_slot ON remote_entries(slot_key);
    CREATE INDEX IF NOT EXISTS idx_remote_entries_mbid ON remote_entries(mbid, album_mbid)
        WHERE mbid IS NOT NULL;
    CREATE INDEX IF NOT EXISTS idx_remote_entries_album ON remote_entries(album_remote_id);
";

const COLUMNS: &str = "title, artist, album_artist, album, date, disc, track_number, genre,
    label, duration_ms, codec, sample_rate, bit_depth, channels, bitrate, size_bytes, mtime,
    path, remote_id, remote_url, album_remote_id, artist_remote_id, mbid, album_mbid,
    album_added_at";

/// A source row as stored, in `COLUMNS` order from `at`.
fn read_meta(row: &rusqlite::Row, at: usize, kind: Kind) -> rusqlite::Result<TrackMeta> {
    Ok(TrackMeta {
        title: row.get(at)?,
        artist: row.get(at + 1)?,
        album_artist: row.get(at + 2)?,
        album: row.get(at + 3)?,
        date: row.get(at + 4)?,
        disc: row.get(at + 5)?,
        track_number: row.get(at + 6)?,
        genre: row.get(at + 7)?,
        label: row.get(at + 8)?,
        duration_ms: row.get(at + 9)?,
        codec: row.get(at + 10)?,
        sample_rate: row.get(at + 11)?,
        bit_depth: row.get(at + 12)?,
        channels: row.get(at + 13)?,
        bitrate: row.get(at + 14)?,
        size_bytes: row.get(at + 15)?,
        mtime: row.get(at + 16)?,
        path: row.get(at + 17)?,
        remote_id: row.get(at + 18)?,
        remote_url: row.get(at + 19)?,
        album_remote_id: row.get(at + 20)?,
        artist_remote_id: row.get(at + 21)?,
        mbid: row.get(at + 22)?,
        album_mbid: row.get(at + 23)?,
        album_added_at: row.get(at + 24)?,
        source: match kind {
            Kind::Local => "local",
            Kind::Remote => "remote",
        }
        .into(),
    })
}

/// A name as matching sees it: NFC, lowercased, whitespace collapsed. Paths
/// arrive decomposed and tags precomposed, and sources disagree about case.
pub(crate) fn fold(s: &str) -> String {
    let lower = s.nfc().collect::<String>().to_lowercase();
    lower.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Disc 0 is no disc. Taggers write it for a single-disc release and servers
/// leave the field out, and a zero on one side only splits every track in two.
fn disc_of(meta: &TrackMeta) -> Option<i32> {
    meta.disc.filter(|d| *d > 0)
}

fn album_artist_of(meta: &TrackMeta) -> &str {
    meta.album_artist.as_deref().unwrap_or(&meta.artist)
}

/// One position on one release: album, album artist, disc, number and title.
fn slot_key(meta: &TrackMeta) -> String {
    let num = |n: Option<i32>| n.map(|n| n.to_string()).unwrap_or_default();
    format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
        fold(&meta.album),
        fold(album_artist_of(meta)),
        num(disc_of(meta)),
        num(meta.track_number),
        fold(&meta.title)
    )
}

fn nonempty(id: &Option<String>) -> Option<&str> {
    id.as_deref().filter(|id| !id.is_empty())
}

/// A source that might be the same track as another.
struct Candidate {
    track_id: i64,
    same_slot: bool,
    same_artist: bool,
    same_recording: bool,
    /// Whether the number and disc agree, for telling apart two placings of
    /// one recording on a release.
    same_place: bool,
}

/// Which candidate is the same track, if exactly one is.
///
/// The slot with the same artist first; then the slot whatever the artist,
/// which needs a track number — without one every untitled slot on a release
/// collapses to the same key, and the artist was what kept two apart; then
/// the MusicBrainz recording on the release, however either source names it,
/// with position as the tie-break, since a release can carry one recording
/// twice. Two candidates in the first tier that applies is ambiguity, and the
/// match is declined rather than guessed.
fn choose(meta: &TrackMeta, candidates: &[Candidate]) -> Option<i64> {
    let numbered = meta.track_number.is_some();
    let tiers: [&dyn Fn(&Candidate) -> bool; 2] = [&|c| c.same_slot && c.same_artist, &|c| {
        c.same_slot && numbered
    }];
    for tier in tiers {
        let hits: Vec<i64> = candidates
            .iter()
            .filter(|c| tier(c))
            .map(|c| c.track_id)
            .collect();
        match hits.as_slice() {
            [] => {}
            [only] => return Some(*only),
            _ => break,
        }
    }
    let recordings: Vec<&Candidate> = candidates.iter().filter(|c| c.same_recording).collect();
    match recordings.as_slice() {
        [only] => Some(only.track_id),
        _ => {
            let placed: Vec<i64> = recordings
                .iter()
                .filter(|c| c.same_place)
                .map(|c| c.track_id)
                .collect();
            match placed.as_slice() {
                [only] => Some(*only),
                _ => None,
            }
        }
    }
}

/// Where to look for the track a source is.
struct Search<'a> {
    /// The kind of source to look among.
    among: Kind,
    /// Only sources whose track holds no source of this kind other than the
    /// one named: a track holds one source of each kind.
    free_of: Option<(Kind, &'a str)>,
    /// A track to leave out.
    not_track: Option<i64>,
    /// Only remote entries the server listed in this sync (`live_ids`).
    live_only: bool,
}

fn candidates(
    conn: &Connection,
    meta: &TrackMeta,
    search: &Search,
) -> Result<Vec<Candidate>, DbError> {
    let free = match search.free_of {
        Some((kind, _)) => format!(
            "AND NOT EXISTS (SELECT 1 FROM {t} o WHERE o.track_id = c.track_id AND o.{k} IS NOT ?6)",
            t = kind.table(),
            k = kind.key()
        ),
        None => String::new(),
    };
    let live = if search.live_only {
        "AND c.remote_id IN (SELECT id FROM temp.live_ids WHERE kind = 'track')"
    } else {
        ""
    };
    let sql = format!(
        "SELECT c.track_id, c.slot_key = ?1, c.artist_key = ?2,
                ?3 IS NOT NULL AND ?4 IS NOT NULL AND c.mbid IS ?3 AND c.album_mbid IS ?4,
                c.track_number IS ?7
                  AND (c.disc IS NULL OR c.disc <= 0 OR ?8 IS NULL OR c.disc = ?8)
           FROM {table} c
          WHERE (c.slot_key = ?1 OR (?3 IS NOT NULL AND ?4 IS NOT NULL
                                     AND c.mbid = ?3 AND c.album_mbid = ?4))
            AND c.track_id IS NOT ?5 {free} {live}",
        table = search.among.table()
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map(
        params![
            slot_key(meta),
            fold(&meta.artist),
            nonempty(&meta.mbid),
            nonempty(&meta.album_mbid),
            search.not_track,
            search.free_of.map(|(_, key)| key),
            meta.track_number,
            disc_of(meta),
        ],
        |r| {
            Ok(Candidate {
                track_id: r.get(0)?,
                same_slot: r.get(1)?,
                same_artist: r.get(2)?,
                same_recording: r.get(3)?,
                same_place: r.get(4)?,
            })
        },
    )?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The track of the other kind of source this one is, if there is exactly one
/// free to take it.
fn counterpart(
    conn: &Connection,
    kind: Kind,
    key: &str,
    meta: &TrackMeta,
    not_track: Option<i64>,
) -> Result<Option<i64>, DbError> {
    let found = candidates(
        conn,
        meta,
        &Search {
            among: kind.other(),
            free_of: Some((kind, key)),
            not_track,
            live_only: false,
        },
    )?;
    Ok(choose(meta, &found))
}

fn load(conn: &Connection, kind: Kind, key: &str) -> Result<Option<(i64, TrackMeta)>, DbError> {
    Ok(conn
        .prepare_cached(&format!(
            "SELECT track_id, {COLUMNS} FROM {} WHERE {} = ?1",
            kind.table(),
            kind.key()
        ))?
        .query_row(params![key], |r| Ok((r.get(0)?, read_meta(r, 1, kind)?)))
        .optional()?)
}

fn on_track(
    conn: &Connection,
    kind: Kind,
    track: i64,
) -> Result<Option<(String, TrackMeta)>, DbError> {
    Ok(conn
        .prepare_cached(&format!(
            "SELECT {}, {COLUMNS} FROM {} WHERE track_id = ?1",
            kind.key(),
            kind.table()
        ))?
        .query_row(params![track], |r| Ok((r.get(0)?, read_meta(r, 1, kind)?)))
        .optional()?)
}

fn write(conn: &Connection, kind: Kind, track: i64, meta: &TrackMeta) -> Result<(), DbError> {
    conn.prepare_cached(&format!(
        "INSERT INTO {} (track_id, slot_key, artist_key, {COLUMNS})
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
                 ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28)
         ON CONFLICT({}) DO UPDATE SET
             track_id = excluded.track_id, slot_key = excluded.slot_key,
             artist_key = excluded.artist_key, title = excluded.title,
             artist = excluded.artist, album_artist = excluded.album_artist,
             album = excluded.album, date = excluded.date, disc = excluded.disc,
             track_number = excluded.track_number, genre = excluded.genre,
             label = excluded.label, duration_ms = excluded.duration_ms,
             codec = excluded.codec, sample_rate = excluded.sample_rate,
             bit_depth = excluded.bit_depth, channels = excluded.channels,
             bitrate = excluded.bitrate, size_bytes = excluded.size_bytes,
             mtime = excluded.mtime, path = excluded.path, remote_id = excluded.remote_id,
             remote_url = excluded.remote_url, album_remote_id = excluded.album_remote_id,
             artist_remote_id = excluded.artist_remote_id, mbid = excluded.mbid,
             album_mbid = excluded.album_mbid, album_added_at = excluded.album_added_at",
        kind.table(),
        kind.key()
    ))?
    .execute(params![
        track,
        slot_key(meta),
        fold(&meta.artist),
        meta.title,
        meta.artist,
        meta.album_artist,
        meta.album,
        meta.date,
        meta.disc,
        meta.track_number,
        meta.genre,
        meta.label,
        meta.duration_ms,
        meta.codec,
        meta.sample_rate,
        meta.bit_depth,
        meta.channels,
        meta.bitrate,
        meta.size_bytes,
        meta.mtime,
        meta.path,
        meta.remote_id,
        meta.remote_url,
        meta.album_remote_id,
        meta.artist_remote_id,
        meta.mbid,
        meta.album_mbid,
        meta.album_added_at,
    ])?;
    Ok(())
}

/// A track row with nothing derived on it yet; [`derive`] fills it in.
fn new_track(conn: &Connection, meta: &TrackMeta) -> Result<i64, DbError> {
    let uid = super::free_uid(conn, super::UidKind::Track, meta.remote_id.as_deref())?;
    conn.prepare_cached("INSERT INTO tracks (title, uid) VALUES (?1, ?2)")?
        .execute(params![meta.title, uid])?;
    Ok(conn.last_insert_rowid())
}

/// A track row from before source rows existed, still carrying this source's
/// path or server id and holding no source of its kind: the row the source
/// belongs to.
fn legacy_row(conn: &Connection, kind: Kind, key: &str) -> Result<Option<i64>, DbError> {
    Ok(conn
        .prepare_cached(&format!(
            "SELECT t.id FROM tracks t WHERE t.{k} = ?1
               AND NOT EXISTS (SELECT 1 FROM {table} s WHERE s.track_id = t.id)
             ORDER BY t.id LIMIT 1",
            k = kind.key(),
            table = kind.table()
        ))?
        .query_row(params![key], |r| r.get(0))
        .optional()?)
}

/// What a track's sources say it is, before it is written.
#[derive(Debug, PartialEq)]
struct Derived {
    album_id: Option<i64>,
    artist_id: Option<i64>,
    disc: Option<i32>,
    track_number: Option<i32>,
    title: String,
    duration_ms: Option<i64>,
    codec: Option<String>,
    sample_rate: Option<i32>,
    bit_depth: Option<i32>,
    channels: Option<i32>,
    bitrate: Option<i32>,
    size_bytes: Option<i64>,
    mtime: Option<i64>,
    genre: Option<String>,
    source: String,
    path: Option<String>,
    remote_id: Option<String>,
    remote_url: Option<String>,
    mbid: Option<String>,
}

fn stored(conn: &Connection, track: i64) -> Result<Option<Derived>, DbError> {
    Ok(conn
        .prepare_cached(
            "SELECT album_id, artist_id, disc, track_number, title, duration_ms, codec,
                    sample_rate, bit_depth, channels, bitrate, size_bytes, mtime, genre,
                    source, path, remote_id, remote_url, mbid
               FROM tracks WHERE id = ?1",
        )?
        .query_row(params![track], |r| {
            Ok(Derived {
                album_id: r.get(0)?,
                artist_id: r.get(1)?,
                disc: r.get(2)?,
                track_number: r.get(3)?,
                title: r.get(4)?,
                duration_ms: r.get(5)?,
                codec: r.get(6)?,
                sample_rate: r.get(7)?,
                bit_depth: r.get(8)?,
                channels: r.get(9)?,
                bitrate: r.get(10)?,
                size_bytes: r.get(11)?,
                mtime: r.get(12)?,
                genre: r.get(13)?,
                source: r.get(14)?,
                path: r.get(15)?,
                remote_id: r.get(16)?,
                remote_url: r.get(17)?,
                mbid: r.get(18)?,
            })
        })
        .optional()?)
}

/// Write a track's columns from its sources. A track left with none is
/// deleted, and the downloaded copy it held, if any, is returned for the
/// caller to remove once the change is committed.
///
/// What the track is called — album, artist, disc, number and title — comes
/// whole from one source, the file while there is one: mixing one source's
/// title with the other's album names a track neither has. Genre, the
/// MusicBrainz ids, the date and the label fall back to the server's where the
/// file has none, and the audio properties and position likewise. The album is
/// the one its names and release name, or for a track only the server has,
/// the one holding the server's id for its record.
fn derive(conn: &Connection, track: i64) -> Result<Option<String>, DbError> {
    let local = on_track(conn, Kind::Local, track)?.map(|(_, m)| m);
    let remote = on_track(conn, Kind::Remote, track)?.map(|(_, m)| m);
    let Some(before) = stored(conn, track)? else {
        return Ok(None);
    };
    let Some(primary) = local.as_ref().or(remote.as_ref()) else {
        return delete_track(conn, track);
    };
    let fallback = local.as_ref().and(remote.as_ref());
    let pick = |f: fn(&TrackMeta) -> Option<String>| {
        f(primary)
            .filter(|v| !v.is_empty())
            .or_else(|| fallback.and_then(f).filter(|v| !v.is_empty()))
    };
    let pick_num = |f: fn(&TrackMeta) -> Option<i32>| f(primary).or_else(|| fallback.and_then(f));

    let album_artist = album_artist_of(primary);
    let album_artist_id = get_or_create_artist(conn, album_artist, None)?;
    let artist_id = if primary.artist == album_artist {
        album_artist_id
    } else {
        get_or_create_artist(conn, &primary.artist, None)?
    };
    let date = pick(|m| m.date.clone());
    let label = pick(|m| m.label.clone());
    let codec = pick(|m| m.codec.clone());
    let added_at = [local.as_ref(), remote.as_ref()]
        .into_iter()
        .flatten()
        .filter_map(|m| m.album_added_at.as_deref())
        .min();
    // A track only the server has belongs to the album holding the server's
    // id for its record, which may be named as the files name it. Otherwise
    // the album is its names and release.
    let by_server_id: Option<(i64, i64)> = match (
        &local,
        remote.as_ref().and_then(|r| r.album_remote_id.as_deref()),
    ) {
        (None, Some(rid)) => conn
            .prepare_cached(
                "SELECT al.id, al.artist_id FROM albums al WHERE al.remote_id = ?1
                  ORDER BY EXISTS (SELECT 1 FROM tracks t JOIN local_files f ON f.track_id = t.id
                                    WHERE t.album_id = al.id) DESC, al.id
                  LIMIT 1",
            )?
            .query_row(params![rid], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?
            .and_then(|(album, artist): (i64, Option<i64>)| Some((album, artist?))),
        _ => None,
    };
    let (album_id, album_artist_id) = match by_server_id {
        Some(found) => found,
        None => (
            get_or_create_album(
                conn,
                &primary.album,
                album_artist_id,
                date.as_deref(),
                None,
                None,
                codec.as_deref(),
                label.as_deref(),
                pick(|m| m.album_mbid.clone()).as_deref(),
                added_at,
            )?,
            album_artist_id,
        ),
    };

    let genre = pick(|m| m.genre.clone());
    let after = Derived {
        album_id: Some(album_id),
        artist_id: Some(artist_id),
        // Position fills from the server where the file has none: a file
        // tagged without a number is still the track at that number.
        disc: disc_of(primary).or_else(|| fallback.and_then(disc_of)),
        track_number: primary
            .track_number
            .or_else(|| fallback.and_then(|m| m.track_number)),
        title: primary.title.clone(),
        duration_ms: primary
            .duration_ms
            .or_else(|| fallback.and_then(|m| m.duration_ms)),
        codec,
        sample_rate: pick_num(|m| m.sample_rate),
        bit_depth: pick_num(|m| m.bit_depth),
        channels: pick_num(|m| m.channels),
        bitrate: pick_num(|m| m.bitrate),
        size_bytes: local.as_ref().and_then(|m| m.size_bytes),
        mtime: local.as_ref().and_then(|m| m.mtime),
        genre: genre.clone(),
        source: if local.is_some() { "local" } else { "remote" }.into(),
        path: local.as_ref().and_then(|m| m.path.clone()),
        remote_id: remote.as_ref().and_then(|m| m.remote_id.clone()),
        remote_url: remote.as_ref().and_then(|m| m.remote_url.clone()),
        mbid: pick(|m| m.mbid.clone()),
    };

    // A sync passes every track through here, and almost none of them have
    // changed. Writing the row regardless rewrites it and all its index
    // entries; comparing first leaves the WAL alone.
    if after != before {
        conn.prepare_cached(
            "UPDATE tracks SET album_id=?1, artist_id=?2, disc=?3, track_number=?4,
             title=?5, duration_ms=?6, codec=?7, sample_rate=?8, bit_depth=?9,
             channels=?10, bitrate=?11, size_bytes=?12, mtime=?13, genre=?14,
             source=?15, path=?16, remote_id=?17, remote_url=?18, mbid=?19
             WHERE id=?20",
        )?
        .execute(params![
            after.album_id,
            after.artist_id,
            after.disc,
            after.track_number,
            after.title,
            after.duration_ms,
            after.codec,
            after.sample_rate,
            after.bit_depth,
            after.channels,
            after.bitrate,
            after.size_bytes,
            after.mtime,
            after.genre,
            after.source,
            after.path,
            after.remote_id,
            after.remote_url,
            after.mbid,
            track
        ])?;
    }

    // The server's uid when the server is koan, so the track is one id on
    // every device; otherwise the one this row has.
    if let Some(rid) = &after.remote_id {
        super::adopt_uid(conn, super::UidKind::Track, track, rid)?;
    }
    derive_record(conn, album_id, album_artist_id, remote.as_ref())?;

    let artist_text = if primary.artist == album_artist {
        primary.artist.clone()
    } else {
        format!("{} {}", primary.artist, album_artist)
    };
    index_for_search(
        conn,
        track,
        &after.title,
        &artist_text,
        &primary.album,
        genre.as_deref(),
    )?;

    super::tracks::prune_if_empty(
        conn,
        before.album_id.filter(|a| Some(*a) != after.album_id),
        before.artist_id.filter(|a| Some(*a) != after.artist_id),
    )?;
    Ok(None)
}

/// The server's id for an album and for its artist: the one most of the
/// album's server entries give, so a server and a file that name a record
/// differently still meet on one album, and nothing trades the id back and
/// forth. The koan server's uids come with them, so the album and artist are
/// one id on every device.
///
/// `remote` is what the track being derived says; nothing is counted unless
/// it disagrees with what is stored, which on a sync is almost never.
fn derive_record(
    conn: &Connection,
    album: i64,
    artist: i64,
    remote: Option<&TrackMeta>,
) -> Result<(), DbError> {
    let Some(remote) = remote else {
        return Ok(());
    };
    let (album_rid, artist_rid): (Option<String>, Option<String>) = conn
        .prepare_cached(
            "SELECT al.remote_id, ar.remote_id FROM albums al, artists ar
              WHERE al.id = ?1 AND ar.id = ?2",
        )?
        .query_row(params![album, artist], |r| Ok((r.get(0)?, r.get(1)?)))?;

    if nonempty(&remote.album_remote_id).is_some() && remote.album_remote_id != album_rid {
        let by_server: Option<String> = conn
            .prepare_cached(
                "SELECT r.album_remote_id FROM remote_entries r JOIN tracks t ON t.id = r.track_id
                  WHERE t.album_id = ?1 AND r.album_remote_id IS NOT NULL
                  GROUP BY 1 ORDER BY COUNT(*) DESC, 1 LIMIT 1",
            )?
            .query_row(params![album], |r| r.get(0))
            .optional()?;
        if let Some(rid) = by_server.filter(|rid| Some(rid) != album_rid.as_ref()) {
            settle_album_id(conn, album, &rid)?;
        }
    }

    if nonempty(&remote.artist_remote_id).is_some() && remote.artist_remote_id != artist_rid {
        let by_server: Option<String> = conn
            .prepare_cached(
                "SELECT r.artist_remote_id FROM remote_entries r
                   JOIN tracks t ON t.id = r.track_id JOIN albums al ON al.id = t.album_id
                  WHERE al.artist_id = ?1 AND r.artist_remote_id IS NOT NULL
                  GROUP BY 1 ORDER BY COUNT(*) DESC, 1 LIMIT 1",
            )?
            .query_row(params![artist], |r| r.get(0))
            .optional()?;
        if let Some(rid) = by_server.filter(|rid| Some(rid) != artist_rid.as_ref()) {
            conn.prepare_cached("UPDATE artists SET remote_id = ?1 WHERE id = ?2")?
                .execute(params![rid, artist])?;
            super::adopt_uid(conn, super::UidKind::Artist, artist, &rid)?;
        }
    }
    Ok(())
}

fn album_has_files(conn: &Connection, album: i64) -> Result<bool, DbError> {
    Ok(conn
        .prepare_cached(
            "SELECT EXISTS (SELECT 1 FROM tracks t JOIN local_files f ON f.track_id = t.id
                             WHERE t.album_id = ?1)",
        )?
        .query_row(params![album], |r| r.get(0))?)
}

/// Give `album` the server's id `rid`. Another album already holding it is
/// the server's grouping of the same record: one with no files is folded into
/// whichever has them, which is how a record the server names one way and
/// the files another becomes one album. Two albums of files claiming one
/// server album keep it where it is.
fn settle_album_id(conn: &Connection, album: i64, rid: &str) -> Result<(), DbError> {
    let holders: Vec<i64> = conn
        .prepare_cached("SELECT id FROM albums WHERE remote_id = ?1 AND id != ?2")?
        .query_map(params![rid, album], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mine = album_has_files(conn, album)?;
    for other in holders {
        if !album_has_files(conn, other)? {
            super::merge_albums(conn, album, other)?;
        } else if !mine {
            super::merge_albums(conn, other, album)?;
            return Ok(());
        } else {
            return Ok(());
        }
    }
    conn.prepare_cached("UPDATE albums SET remote_id = ?1 WHERE id = ?2")?
        .execute(params![rid, album])?;
    super::adopt_uid(conn, super::UidKind::Album, album, rid)?;
    Ok(())
}

/// Put a track's text in the search index, unless it is there already.
///
/// An FTS5 delete and insert is a write to every term's posting list and
/// eventually a segment merge, so an unchanged track is read and left alone.
fn index_for_search(
    conn: &Connection,
    id: i64,
    title: &str,
    artist: &str,
    album: &str,
    genre: Option<&str>,
) -> Result<(), DbError> {
    // `None` when the track is not indexed, else whether it is indexed as is.
    let current: Option<bool> = conn
        .prepare_cached(
            "SELECT title IS ?2 AND artist_name IS ?3 AND album_title IS ?4 AND genre IS ?5
               FROM tracks_fts WHERE rowid = ?1",
        )?
        .query_row(params![id, title, artist, album, genre], |r| r.get(0))
        .optional()?;
    if current == Some(true) {
        return Ok(());
    }
    if current.is_some() {
        conn.prepare_cached("DELETE FROM tracks_fts WHERE rowid = ?1")?
            .execute(params![id])?;
    }
    conn.prepare_cached(
        "INSERT INTO tracks_fts (rowid, title, artist_name, album_title, genre)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?
    .execute(params![id, title, artist, album, genre])?;
    Ok(())
}

/// Delete a track and everything that only means something with it, then the
/// album and artist it leaves empty. Returns the downloaded copy, if there was one, for the caller to remove
/// once the change is committed.
pub(crate) fn delete_track(conn: &Connection, track: i64) -> Result<Option<String>, DbError> {
    let Some((album, artist, cached)): Option<(Option<i64>, Option<i64>, Option<String>)> = conn
        .prepare_cached("SELECT album_id, artist_id, cached_path FROM tracks WHERE id = ?1")?
        .query_row(params![track], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()?
    else {
        return Ok(None);
    };
    for table in [
        "tracks_fts WHERE rowid",
        "lyrics_cache WHERE track_id",
        "play_history WHERE track_id",
        "favourites WHERE track_id",
        "scan_cache WHERE track_id",
        "playlist_tracks WHERE track_id",
        "share_tracks WHERE track_id",
        "local_files WHERE track_id",
        "remote_entries WHERE track_id",
        "tracks WHERE id",
    ] {
        conn.prepare_cached(&format!("DELETE FROM {table} = ?1"))?
            .execute(params![track])?;
    }
    super::tracks::prune_if_empty(conn, album, artist)?;
    Ok(cached)
}

/// Fold `loser` into `winner`: its sources and everything pointing at it move
/// across, and it is deleted. The two are one track seen from two sources.
///
/// Play history concatenates: every one of those plays was a play of this
/// track. Lyrics are one per track, so the winner keeps what it has and
/// inherits only what it is missing; so does the download.
fn merge(conn: &Connection, winner: i64, loser: i64) -> Result<(), DbError> {
    let (album, artist): (Option<i64>, Option<i64>) = conn
        .prepare_cached("SELECT album_id, artist_id FROM tracks WHERE id = ?1")?
        .query_row(params![loser], |r| Ok((r.get(0)?, r.get(1)?)))?;

    fold_rows(conn, winner, loser)?;
    log::info!("track {loser} is track {winner} from another source; merged them");

    derive(conn, winner)?;
    super::tracks::prune_if_empty(conn, album, artist)?;
    Ok(())
}

/// Move everything pointing at `loser` to `winner`, then delete it: sources,
/// favourites, ratings, bookmarks, history, scan cache, organize log, playlist,
/// share and saved play queue entries, lyrics where the winner has none, and
/// the download where the winner has none.
pub(crate) fn fold_rows(conn: &Connection, winner: i64, loser: i64) -> rusqlite::Result<()> {
    for table in ["favourites", "track_ratings", "bookmarks"] {
        conn.prepare_cached(&format!(
            "UPDATE OR IGNORE {table} SET track_id = ?1 WHERE track_id = ?2"
        ))?
        .execute(params![winner, loser])?;
    }
    for table in [
        "local_files",
        "remote_entries",
        "play_history",
        "scan_cache",
        "organize_log",
        "playlist_tracks",
        "share_tracks",
        "play_queue_entries",
    ] {
        conn.prepare_cached(&format!(
            "UPDATE {table} SET track_id = ?1 WHERE track_id = ?2"
        ))?
        .execute(params![winner, loser])?;
    }
    conn.prepare_cached(
        "UPDATE lyrics_cache SET track_id = ?1 WHERE track_id = ?2
           AND NOT EXISTS (SELECT 1 FROM lyrics_cache WHERE track_id = ?1)",
    )?
    .execute(params![winner, loser])?;
    conn.prepare_cached(
        "UPDATE tracks SET
             cached_path = COALESCE(cached_path, (SELECT cached_path FROM tracks WHERE id = ?2)),
             cache_size_bytes = COALESCE(cache_size_bytes,
                 (SELECT cache_size_bytes FROM tracks WHERE id = ?2)),
             cache_download_date = COALESCE(cache_download_date,
                 (SELECT cache_download_date FROM tracks WHERE id = ?2)),
             cache_pinned = (SELECT cache_pinned FROM tracks WHERE id = ?2)
           WHERE id = ?1 AND cached_path IS NULL",
    )?
    .execute(params![winner, loser])?;
    for sql in [
        "DELETE FROM lyrics_cache WHERE track_id = ?1",
        "DELETE FROM favourites WHERE track_id = ?1",
        "DELETE FROM track_ratings WHERE track_id = ?1",
        "DELETE FROM bookmarks WHERE track_id = ?1",
        "DELETE FROM tracks_fts WHERE rowid = ?1",
        "DELETE FROM tracks WHERE id = ?1",
    ] {
        conn.prepare_cached(sql)?.execute(params![loser])?;
    }
    Ok(())
}

/// Merge two tracks into the older one, which keeps its id, uid and history
/// where clients know them. Returns the survivor.
fn merge_into_older(conn: &Connection, a: i64, b: i64) -> Result<i64, DbError> {
    let (winner, loser) = if a < b { (a, b) } else { (b, a) };
    merge(conn, winner, loser)?;
    Ok(winner)
}

/// Decide, again, which track the source `key` belongs to, and write the
/// tracks that changes. Returns its track.
///
/// A source sharing its track with a partner stays while the two still match.
/// One that no longer matches leaves: for the counterpart it matches now, or
/// for a track of its own. A source on its own joins the counterpart it
/// matches, if exactly one is free.
pub(crate) fn link(conn: &Connection, kind: Kind, key: &str) -> Result<i64, DbError> {
    let (track, meta) = load(conn, kind, key)?
        .ok_or_else(|| DbError::from(rusqlite::Error::QueryReturnedNoRows))?;
    let partner = on_track(conn, kind.other(), track)?;

    if let Some((partner_key, _)) = &partner {
        let still = candidates(
            conn,
            &meta,
            &Search {
                among: kind.other(),
                free_of: None,
                not_track: None,
                live_only: false,
            },
        )?
        .into_iter()
        .filter(|c| c.track_id == track)
        .any(|c| {
            (c.same_slot && (c.same_artist || meta.track_number.is_some())) || c.same_recording
        });
        if still {
            derive(conn, track)?;
            return Ok(track);
        }

        let to = match counterpart(conn, kind, key, &meta, Some(track))? {
            Some(other) => other,
            None => new_track(conn, &meta)?,
        };
        log::info!(
            "{} {key} no longer matches {partner_key} on track {track}; moved to track {to}",
            kind.table()
        );
        conn.prepare_cached(&format!(
            "UPDATE {} SET track_id = ?1 WHERE {} = ?2",
            kind.table(),
            kind.key()
        ))?
        .execute(params![to, key])?;
        derive(conn, track)?;
        derive(conn, to)?;
        return Ok(to);
    }

    match counterpart(conn, kind, key, &meta, Some(track))? {
        Some(other) => merge_into_older(conn, track, other),
        None => {
            derive(conn, track)?;
            Ok(track)
        }
    }
}

/// What a `TrackMeta` says as a source of `kind`: a file knows nothing of the
/// server's ids, and an entry nothing of a path or the file's size and mtime.
fn as_kind(meta: &TrackMeta, kind: Kind) -> TrackMeta {
    let mut meta = meta.clone();
    match kind {
        Kind::Local => {
            meta.source = "local".into();
            meta.remote_id = None;
            meta.remote_url = None;
            meta.album_remote_id = None;
            meta.artist_remote_id = None;
        }
        Kind::Remote => {
            meta.source = "remote".into();
            meta.path = None;
            meta.size_bytes = None;
            meta.mtime = None;
        }
    }
    meta
}

/// Give a file gone from `old` back its track, if it is one of `arrived`:
/// tracks a scan has just made for files at new paths. The same file moved
/// is the same MusicBrainz recording on the same release when both name
/// one, or else the same slot and the same size or length, or failing that
/// the same size, length and modification time, which a rename keeps and an
/// untagged file's title (its name) does not. Only when exactly one arrival
/// fits; several is ambiguity, and declined. Whether it was given back.
///
/// The old track survives, with its id, uid, history, favourites, ratings,
/// playlist places and server partner; the arrival's file becomes its source
/// and the arrival is folded into it.
pub(crate) fn adopt_moved(conn: &Connection, old: &str, arrived: &[i64]) -> Result<bool, DbError> {
    if arrived.is_empty() {
        return Ok(false);
    }
    let Some((track, meta)) = load(conn, Kind::Local, old)? else {
        return Ok(false);
    };
    let found: Vec<(String, i64)> = conn
        .prepare_cached(
            "SELECT f.path, f.track_id FROM local_files f
              WHERE f.track_id IN (SELECT value FROM json_each(?1)) AND f.track_id != ?2
                AND NOT EXISTS (SELECT 1 FROM remote_entries r WHERE r.track_id = f.track_id)
                AND ((?3 IS NOT NULL AND ?4 IS NOT NULL AND f.mbid = ?3 AND f.album_mbid IS ?4)
                  OR ((?3 IS NULL OR f.mbid IS NULL) AND f.slot_key = ?5
                      AND (f.size_bytes = ?6 OR f.duration_ms = ?7))
                  OR (f.size_bytes = ?6 AND f.duration_ms = ?7 AND f.mtime = ?8))",
        )?
        .query_map(
            params![
                super::json_list(arrived),
                track,
                nonempty(&meta.mbid),
                nonempty(&meta.album_mbid),
                slot_key(&meta),
                meta.size_bytes,
                meta.duration_ms,
                meta.mtime,
            ],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?
        .collect::<rusqlite::Result<_>>()?;
    let [(path, arrival)] = found.as_slice() else {
        return Ok(false);
    };
    conn.prepare_cached("DELETE FROM local_files WHERE path = ?1")?
        .execute(params![old])?;
    conn.prepare_cached("DELETE FROM scan_cache WHERE path = ?1")?
        .execute(params![old])?;
    merge(conn, track, *arrival)?;
    log::info!("{old} moved to {path}; it keeps track {track}");
    Ok(true)
}

/// Record what a source says, linking and deriving as needed. Returns the
/// track and whether this source made a new one.
///
/// `seen` is the server ids a sync has listed so far. An entry the sync has
/// not listed, in the same slot, is taken to be this one under the id it had
/// before: a server that renumbers its library keeps its tracks, history and
/// favourites rather than gaining a second copy of every one. An id already
/// seen belongs to an entry of its own, so two entries the server lists with
/// identical tags stay two.
pub(crate) fn record(
    conn: &Connection,
    kind: Kind,
    meta: &TrackMeta,
    seen: Option<&HashSet<String>>,
) -> Result<(i64, bool), DbError> {
    let key = match kind {
        Kind::Local => meta.path.as_deref(),
        Kind::Remote => meta.remote_id.as_deref(),
    }
    .ok_or(DbError::NoSource)?;
    let meta = as_kind(meta, kind);

    let mut stored = load(conn, kind, key)?;
    if stored.is_none()
        && kind == Kind::Remote
        && let Some(seen) = seen
    {
        let renumbered: Option<String> = conn
            .prepare_cached(
                "SELECT remote_id FROM remote_entries WHERE slot_key = ?1 AND remote_id != ?2
                 ORDER BY rowid",
            )?
            .query_map(params![slot_key(&meta), key], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .find(|old| !seen.contains(old));
        if let Some(old) = renumbered {
            conn.prepare_cached("UPDATE remote_entries SET remote_id = ?1 WHERE remote_id = ?2")?
                .execute(params![key, old])?;
            stored = load(conn, kind, key)?;
        }
    }

    if let Some((track, old)) = stored {
        if old == meta {
            return Ok((track, false));
        }
        write(conn, kind, track, &meta)?;
        let relink = slot_key(&old) != slot_key(&meta)
            || fold(&old.artist) != fold(&meta.artist)
            || (&old.mbid, &old.album_mbid) != (&meta.mbid, &meta.album_mbid);
        let track = if relink {
            link(conn, kind, key)?
        } else {
            derive(conn, track)?;
            track
        };
        return Ok((track, false));
    }

    // A row from before source rows existed takes its source back, and is
    // then linked like any other whose tags may have changed.
    if let Some(track) = legacy_row(conn, kind, key)? {
        write(conn, kind, track, &meta)?;
        return Ok((link(conn, kind, key)?, false));
    }
    let (track, inserted) = match counterpart(conn, kind, key, &meta, None)? {
        Some(other) => (other, false),
        None => (new_track(conn, &meta)?, true),
    };
    write(conn, kind, track, &meta)?;
    derive(conn, track)?;
    Ok((track, inserted))
}

/// Drop a source. Its track keeps whatever other source it has, and that one
/// may now pair with a counterpart it could not take before; a track left
/// with none is deleted. Returns the downloaded copy a deleted track held.
pub(crate) fn remove(conn: &Connection, kind: Kind, key: &str) -> Result<Option<String>, DbError> {
    let Some((track, _)) = load(conn, kind, key)? else {
        return Ok(None);
    };
    conn.prepare_cached(&format!(
        "DELETE FROM {} WHERE {} = ?1",
        kind.table(),
        kind.key()
    ))?
    .execute(params![key])?;
    match on_track(conn, kind.other(), track)? {
        Some((partner, _)) => {
            derive(conn, track)?;
            link(conn, kind.other(), &partner)?;
            Ok(None)
        }
        None => delete_track(conn, track),
    }
}

/// Forget a track a rebuilt index has not re-read, whose source of `kind` —
/// the path or server id it still carries — is gone. Its other source, if one
/// has been read again, keeps it; otherwise it is deleted, and its download
/// returned for the caller to remove.
pub(crate) fn forget_unread(
    conn: &Connection,
    kind: Kind,
    key: &str,
) -> Result<Option<String>, DbError> {
    let Some(track) = legacy_row(conn, kind, key)? else {
        return Ok(None);
    };
    match on_track(conn, kind.other(), track)? {
        Some(_) => derive(conn, track),
        None => delete_track(conn, track),
    }
}

/// Drop the remote entries the server no longer lists, as [`remove`] does,
/// except that an entry whose track has no file hands the track to its
/// successor — the live entry in the same slot — rather than deleting it, so
/// a server that renumbered keeps the history and favourites under the new
/// id. Expects `temp.live_ids` populated with the live track ids.
pub(crate) fn remove_vanished(
    conn: &Connection,
    dead: &[String],
    live_tracks: bool,
) -> Result<Vec<String>, DbError> {
    let mut downloads = Vec::new();
    for key in dead {
        let Some((track, meta)) = load(conn, Kind::Remote, key)? else {
            continue;
        };
        conn.prepare_cached("DELETE FROM remote_entries WHERE remote_id = ?1")?
            .execute(params![key])?;
        if let Some((file, _)) = on_track(conn, Kind::Local, track)? {
            derive(conn, track)?;
            link(conn, Kind::Local, &file)?;
            continue;
        }
        let successor = if live_tracks {
            let found = candidates(
                conn,
                &meta,
                &Search {
                    among: Kind::Remote,
                    free_of: None,
                    not_track: Some(track),
                    live_only: true,
                },
            )?;
            choose(&meta, &found)
        } else {
            None
        };
        match successor {
            Some(other) => {
                merge_into_older(conn, track, other)?;
            }
            None => downloads.extend(delete_track(conn, track)?),
        }
    }
    Ok(downloads)
}

/// Build the source rows for a library indexed before they existed, from what
/// its tracks hold, then pair what the old matching left apart. The names a
/// track holds are what one of its sources said; a rescan and a full sync, which
/// the migration arranges, replace them with each source's own.
pub(crate) fn build_from_tracks(conn: &Connection) -> Result<(), DbError> {
    type Row = (i64, TrackMeta);
    let rows: Vec<Row> = {
        let mut stmt = conn.prepare(
            "SELECT t.id, t.title, COALESCE(a.name, ''), aa.name, COALESCE(al.title, ''),
                    al.date, t.disc, t.track_number, t.genre, al.label, t.duration_ms,
                    t.codec, t.sample_rate, t.bit_depth, t.channels, t.bitrate,
                    t.size_bytes, t.mtime, t.path, t.remote_id, t.remote_url,
                    al.remote_id, aa.remote_id, t.mbid, al.mbid, al.added_at
               FROM tracks t
               LEFT JOIN artists a ON a.id = t.artist_id
               LEFT JOIN albums al ON al.id = t.album_id
               LEFT JOIN artists aa ON aa.id = al.artist_id
              ORDER BY t.id",
        )?;
        stmt.query_map([], |r| Ok((r.get(0)?, read_meta(r, 1, Kind::Local)?)))?
            .collect::<rusqlite::Result<_>>()?
    };

    let mut strays = Vec::new();
    for (track, meta) in &rows {
        if meta.path.is_some() {
            write(conn, Kind::Local, *track, &as_kind(meta, Kind::Local))?;
        }
        if let Some(rid) = &meta.remote_id {
            match load(conn, Kind::Remote, rid)? {
                // A server id two rows carried: the later row is the same
                // entry, kept apart by the old matching.
                Some((holder, _)) => strays.push((holder, *track)),
                None => write(conn, Kind::Remote, *track, &as_kind(meta, Kind::Remote))?,
            }
        }
    }
    for (holder, stray) in strays {
        if on_track(conn, Kind::Local, stray)?.is_none() {
            merge_into_older(conn, holder, stray)?;
        }
    }

    // What the old matching left apart. Each lone source tries once.
    let lone: Vec<(Kind, String)> = {
        let mut stmt = conn.prepare(
            "SELECT 'l', path FROM local_files f
              WHERE NOT EXISTS (SELECT 1 FROM remote_entries r WHERE r.track_id = f.track_id)
             UNION ALL
             SELECT 'r', remote_id FROM remote_entries r
              WHERE NOT EXISTS (SELECT 1 FROM local_files f WHERE f.track_id = r.track_id)",
        )?;
        stmt.query_map([], |r| {
            let kind = if r.get::<_, String>(0)? == "l" {
                Kind::Local
            } else {
                Kind::Remote
            };
            Ok((kind, r.get(1)?))
        })?
        .collect::<rusqlite::Result<_>>()?
    };
    for (kind, key) in lone {
        if let Some((track, meta)) = load(conn, kind, &key)?
            && on_track(conn, kind.other(), track)?.is_none()
            && let Some(other) = counterpart(conn, kind, &key, &meta, Some(track))?
        {
            merge_into_older(conn, track, other)?;
        }
    }
    Ok(())
}

/// Point a file's source row at its new path, for a move that already
/// rewrote the track's.
pub(crate) fn rename_file(conn: &Connection, old: &str, new: &str) -> Result<(), DbError> {
    conn.prepare_cached("UPDATE local_files SET path = ?1 WHERE path = ?2")?
        .execute(params![new, old])?;
    Ok(())
}
