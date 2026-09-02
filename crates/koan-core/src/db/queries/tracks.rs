use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::connection::DbError;

use super::albums::get_or_create_album;
use super::artists::get_or_create_artist;
use super::{PlaybackSource, TrackMeta, TrackRow};

/// Map a rusqlite Row to a TrackRow. Expects the standard column order:
/// id, album_id, artist_id, artist_name, album_artist_name, album_title,
/// disc, track_number, title, duration_ms, path,
/// codec, sample_rate, bit_depth, channels, bitrate,
/// genre, source, remote_id, cached_path
pub(crate) fn row_to_track_row(row: &rusqlite::Row) -> rusqlite::Result<TrackRow> {
    row_to_track_row_at(row, 0)
}

/// The same, for a query that selects something of its own before the track's
/// columns — a playlist entry selects its id and position first.
pub(crate) fn row_to_track_row_at(row: &rusqlite::Row, at: usize) -> rusqlite::Result<TrackRow> {
    let artist_name: String = row.get::<_, Option<String>>(at + 3)?.unwrap_or_default();
    Ok(TrackRow {
        id: row.get(at)?,
        album_id: row.get(at + 1)?,
        artist_id: row.get(at + 2)?,
        artist_name: artist_name.clone(),
        album_artist_name: row.get::<_, Option<String>>(at + 4)?.unwrap_or(artist_name),
        album_title: row.get::<_, Option<String>>(at + 5)?.unwrap_or_default(),
        disc: row.get(at + 6)?,
        track_number: row.get(at + 7)?,
        title: row.get(at + 8)?,
        duration_ms: row.get(at + 9)?,
        path: row.get(at + 10)?,
        codec: row.get(at + 11)?,
        sample_rate: row.get(at + 12)?,
        bit_depth: row.get(at + 13)?,
        channels: row.get(at + 14)?,
        bitrate: row.get(at + 15)?,
        genre: row.get(at + 16)?,
        source: row.get(at + 17)?,
        remote_id: row.get(at + 18)?,
        cached_path: row.get(at + 19)?,
    })
}

/// Column values already on a row that is being merged into. The incoming
/// `TrackMeta` fills gaps from here; it never overwrites a populated column with NULL.
struct ExistingTrack {
    album_id: Option<i64>,
    artist_id: Option<i64>,
    path: Option<String>,
    remote_id: Option<String>,
    remote_url: Option<String>,
    cached_path: Option<String>,
    codec: Option<String>,
    sample_rate: Option<i32>,
    bit_depth: Option<i32>,
    channels: Option<i32>,
    bitrate: Option<i32>,
    duration_ms: Option<i64>,
    size_bytes: Option<i64>,
    mtime: Option<i64>,
    genre: Option<String>,
    mbid: Option<String>,
}

impl ExistingTrack {
    fn load(conn: &Connection, id: i64) -> Result<Self, DbError> {
        Ok(conn.query_row(
            "SELECT album_id, artist_id, path, remote_id, remote_url, cached_path, codec,
                    sample_rate, bit_depth, channels, bitrate, duration_ms, size_bytes,
                    mtime, genre, mbid
             FROM tracks WHERE id = ?1",
            params![id],
            |row| {
                Ok(ExistingTrack {
                    album_id: row.get(0)?,
                    artist_id: row.get(1)?,
                    path: row.get(2)?,
                    remote_id: row.get(3)?,
                    remote_url: row.get(4)?,
                    cached_path: row.get(5)?,
                    codec: row.get(6)?,
                    sample_rate: row.get(7)?,
                    bit_depth: row.get(8)?,
                    channels: row.get(9)?,
                    bitrate: row.get(10)?,
                    duration_ms: row.get(11)?,
                    size_bytes: row.get(12)?,
                    mtime: row.get(13)?,
                    genre: row.get(14)?,
                    mbid: row.get(15)?,
                })
            },
        )?)
    }

    /// Absorb a row that is about to be merged away. It fills gaps and never wins:
    /// the surviving row was matched on its own path or remote id, so where both
    /// have a value it is the one describing the copy that is actually there.
    fn absorb(&mut self, other: ExistingTrack) {
        self.path = self.path.take().or(other.path);
        self.remote_id = self.remote_id.take().or(other.remote_id);
        self.remote_url = self.remote_url.take().or(other.remote_url);
        self.cached_path = self.cached_path.take().or(other.cached_path);
        self.codec = self.codec.take().or(other.codec);
        self.genre = self.genre.take().or(other.genre);
        self.mbid = self.mbid.take().or(other.mbid);
        self.sample_rate = self.sample_rate.or(other.sample_rate);
        self.bit_depth = self.bit_depth.or(other.bit_depth);
        self.channels = self.channels.or(other.channels);
        self.bitrate = self.bitrate.or(other.bitrate);
        self.duration_ms = self.duration_ms.or(other.duration_ms);
        self.size_bytes = self.size_bytes.or(other.size_bytes);
        self.mtime = self.mtime.or(other.mtime);
    }
}

/// Insert or update a track. Deduplicates local+remote: one row per logical track.
///
/// Matching priority:
/// 1. By path (local tracks)
/// 2. By remote_id (remote tracks)
/// 3. By content match: same artist_id + album_id + disc + track# + title.
///    Cross-source only — two rows that both carry a local path, or that both
///    carry a remote_id, are two tracks, not one. `disc` is part of the identity
///    because multi-disc releases repeat both title and track number across discs.
/// 4. The same, minus the artist, when both sides carry a track number. Sources
///    disagree about how to credit a release; album + disc + track# + title
///    already names one position on it.
///
/// A row matched by path or remote_id is then asked the content-match question a
/// second time, against the corrected metadata: strategies 1 and 2 pin a row to
/// one source, so a file whose tags were bad when it was first indexed could never
/// merge with its remote copy however good the tags later became. If a counterpart
/// turns up, it is folded in and deleted rather than left as a duplicate.
///
/// A merge never replaces a populated column with NULL: a remote sync that knows
/// nothing about sample rate or bit depth leaves the locally-scanned values alone.
/// An existing path is only repointed at a different file once the old one is gone.
/// The `source` field reflects what's available: "local" if path exists, "remote" if remote-only.
pub fn upsert_track(conn: &Connection, meta: &TrackMeta) -> Result<i64, DbError> {
    upsert_track_status(conn, meta).map(|(id, _)| id)
}

/// `upsert_track`, additionally reporting whether a new row was inserted (`true`)
/// or an existing one updated (`false`).
pub fn upsert_track_status(conn: &Connection, meta: &TrackMeta) -> Result<(i64, bool), DbError> {
    // Use a savepoint so this works both standalone and inside an existing
    // transaction (e.g. the chunk transactions in scan_folder).
    conn.execute_batch("SAVEPOINT upsert_track")?;

    let result = upsert_track_inner(conn, meta);
    match &result {
        Ok(_) => conn.execute_batch("RELEASE upsert_track")?,
        Err(_) => conn.execute_batch("ROLLBACK TO upsert_track; RELEASE upsert_track")?,
    }
    result
}

fn upsert_track_inner(conn: &Connection, meta: &TrackMeta) -> Result<(i64, bool), DbError> {
    let album_artist_name = meta.album_artist.as_deref().unwrap_or(&meta.artist);
    let album_artist_id =
        get_or_create_artist(conn, album_artist_name, meta.artist_remote_id.as_deref())?;
    // Track artist — may differ from album artist (e.g. compilations, VA albums).
    let track_artist_id = if meta.artist == album_artist_name {
        album_artist_id
    } else {
        get_or_create_artist(conn, &meta.artist, None)?
    };
    let album_id = get_or_create_album(
        conn,
        &meta.album,
        album_artist_id,
        meta.date.as_deref(),
        None,
        None,
        meta.codec.as_deref(),
        meta.label.as_deref(),
        meta.album_remote_id.as_deref(),
        meta.album_added_at.as_deref(),
    )?;

    // 1. Match by path.
    let track_id: Option<i64> = if let Some(ref path) = meta.path {
        conn.query_row(
            "SELECT id FROM tracks WHERE path = ?1",
            params![path],
            |row| row.get(0),
        )
        .ok()
    } else {
        None
    };

    // 2. Match by remote_id.
    let track_id = track_id.or_else(|| {
        meta.remote_id.as_ref().and_then(|rid| {
            conn.query_row(
                "SELECT id FROM tracks WHERE remote_id = ?1",
                params![rid],
                |row| row.get(0),
            )
            .ok()
        })
    });

    // 3. Content match: same artist + album + disc + track# + title (cross-source dedup).
    // The two NULL clauses keep this to genuine local<->remote merges: two files on
    // disk are two tracks however identical their tags, and so are two entries on the
    // same server. A server that rotates its IDs now yields visible duplicates rather
    // than silently swallowing one of them — losing beats confusing.
    let track_id = track_id.or_else(|| {
        conn.query_row(
            "SELECT id FROM tracks
             WHERE artist_id = ?1 AND album_id = ?2 AND title = ?3
               AND COALESCE(track_number, -1) = COALESCE(?4, -1)
               AND COALESCE(disc, -1) = COALESCE(?5, -1)
               AND (path IS NULL OR ?6 IS NULL)
               AND (remote_id IS NULL OR ?7 IS NULL)",
            params![
                track_artist_id,
                album_id,
                meta.title,
                meta.track_number,
                meta.disc,
                meta.path,
                meta.remote_id
            ],
            |row| row.get(0),
        )
        .ok()
    });

    // 4. Same slot on the same release, whatever each source calls the artist.
    // A server states the credit its own way — "Booka Shade" locally against
    // "Booka Shade • Walter Merziger, Arno Kammermeier" over Subsonic, or merely a
    // different case — and step 3 then reads one track as two. Album, disc, track
    // number and title already name a single position on a release, so the artist
    // is what the sources disagree about rather than what distinguishes them.
    //
    // A track number is required on both sides: without one every untitled slot
    // on a release collapses to the same key, and the artist was the only thing
    // keeping two of them apart. The cross-source NULL clauses still apply.
    let track_id = track_id.or_else(|| {
        // No position on the release, so nothing this step can match on.
        meta.track_number?;
        conn.query_row(
            "SELECT id FROM tracks
             WHERE album_id = ?1 AND title = ?2
               AND track_number IS NOT NULL AND track_number = ?3
               AND COALESCE(disc, -1) = COALESCE(?4, -1)
               AND (path IS NULL OR ?5 IS NULL)
               AND (remote_id IS NULL OR ?6 IS NULL)",
            params![
                album_id,
                meta.title,
                meta.track_number,
                meta.disc,
                meta.path,
                meta.remote_id
            ],
            |row| row.get(0),
        )
        .ok()
    });

    if let Some(id) = track_id {
        // Merge: the incoming meta fills gaps, it never blanks what is already there.
        // A local scan supplies path + audio properties; a remote sync supplies
        // remote_id + remote_url and knows nothing about sample rate or bit depth.
        let mut existing = ExistingTrack::load(conn, id)?;

        // 4. Re-merge. Strategies 1 and 2 pin a row to the source it was first seen
        // from, so once it exists every later scan updates it in place and never
        // reconsiders the cross-source merge. That is wrong exactly when the original
        // tags were bad: correcting them gives the row the identity that would have
        // matched its counterpart, and without this the library keeps both copies.
        // Ask the strategy-3 question again against the corrected metadata, and fold
        // the counterpart in if it is there.
        //
        // Only worth asking while the row is still single-sourced. One already
        // carrying both a path and a remote id has nothing left to absorb, which is
        // also what keeps this off the back of a strategy-3 match.
        let mut vacated = vec![(existing.album_id, existing.artist_id)];
        let have_path = meta.path.is_some() || existing.path.is_some();
        let have_remote = meta.remote_id.is_some() || existing.remote_id.is_some();
        if have_path != have_remote {
            let counterpart: Option<i64> = conn
                .query_row(
                    "SELECT id FROM tracks
                     WHERE id != ?1 AND artist_id = ?2 AND album_id = ?3 AND title = ?4
                       AND COALESCE(track_number, -1) = COALESCE(?5, -1)
                       AND COALESCE(disc, -1) = COALESCE(?6, -1)
                       AND (path IS NULL OR ?7 = 0)
                       AND (remote_id IS NULL OR ?8 = 0)",
                    params![
                        id,
                        track_artist_id,
                        album_id,
                        meta.title,
                        meta.track_number,
                        meta.disc,
                        have_path,
                        have_remote
                    ],
                    |row| row.get(0),
                )
                .ok();

            if let Some(loser) = counterpart {
                let absorbed = ExistingTrack::load(conn, loser)?;
                vacated.push((absorbed.album_id, absorbed.artist_id));
                existing.absorb(absorbed);
                merge_track_rows(conn, loser, id)?;
                log::info!(
                    "corrected tags matched track {} with {}; merged them into one row",
                    loser,
                    id
                );
            }
        }

        // Only repoint at a different file once the old one is gone, so an upsert
        // can never make a file that still exists unreachable.
        let merged_path = match (meta.path.as_ref(), existing.path.as_ref()) {
            (Some(incoming), Some(current))
                if incoming != current && Path::new(current).exists() =>
            {
                log::warn!(
                    "track {} already points at {}; not repointing it at {}",
                    id,
                    current,
                    incoming
                );
                Some(current)
            }
            (Some(incoming), _) => Some(incoming),
            (None, current) => current,
        };
        let merged_remote_id = meta.remote_id.as_ref().or(existing.remote_id.as_ref());
        let merged_remote_url = meta.remote_url.as_ref().or(existing.remote_url.as_ref());
        // Whatever was already downloaded stays reachable; dropping the reference
        // would leak the file into the cache with nothing left pointing at it.
        let merged_cached_path = existing.cached_path.as_ref();
        let merged_mbid = existing.mbid.as_ref().or(meta.mbid.as_ref());
        let merged_codec = meta.codec.as_ref().or(existing.codec.as_ref());
        let merged_genre = meta.genre.as_ref().or(existing.genre.as_ref());
        let merged_sample_rate = meta.sample_rate.or(existing.sample_rate);
        let merged_bit_depth = meta.bit_depth.or(existing.bit_depth);
        let merged_channels = meta.channels.or(existing.channels);
        let merged_bitrate = meta.bitrate.or(existing.bitrate);
        let merged_duration_ms = meta.duration_ms.or(existing.duration_ms);
        let merged_size_bytes = meta.size_bytes.or(existing.size_bytes);
        let merged_mtime = meta.mtime.or(existing.mtime);

        // Source reflects what's available: local path wins.
        let source = if merged_path.is_some() {
            "local"
        } else {
            &meta.source
        };

        conn.execute(
            "UPDATE tracks SET album_id=?1, artist_id=?2, disc=?3, track_number=?4,
             title=?5, duration_ms=?6, codec=?7, sample_rate=?8, bit_depth=?9,
             channels=?10, bitrate=?11, size_bytes=?12, mtime=?13, genre=?14,
             source=?15, remote_id=?16, remote_url=?17, path=?18, mbid=?19,
             cached_path=?20
             WHERE id=?21",
            params![
                album_id,
                track_artist_id,
                meta.disc,
                meta.track_number,
                meta.title,
                merged_duration_ms,
                merged_codec,
                merged_sample_rate,
                merged_bit_depth,
                merged_channels,
                merged_bitrate,
                merged_size_bytes,
                merged_mtime,
                merged_genre,
                source,
                merged_remote_id,
                merged_remote_url,
                merged_path,
                merged_mbid,
                merged_cached_path,
                id
            ],
        )?;

        conn.execute("DELETE FROM tracks_fts WHERE rowid = ?1", params![id])?;
        // Index both track artist and album artist for FTS searchability.
        let fts_artist = if meta.artist == album_artist_name {
            meta.artist.clone()
        } else {
            format!("{} {}", meta.artist, album_artist_name)
        };
        conn.execute(
            "INSERT INTO tracks_fts (rowid, title, artist_name, album_title, genre)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, meta.title, fts_artist, meta.album, merged_genre],
        )?;

        for (old_album, old_artist) in vacated {
            prune_if_empty(
                conn,
                old_album.filter(|a| *a != album_id),
                old_artist.filter(|a| *a != track_artist_id && *a != album_artist_id),
            )?;
        }

        Ok((id, false))
    } else {
        let source = if meta.path.is_some() {
            "local"
        } else {
            &meta.source
        };

        conn.execute(
            "INSERT INTO tracks (album_id, artist_id, disc, track_number, title,
             duration_ms, path, codec, sample_rate, bit_depth, channels, bitrate,
             size_bytes, mtime, genre, source, remote_id, remote_url, mbid)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)",
            params![
                album_id,
                track_artist_id,
                meta.disc,
                meta.track_number,
                meta.title,
                meta.duration_ms,
                meta.path,
                meta.codec,
                meta.sample_rate,
                meta.bit_depth,
                meta.channels,
                meta.bitrate,
                meta.size_bytes,
                meta.mtime,
                meta.genre,
                source,
                meta.remote_id,
                meta.remote_url,
                meta.mbid
            ],
        )?;

        let id = conn.last_insert_rowid();
        let fts_artist = if meta.artist == album_artist_name {
            meta.artist.clone()
        } else {
            format!("{} {}", meta.artist, album_artist_name)
        };
        conn.execute(
            "INSERT INTO tracks_fts (rowid, title, artist_name, album_title, genre)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, meta.title, fts_artist, meta.album, meta.genre],
        )?;

        Ok((id, true))
    }
}

/// Fold `loser` into `winner`, then delete it. The two rows are the same track
/// seen from different sources, so everything pointing at one has to end up
/// pointing at the other.
///
/// Play history concatenates: every one of those plays was a play of this track.
/// Lyrics and the embedding are one per track, so the winner keeps what it has and
/// inherits only what it is missing. Favourites need no move at all — they are
/// keyed by path, and the path survives on the winner.
fn merge_track_rows(conn: &Connection, loser: i64, winner: i64) -> rusqlite::Result<()> {
    for table in ["play_history", "scan_cache", "organize_log"] {
        conn.execute(
            &format!("UPDATE {table} SET track_id = ?1 WHERE track_id = ?2"),
            params![winner, loser],
        )?;
    }

    // One row per track: move the loser's across only into a gap, then drop the rest.
    for table in ["lyrics_cache", "track_vectors"] {
        conn.execute(
            &format!(
                "UPDATE {table} SET track_id = ?1 WHERE track_id = ?2
                   AND NOT EXISTS (SELECT 1 FROM {table} WHERE track_id = ?1)"
            ),
            params![winner, loser],
        )?;
        conn.execute(
            &format!("DELETE FROM {table} WHERE track_id = ?1"),
            params![loser],
        )?;
    }

    conn.execute("DELETE FROM tracks WHERE id = ?1", params![loser])?;
    conn.execute("DELETE FROM tracks_fts WHERE rowid = ?1", params![loser])?;
    Ok(())
}

/// Fold together the cross-source duplicates an earlier dedup key left behind.
///
/// Matching on the artist meant a local file and the same recording from a
/// server parted company the moment the two spelled the credit differently, and
/// the pair is already in the library by the time the key is fixed: a sync
/// matches the remote row by its own `remote_id` long before any content match
/// runs, so nothing after this point would ever bring them back together.
///
/// The local row wins. It carries the path and the audio properties read from
/// the file, and playback prefers it; the remote row contributes the identity
/// the server knows it by. Only a clean pair is touched — one row with a path
/// and no remote id, one with a remote id and no path, sharing an album, a
/// title, a disc and a track number — so anything ambiguous is left visible
/// rather than guessed at.
pub(crate) fn merge_split_cross_source_tracks(conn: &Connection) -> rusqlite::Result<()> {
    let pairs: Vec<(i64, i64)> = {
        let mut stmt = conn.prepare(
            "SELECT r.id, l.id
               FROM tracks l
               JOIN tracks r
                 ON r.album_id = l.album_id
                AND r.title = l.title
                AND r.track_number = l.track_number
                AND COALESCE(r.disc, -1) = COALESCE(l.disc, -1)
              WHERE l.album_id IS NOT NULL
                AND l.track_number IS NOT NULL
                AND l.path IS NOT NULL AND l.remote_id IS NULL
                AND r.path IS NULL AND r.remote_id IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    for (loser, winner) in pairs {
        let stranded: Option<i64> = conn
            .query_row(
                "SELECT artist_id FROM tracks WHERE id = ?1",
                params![loser],
                |row| row.get(0),
            )
            .optional()?;

        conn.execute(
            "UPDATE tracks SET
                 remote_id = (SELECT remote_id FROM tracks WHERE id = ?2),
                 remote_url = (SELECT remote_url FROM tracks WHERE id = ?2),
                 cached_path = COALESCE(cached_path, (SELECT cached_path FROM tracks WHERE id = ?2)),
                 cache_size_bytes = COALESCE(cache_size_bytes, (SELECT cache_size_bytes FROM tracks WHERE id = ?2)),
                 cache_download_date = COALESCE(cache_download_date, (SELECT cache_download_date FROM tracks WHERE id = ?2)),
                 genre = COALESCE(genre, (SELECT genre FROM tracks WHERE id = ?2)),
                 mbid = COALESCE(mbid, (SELECT mbid FROM tracks WHERE id = ?2))
               WHERE id = ?1",
            params![winner, loser],
        )?;

        merge_track_rows(conn, loser, winner)?;
        prune_if_empty(conn, None, stranded)?;
    }

    Ok(())
}

/// Drop an album or artist the last track just left. Correcting a tag moves a row
/// to a different album, and the one it came from is usually a misreading nobody
/// wants left in the browser looking like a record with nothing on it.
fn prune_if_empty(
    conn: &Connection,
    album_id: Option<i64>,
    artist_id: Option<i64>,
) -> rusqlite::Result<()> {
    if let Some(album_id) = album_id {
        conn.execute(
            "DELETE FROM albums WHERE id = ?1
               AND NOT EXISTS (SELECT 1 FROM tracks WHERE album_id = ?1)",
            params![album_id],
        )?;
    }

    if let Some(artist_id) = artist_id {
        let stranded: bool = conn.query_row(
            "SELECT NOT EXISTS (SELECT 1 FROM tracks WHERE artist_id = ?1)
                AND NOT EXISTS (SELECT 1 FROM albums WHERE artist_id = ?1)",
            params![artist_id],
            |row| row.get(0),
        )?;
        if stranded {
            conn.execute(
                "DELETE FROM similar_artists WHERE artist_id = ?1 OR similar_id = ?1",
                params![artist_id],
            )?;
            conn.execute("DELETE FROM artists WHERE id = ?1", params![artist_id])?;
        }
    }

    Ok(())
}

/// A folder holding fewer tracks than this is exempt from the removal-fraction
/// check, where one deleted track out of three is already 33%.
const STALE_CHECK_MIN_ROWS: i64 = 100;

/// Share of a folder's tracks that may vanish in a single scan before the removal
/// is treated as a mount failure rather than a deletion.
const MAX_STALE_FRACTION: f64 = 0.2;

/// Remove scan cache entries and tracks for paths that no longer exist in the given folder.
///
/// Remote-backed tracks (those with a `remote_id`) are demoted to remote-only
/// instead of deleted: their `path` is nulled, `source` set to "remote", and
/// local-only fields (`mtime`, `size_bytes`) cleared. This preserves streaming
/// fallback when a local drive is unplugged. When the drive comes back,
/// `upsert_track` content-match (strategy 3) re-merges the path automatically.
///
/// Pure-local tracks (no `remote_id`) are deleted outright, taking their play
/// history, lyrics and embedding with them, so a folder that is present but
/// unreadable must never look like a folder whose files were deleted. Two brakes
/// enforce that: an IO error is not read as "gone", and a run that would clear
/// more than [`MAX_STALE_FRACTION`] of a folder holding at least
/// [`STALE_CHECK_MIN_ROWS`] tracks is refused with [`DbError::UnsafeBulkDelete`].
///
/// `force_remove` lifts the second brake only, for the case where the files really
/// were deleted. The IO-error check still applies, and the caller is still
/// responsible for not calling this at all when the folder yielded no files.
///
/// Returns the paths removed or demoted, so a caller can show what it did.
pub fn remove_stale_tracks(
    conn: &Connection,
    folder: &Path,
    force_remove: bool,
) -> Result<Vec<String>, DbError> {
    let (lower, upper) = super::folder_prefix_range(folder);

    let total: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tracks WHERE path >= ?1 AND path < ?2",
        params![lower, upper],
        |row| row.get(0),
    )?;

    // Find tracks in this folder that no longer exist on disk.
    // A path below the upper bound is a path in this folder, and NULL is below
    // nothing — so this catches every track with a local path regardless of its
    // `source` flag, which a merged local+remote row may still call 'remote'.
    let mut stmt = conn.prepare(
        "SELECT t.id, t.path, t.remote_id FROM tracks t
         WHERE t.path >= ?1 AND t.path < ?2",
    )?;

    let stale: Vec<(i64, String, Option<String>)> = stmt
        .query_map(params![lower, upper], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?
        .filter_map(|r| r.ok())
        // `Ok(false)` only: a permission error or an ailing mount reports Err,
        // which is "cannot tell", not "deleted".
        .filter(|(_, path, _)| matches!(Path::new(path).try_exists(), Ok(false)))
        .collect();

    let count = stale.len();
    if !force_remove
        && total >= STALE_CHECK_MIN_ROWS
        && count as f64 > total as f64 * MAX_STALE_FRACTION
    {
        return Err(DbError::UnsafeBulkDelete(format!(
            "{} of {} tracks under {} are missing ({:.0}% of the folder) — that reads as an \
             unmounted or unreadable folder rather than a deletion, so nothing was removed. \
             If the files really are gone, re-run with `koan scan --force-remove`.",
            count,
            total,
            folder.display(),
            count as f64 / total as f64 * 100.0
        )));
    }

    if force_remove && count > 0 {
        log::warn!(
            "--force-remove: deleting {} of {} tracks under {} along with their play history",
            count,
            total,
            folder.display()
        );
    }

    for (id, path, remote_id) in &stale {
        // Match on track_id as well as path: a row whose path changed since it was
        // cached leaves an orphan that would otherwise block the delete below.
        conn.execute(
            "DELETE FROM scan_cache WHERE track_id = ?1 OR path = ?2",
            params![id, path],
        )?;

        if remote_id.is_some() {
            // Demote to remote-only: null out local fields, keep the row for streaming.
            conn.execute(
                "UPDATE tracks SET path = NULL, source = 'remote', mtime = NULL, size_bytes = NULL
                 WHERE id = ?1",
                params![id],
            )?;
        } else {
            // Pure local — delete entirely. Clean up all FK references first.
            conn.execute("DELETE FROM tracks_fts WHERE rowid = ?1", params![id])?;
            conn.execute("DELETE FROM lyrics_cache WHERE track_id = ?1", params![id])?;
            conn.execute("DELETE FROM play_history WHERE track_id = ?1", params![id])?;
            conn.execute("DELETE FROM track_vectors WHERE track_id = ?1", params![id])?;
            conn.execute("DELETE FROM tracks WHERE id = ?1", params![id])?;
        }
    }

    Ok(stale.into_iter().map(|(_, path, _)| path).collect())
}

/// Get all tracks for an artist, ordered chronologically (album date, disc, track#).
///
/// The album-artist half is a subquery on `albums` rather than `al.artist_id =
/// ?1` on the join: SQLite can only use an index for an `OR` when both sides
/// name the same table, so the join form read every track in the library to
/// find one artist's.
pub fn tracks_for_artist(conn: &Connection, artist_id: i64) -> Result<Vec<TrackRow>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.album_id, t.artist_id, a.name, aa.name, al.title,
                t.disc, t.track_number, t.title, t.duration_ms, t.path,
                t.codec, t.sample_rate, t.bit_depth, t.channels, t.bitrate,
                t.genre, t.source, t.remote_id, t.cached_path
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         LEFT JOIN artists aa ON al.artist_id = aa.id
         WHERE t.artist_id = ?1
                OR t.album_id IN (SELECT id FROM albums WHERE artist_id = ?1)
         ORDER BY al.date, al.title COLLATE LIBRARY, t.disc, t.track_number",
    )?;
    let rows = stmt
        .query_map(params![artist_id], row_to_track_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Load all tracks that have a local path into a HashMap keyed by path.
/// Used by the playlist builder to skip expensive lofty reads for known files.
///
/// For large libraries, prefer `tracks_by_paths()` which only fetches the
/// tracks you actually need.
pub fn all_tracks_by_path(
    conn: &Connection,
) -> Result<std::collections::HashMap<String, TrackRow>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.album_id, t.artist_id, a.name, aa.name, al.title,
                t.disc, t.track_number, t.title, t.duration_ms, t.path,
                t.codec, t.sample_rate, t.bit_depth, t.channels, t.bitrate,
                t.genre, t.source, t.remote_id, t.cached_path
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         LEFT JOIN artists aa ON al.artist_id = aa.id
         WHERE t.path IS NOT NULL",
    )?;

    let rows = stmt
        .query_map(params![], row_to_track_row)?
        .collect::<Result<Vec<_>, _>>()?;

    let mut map = std::collections::HashMap::with_capacity(rows.len());
    for row in rows {
        if let Some(ref path) = row.path {
            map.insert(path.clone(), row);
        }
    }
    Ok(map)
}

/// Load tracks matching a specific set of paths into a HashMap.
/// Processes in batches of 500 to stay within SQLite variable limits.
/// For small path sets this is dramatically cheaper than `all_tracks_by_path`.
pub fn tracks_by_paths(
    conn: &Connection,
    paths: &[String],
) -> Result<std::collections::HashMap<String, TrackRow>, DbError> {
    const BATCH_SIZE: usize = 500;
    let mut map = std::collections::HashMap::with_capacity(paths.len());

    for chunk in paths.chunks(BATCH_SIZE) {
        let placeholders: String = chunk
            .iter()
            .enumerate()
            .map(|(i, _)| {
                if i == 0 {
                    "?".to_string()
                } else {
                    ",?".to_string()
                }
            })
            .collect();

        let sql = format!(
            "SELECT t.id, t.album_id, t.artist_id, a.name, aa.name, al.title,
                    t.disc, t.track_number, t.title, t.duration_ms, t.path,
                    t.codec, t.sample_rate, t.bit_depth, t.channels, t.bitrate,
                    t.genre, t.source, t.remote_id, t.cached_path
             FROM tracks t
             LEFT JOIN artists a ON t.artist_id = a.id
             LEFT JOIN albums al ON t.album_id = al.id
             LEFT JOIN artists aa ON al.artist_id = aa.id
             WHERE t.path IN ({placeholders})"
        );

        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::types::ToSql> = chunk
            .iter()
            .map(|s| s as &dyn rusqlite::types::ToSql)
            .collect();
        let rows = stmt
            .query_map(params.as_slice(), row_to_track_row)?
            .collect::<Result<Vec<_>, _>>()?;

        for row in rows {
            if let Some(ref path) = row.path {
                map.insert(path.clone(), row);
            }
        }
    }

    Ok(map)
}

/// Get all tracks in the library, ordered by artist/album/disc/track.
pub fn all_tracks(conn: &Connection) -> Result<Vec<TrackRow>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.album_id, t.artist_id, a.name, aa.name, al.title,
                t.disc, t.track_number, t.title, t.duration_ms, t.path,
                t.codec, t.sample_rate, t.bit_depth, t.channels, t.bitrate,
                t.genre, t.source, t.remote_id, t.cached_path
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         LEFT JOIN artists aa ON al.artist_id = aa.id
         ORDER BY a.name COLLATE LIBRARY, al.date, al.title COLLATE LIBRARY, t.disc, t.track_number",
    )?;

    let rows = stmt
        .query_map(params![], row_to_track_row)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(rows)
}

/// Get random tracks from the library, optionally filtered by artist.
pub fn random_tracks(
    conn: &Connection,
    count: u32,
    artist_id: Option<i64>,
) -> Result<Vec<TrackRow>, DbError> {
    let (sql, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) =
        if let Some(aid) = artist_id {
            (
                "SELECT t.id, t.album_id, t.artist_id, a.name, aa.name, al.title,
                    t.disc, t.track_number, t.title, t.duration_ms, t.path,
                    t.codec, t.sample_rate, t.bit_depth, t.channels, t.bitrate,
                    t.genre, t.source, t.remote_id, t.cached_path
             FROM tracks t
             LEFT JOIN artists a ON t.artist_id = a.id
             LEFT JOIN albums al ON t.album_id = al.id
             LEFT JOIN artists aa ON al.artist_id = aa.id
             WHERE t.artist_id = ?1
                OR t.album_id IN (SELECT id FROM albums WHERE artist_id = ?1)
             ORDER BY RANDOM()
             LIMIT ?2"
                    .into(),
                vec![
                    Box::new(aid) as Box<dyn rusqlite::types::ToSql>,
                    Box::new(count),
                ],
            )
        } else {
            (
                "SELECT t.id, t.album_id, t.artist_id, a.name, aa.name, al.title,
                    t.disc, t.track_number, t.title, t.duration_ms, t.path,
                    t.codec, t.sample_rate, t.bit_depth, t.channels, t.bitrate,
                    t.genre, t.source, t.remote_id, t.cached_path
             FROM tracks t
             LEFT JOIN artists a ON t.artist_id = a.id
             LEFT JOIN albums al ON t.album_id = al.id
             LEFT JOIN artists aa ON al.artist_id = aa.id
             ORDER BY RANDOM()
             LIMIT ?1"
                    .into(),
                vec![Box::new(count) as Box<dyn rusqlite::types::ToSql>],
            )
        };
    let mut stmt = conn.prepare(&sql)?;
    let params_refs: Vec<&dyn rusqlite::types::ToSql> =
        params_vec.iter().map(|p| p.as_ref()).collect();
    let rows = stmt
        .query_map(params_refs.as_slice(), row_to_track_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Get all tracks with pagination.
pub fn all_tracks_paged(
    conn: &Connection,
    limit: u32,
    offset: u32,
) -> Result<Vec<TrackRow>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.album_id, t.artist_id, a.name, aa.name, al.title,
                t.disc, t.track_number, t.title, t.duration_ms, t.path,
                t.codec, t.sample_rate, t.bit_depth, t.channels, t.bitrate,
                t.genre, t.source, t.remote_id, t.cached_path
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         LEFT JOIN artists aa ON al.artist_id = aa.id
         ORDER BY a.name COLLATE LIBRARY, al.date, al.title COLLATE LIBRARY, t.disc, t.track_number
         LIMIT ?1 OFFSET ?2",
    )?;
    let rows = stmt
        .query_map(params![limit, offset], row_to_track_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Fetch many tracks in one query, in the order the ids were given.
///
/// Building a queue used to call `get_track_row` per id. That is one round trip
/// per track, and a thousand-track add felt like it.
pub fn tracks_by_ids(conn: &Connection, ids: &[i64]) -> Result<Vec<TrackRow>, DbError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = std::iter::repeat_n("?", ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT t.id, t.album_id, t.artist_id, a.name, aa.name, al.title,
                t.disc, t.track_number, t.title, t.duration_ms, t.path,
                t.codec, t.sample_rate, t.bit_depth, t.channels, t.bitrate,
                t.genre, t.source, t.remote_id, t.cached_path
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         LEFT JOIN artists aa ON al.artist_id = aa.id
         WHERE t.id IN ({placeholders})"
    );
    let mut stmt = conn.prepare(&sql)?;
    let params = rusqlite::params_from_iter(ids.iter());
    let rows = stmt
        .query_map(params, row_to_track_row)?
        .collect::<Result<Vec<_>, _>>()?;

    // SQL returns them in whatever order it likes; callers care about the order
    // they asked for, because that is the order they will be queued in.
    //
    // Looked up rather than taken: an id asked for twice must come back twice.
    // Removing each row as it was matched meant the second copy found nothing
    // and was quietly dropped — so queueing a track you already had in the
    // queue added nothing, and a playlist holding the same song twice played it
    // once.
    let by_id: HashMap<i64, TrackRow> = rows.into_iter().map(|r| (r.id, r)).collect();
    Ok(ids.iter().filter_map(|id| by_id.get(id).cloned()).collect())
}

/// Get a single track by ID with full metadata.
pub fn get_track_row(conn: &Connection, track_id: i64) -> Result<Option<TrackRow>, DbError> {
    let result = conn.query_row(
        "SELECT t.id, t.album_id, t.artist_id, a.name, aa.name, al.title,
                t.disc, t.track_number, t.title, t.duration_ms, t.path,
                t.codec, t.sample_rate, t.bit_depth, t.channels, t.bitrate,
                t.genre, t.source, t.remote_id, t.cached_path
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         LEFT JOIN artists aa ON al.artist_id = aa.id
         WHERE t.id = ?1",
        params![track_id],
        row_to_track_row,
    );

    match result {
        Ok(row) => Ok(Some(row)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Look up a track ID by its local file path.
pub fn track_id_by_path(conn: &Connection, path: &str) -> Result<Option<i64>, DbError> {
    let result = conn.query_row(
        "SELECT id FROM tracks WHERE path = ?1 OR cached_path = ?1 OR remote_url = ?1",
        params![path],
        |row| row.get(0),
    );
    match result {
        Ok(id) => Ok(Some(id)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Clear all cached_path values (used when purging the download cache).
pub fn clear_cached_paths(conn: &Connection) -> Result<(), DbError> {
    conn.execute(
        "UPDATE tracks SET cached_path = NULL, cache_size_bytes = NULL, cache_download_date = NULL",
        params![],
    )?;
    Ok(())
}

/// Where the named tracks were downloaded to, for the ones that were.
pub fn cached_paths_for(conn: &Connection, track_ids: &[i64]) -> Result<Vec<String>, DbError> {
    if track_ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = vec!["?"; track_ids.len()].join(",");
    let sql = format!(
        "SELECT cached_path FROM tracks WHERE id IN ({placeholders}) AND cached_path IS NOT NULL"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(track_ids), |row| row.get(0))?;
    Ok(rows.filter_map(Result::ok).collect())
}

/// Which of the named tracks have a downloaded copy.
pub fn downloaded_of(conn: &Connection, track_ids: &[i64]) -> Result<Vec<i64>, DbError> {
    if track_ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = vec!["?"; track_ids.len()].join(",");
    let sql =
        format!("SELECT id FROM tracks WHERE id IN ({placeholders}) AND cached_path IS NOT NULL");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(track_ids), |row| row.get(0))?;
    Ok(rows.filter_map(Result::ok).collect())
}

/// Forget where the named tracks were downloaded to. The rows stay: a remote
/// track is still in the library, it just has to be fetched again to play.
pub fn clear_cached_paths_for(conn: &Connection, track_ids: &[i64]) -> Result<(), DbError> {
    if track_ids.is_empty() {
        return Ok(());
    }
    let placeholders = vec!["?"; track_ids.len()].join(",");
    conn.execute(
        &format!(
            "UPDATE tracks SET cached_path = NULL, cache_size_bytes = NULL, \
             cache_download_date = NULL WHERE id IN ({placeholders})"
        ),
        rusqlite::params_from_iter(track_ids),
    )?;
    Ok(())
}

/// Update the cached_path for a track after downloading, recording size and timestamp.
pub fn set_cached_path(conn: &Connection, track_id: i64, path: &str) -> Result<(), DbError> {
    let size_bytes: Option<i64> = std::fs::metadata(path).ok().map(|m| m.len() as i64);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    conn.execute(
        "UPDATE tracks SET cached_path = ?1, cache_size_bytes = ?2, cache_download_date = ?3
         WHERE id = ?4",
        params![path, size_bytes, now, track_id],
    )?;
    Ok(())
}

/// Row returned by the cache eviction query.
#[derive(Debug, Clone)]
pub struct CachedAlbumInfo {
    pub album_id: i64,
    pub album_title: String,
    pub artist_name: String,
    pub total_size: i64,
    pub track_ids: Vec<i64>,
    pub cached_paths: Vec<String>,
}

/// Get cached albums ordered by LRU (oldest last-played first), excluding favourited tracks.
/// Returns albums with their total cache size and file paths for eviction.
pub fn cached_albums_lru(conn: &Connection) -> Result<Vec<CachedAlbumInfo>, DbError> {
    // Get all cached tracks with their last played timestamp.
    // A track is "protected" if it appears in the favourites table.
    // We exclude any album that has ANY favourited cached track.
    // Uses LEFT JOIN with pre-aggregated play_history to avoid O(N) correlated subquery.
    let mut stmt = conn.prepare(
        "SELECT t.id, t.album_id, COALESCE(al.title, 'Unknown'), COALESCE(a.name, 'Unknown'),
                t.cached_path, COALESCE(t.cache_size_bytes, 0),
                ph_max.last_play,
                EXISTS(SELECT 1 FROM favourites f
                       WHERE f.track_path = t.cached_path
                          OR f.track_path = t.path
                          OR f.track_path = t.remote_url) as is_fav
         FROM tracks t
         LEFT JOIN albums al ON t.album_id = al.id
         LEFT JOIN artists a ON al.artist_id = a.id
         LEFT JOIN (SELECT track_id, MAX(played_at) as last_play
                    FROM play_history GROUP BY track_id) ph_max
                ON ph_max.track_id = t.id
         WHERE t.cached_path IS NOT NULL
         ORDER BY t.album_id, t.disc, t.track_number",
    )?;

    struct CachedTrackRow {
        track_id: i64,
        album_id: Option<i64>,
        album_title: String,
        artist_name: String,
        cached_path: String,
        size: i64,
        last_play: Option<i64>,
        is_fav: bool,
    }

    let rows: Vec<CachedTrackRow> = stmt
        .query_map([], |row| {
            Ok(CachedTrackRow {
                track_id: row.get(0)?,
                album_id: row.get(1)?,
                album_title: row.get(2)?,
                artist_name: row.get(3)?,
                cached_path: row.get(4)?,
                size: row.get(5)?,
                last_play: row.get(6)?,
                is_fav: row.get(7)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    // Group by album_id. Use -1 for tracks without an album.
    let mut albums: std::collections::BTreeMap<i64, CachedAlbumInfo> =
        std::collections::BTreeMap::new();
    // Track max last_play per album, and whether album has any favourites.
    let mut album_last_play: HashMap<i64, Option<i64>> = HashMap::new();
    let mut album_has_fav: HashSet<i64> = HashSet::new();

    for r in &rows {
        let aid = r.album_id.unwrap_or(-r.track_id); // unique key for albumless tracks
        if r.is_fav {
            album_has_fav.insert(aid);
        }
        let entry = albums.entry(aid).or_insert_with(|| CachedAlbumInfo {
            album_id: aid,
            album_title: r.album_title.clone(),
            artist_name: r.artist_name.clone(),
            total_size: 0,
            track_ids: Vec::new(),
            cached_paths: Vec::new(),
        });
        entry.total_size += r.size;
        entry.track_ids.push(r.track_id);
        entry.cached_paths.push(r.cached_path.clone());

        let current_max = album_last_play.entry(aid).or_insert(None);
        *current_max = match (*current_max, r.last_play) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
    }

    // Filter out albums with any favourited tracks, then sort by last_play ascending (oldest first).
    // Never-played albums sort before everything (None < Some).
    let mut result: Vec<CachedAlbumInfo> = albums
        .into_values()
        .filter(|a| !album_has_fav.contains(&a.album_id))
        .collect();

    result.sort_by_key(|a| album_last_play.get(&a.album_id).copied().unwrap_or(None));

    Ok(result)
}

/// Get total cache size from DB tracking (sum of cache_size_bytes for all cached tracks).
pub fn total_cache_size(conn: &Connection) -> Result<i64, DbError> {
    let size: i64 = conn.query_row(
        "SELECT COALESCE(SUM(cache_size_bytes), 0) FROM tracks WHERE cached_path IS NOT NULL",
        [],
        |row| row.get(0),
    )?;
    Ok(size)
}

/// Clear cache tracking for specific tracks (after eviction deletes files).
pub fn clear_cache_for_tracks(conn: &Connection, track_ids: &[i64]) -> Result<(), DbError> {
    for &id in track_ids {
        conn.execute(
            "UPDATE tracks SET cached_path = NULL, cache_size_bytes = NULL, cache_download_date = NULL
             WHERE id = ?1",
            params![id],
        )?;
    }
    Ok(())
}

/// Resolve the best playback source for a track. Local > Cached > Remote.
pub fn resolve_playback_path(
    conn: &Connection,
    track_id: i64,
) -> Result<Option<PlaybackSource>, DbError> {
    let row = conn.query_row(
        "SELECT path, cached_path, remote_url, source FROM tracks WHERE id = ?1",
        params![track_id],
        |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
            ))
        },
    );

    match row {
        Ok((path, cached_path, remote_url, _source)) => {
            // Local file always wins.
            if let Some(p) = path {
                let pb = PathBuf::from(&p);
                if pb.exists() {
                    return Ok(Some(PlaybackSource::Local(pb)));
                }
            }
            // Cached download.
            if let Some(cp) = cached_path {
                let pb = PathBuf::from(&cp);
                if pb.exists() {
                    return Ok(Some(PlaybackSource::Cached(pb)));
                }
            }
            // Remote stream.
            if let Some(url) = remote_url {
                return Ok(Some(PlaybackSource::Remote(url)));
            }
            Ok(None)
        }
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Get tracks for a specific album, ordered by disc/track number.
pub fn tracks_for_album(conn: &Connection, album_id: i64) -> Result<Vec<TrackRow>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.album_id, t.artist_id, a.name, aa.name, al.title,
                t.disc, t.track_number, t.title, t.duration_ms, t.path,
                t.codec, t.sample_rate, t.bit_depth, t.channels, t.bitrate,
                t.genre, t.source, t.remote_id, t.cached_path
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         LEFT JOIN artists aa ON al.artist_id = aa.id
         WHERE t.album_id = ?1
         ORDER BY t.disc, t.track_number",
    )?;

    let rows = stmt
        .query_map(params![album_id], row_to_track_row)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(rows)
}

/// One track off an album, for anything that only needs a representative.
///
/// Artwork is the case: every track on a record shares the record's cover, so a
/// client wanting it needs any one of them. Asking for the album's tracks and
/// picking an id out of the answer is a listing built and carried across a
/// boundary to be thrown away — and on a grid of tiles, one of those per tile.
///
/// Prefers a track with a file, because art can then be read straight out of
/// the tag without asking the server at all.
pub fn cover_track_for_album(
    conn: &Connection,
    album_id: i64,
) -> Result<Option<TrackRow>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.album_id, t.artist_id, a.name, aa.name, al.title,
                t.disc, t.track_number, t.title, t.duration_ms, t.path,
                t.codec, t.sample_rate, t.bit_depth, t.channels, t.bitrate,
                t.genre, t.source, t.remote_id, t.cached_path
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         LEFT JOIN artists aa ON al.artist_id = aa.id
         WHERE t.album_id = ?1
         ORDER BY (t.path IS NULL AND t.cached_path IS NULL), t.disc, t.track_number
         LIMIT 1",
    )?;

    let mut rows = stmt.query_map(params![album_id], row_to_track_row)?;
    rows.next().transpose().map_err(Into::into)
}

/// Build a SQL `IN (?, ?, ...)` clause with the given number of placeholders.
fn in_clause(n: usize) -> String {
    let mut s = String::with_capacity(2 + n * 2);
    s.push('(');
    for i in 0..n {
        if i > 0 {
            s.push(',');
        }
        s.push('?');
    }
    s.push(')');
    s
}

/// Get distinct genres for a batch of artist IDs in a single query.
/// Returns a map from artist_id → set of lowercased genre strings.
pub fn genres_by_artist_ids(
    conn: &Connection,
    ids: &[i64],
) -> Result<HashMap<i64, HashSet<String>>, DbError> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let sql = format!(
        "SELECT t.artist_id, t.genre FROM tracks t
         WHERE t.artist_id IN {} AND t.genre IS NOT NULL
         UNION
         SELECT al.artist_id, t.genre FROM tracks t
         JOIN albums al ON t.album_id = al.id
         WHERE al.artist_id IN {} AND t.genre IS NOT NULL",
        in_clause(ids.len()),
        in_clause(ids.len()),
    );
    let mut stmt = conn.prepare(&sql)?;
    let params: Vec<Box<dyn rusqlite::types::ToSql>> = ids
        .iter()
        .chain(ids.iter())
        .map(|id| Box::new(*id) as Box<dyn rusqlite::types::ToSql>)
        .collect();
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let rows = stmt.query_map(param_refs.as_slice(), |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut map: HashMap<i64, HashSet<String>> = HashMap::new();
    for row in rows {
        let (artist_id, genre) = row?;
        map.entry(artist_id)
            .or_default()
            .insert(genre.to_lowercase());
    }
    Ok(map)
}

/// Get distinct genres for a batch of album IDs in a single query.
/// Returns a map from album_id → set of lowercased genre strings.
pub fn genres_by_album_ids(
    conn: &Connection,
    ids: &[i64],
) -> Result<HashMap<i64, HashSet<String>>, DbError> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let sql = format!(
        "SELECT t.album_id, t.genre FROM tracks t
         WHERE t.album_id IN {} AND t.genre IS NOT NULL",
        in_clause(ids.len()),
    );
    let mut stmt = conn.prepare(&sql)?;
    let params: Vec<Box<dyn rusqlite::types::ToSql>> = ids
        .iter()
        .map(|id| Box::new(*id) as Box<dyn rusqlite::types::ToSql>)
        .collect();
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let rows = stmt.query_map(param_refs.as_slice(), |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut map: HashMap<i64, HashSet<String>> = HashMap::new();
    for row in rows {
        let (album_id, genre) = row?;
        map.entry(album_id)
            .or_default()
            .insert(genre.to_lowercase());
    }
    Ok(map)
}

/// Get all artist IDs that have at least one favourited track, in a single query.
pub fn favourite_artist_ids_batch(conn: &Connection) -> Result<HashSet<i64>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT t.artist_id FROM tracks t
         JOIN favourites f ON (t.path = f.track_path OR t.cached_path = f.track_path)
         WHERE t.artist_id IS NOT NULL",
    )?;
    let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;
    let mut ids = HashSet::new();
    for row in rows {
        ids.insert(row?);
    }
    Ok(ids)
}

/// The path a track is favourited under.
///
/// Favourites are keyed by path, but which path depends on the track: a local
/// file has one, a cached remote track has a cache path, and a remote track
/// that has never been downloaded only has its URL. Without this, remote
/// tracks can't be favourited at all.
pub fn track_favourite_key(conn: &Connection, track_id: i64) -> Result<Option<String>, DbError> {
    let result = conn.query_row(
        "SELECT COALESCE(path, cached_path, remote_url) FROM tracks WHERE id = ?1",
        params![track_id],
        |row| row.get::<_, Option<String>>(0),
    );
    match result {
        Ok(key) => Ok(key),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Get all favourited track IDs in a single query.
///
/// Matches the same three columns as [`track_id_by_path`], `remote_url`
/// included — a remote track that has never been cached is favourited by its
/// remote URL, and comparing only local paths misses every one of them.
pub fn favourite_track_ids_batch(conn: &Connection) -> Result<HashSet<i64>, DbError> {
    // Three indexed lookups rather than one join with an OR across three
    // columns. SQLite cannot use an index for that OR, so it read every track
    // in the library and probed favourites for each — fifty milliseconds to
    // find a hundred rows, paid by every listing that shows a star. As a union
    // each branch searches its own index instead.
    let mut stmt = conn.prepare(
        "SELECT id FROM tracks WHERE path IN (SELECT track_path FROM favourites)
         UNION
         SELECT id FROM tracks WHERE cached_path IN (SELECT track_path FROM favourites)
         UNION
         SELECT id FROM tracks WHERE remote_url IN (SELECT track_path FROM favourites)",
    )?;
    let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;
    let mut ids = HashSet::new();
    for row in rows {
        ids.insert(row?);
    }
    Ok(ids)
}

/// Every favourited track, narrowed by `search` and ordered as a library
/// reads: artist, record, then running order.
///
/// One query rather than a favourite id list the caller resolves row by row —
/// which is what the id set is for, and it is not for this.
///
/// Matched through the same union of three indexed lookups as
/// [`favourite_track_ids_batch`], for the same reason: joining `favourites` on
/// an `OR` across the three path columns cannot use an index, and read the
/// whole library to find a hundred rows.
pub fn favourite_tracks(conn: &Connection, search: Option<&str>) -> Result<Vec<TrackRow>, DbError> {
    let mut sql = String::from(
        "SELECT t.id, t.album_id, t.artist_id, a.name, aa.name, al.title,
                t.disc, t.track_number, t.title, t.duration_ms, t.path,
                t.codec, t.sample_rate, t.bit_depth, t.channels, t.bitrate,
                t.genre, t.source, t.remote_id, t.cached_path
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         LEFT JOIN artists aa ON al.artist_id = aa.id
         WHERE t.id IN (
             SELECT id FROM tracks WHERE path IN (SELECT track_path FROM favourites)
             UNION
             SELECT id FROM tracks WHERE cached_path IN (SELECT track_path FROM favourites)
             UNION
             SELECT id FROM tracks WHERE remote_url IN (SELECT track_path FROM favourites))",
    );
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if let Some(query) = search {
        let pattern = format!("%{}%", super::artists::escape_like(query));
        for _ in 0..3 {
            params.push(Box::new(pattern.clone()));
        }
        sql.push_str(
            " AND (t.title LIKE ? COLLATE NOCASE ESCAPE '\\'
                OR a.name LIKE ? COLLATE NOCASE ESCAPE '\\'
                OR al.title LIKE ? COLLATE NOCASE ESCAPE '\\')",
        );
    }
    sql.push_str(
        " ORDER BY a.name COLLATE LIBRARY, al.title COLLATE LIBRARY, t.disc, t.track_number",
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(params.iter()), row_to_track_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Get all album IDs that have at least one favourited track, in a single query.
pub fn favourite_album_ids_batch(conn: &Connection) -> Result<HashSet<i64>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT t.album_id FROM tracks t
         JOIN favourites f ON (t.path = f.track_path OR t.cached_path = f.track_path)
         WHERE t.album_id IS NOT NULL",
    )?;
    let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;
    let mut ids = HashSet::new();
    for row in rows {
        ids.insert(row?);
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::Database;
    use crate::db::queries::{library_stats, sample_meta};

    fn test_db() -> Database {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        Database { conn }
    }

    #[test]
    fn favourite_tracks_come_back_as_rows_narrowed_by_search() {
        use crate::db::queries::toggle_favourite;
        let db = test_db();
        upsert_track(&db.conn, &sample_meta("Amber", "Autechre", "Amber")).unwrap();
        upsert_track(&db.conn, &sample_meta("Foil", "Autechre", "Amber")).unwrap();

        let titles = |q| {
            favourite_tracks(&db.conn, q)
                .unwrap()
                .into_iter()
                .map(|t| t.title)
                .collect::<Vec<_>>()
        };
        assert!(titles(None).is_empty(), "nothing is favourite until it is");

        toggle_favourite(&db.conn, Path::new("/music/Amber/Amber.flac")).unwrap();
        toggle_favourite(&db.conn, Path::new("/music/Amber/Foil.flac")).unwrap();
        assert_eq!(titles(None), ["Amber", "Foil"]);
        assert_eq!(titles(Some("foil")), ["Foil"]);
        assert_eq!(
            titles(Some("autechre")).len(),
            2,
            "matched on the artist name"
        );
    }

    /// A track is favourited by whichever of its three paths the user was
    /// looking at, and all three have to find it.
    #[test]
    fn favourite_tracks_finds_a_track_by_any_of_its_paths() {
        let db = test_db();
        db.conn
            .execute_batch(
                "INSERT INTO artists (id, name) VALUES (1, 'Boards of Canada');
                 INSERT INTO albums (id, title, artist_id) VALUES (1, 'Geogaddi', 1);
                 INSERT INTO tracks (id, title, artist_id, album_id, source, path)
                   VALUES (1, 'Music Is Math', 1, 1, 'local', '/music/math.flac');
                 INSERT INTO tracks (id, title, artist_id, album_id, source, cached_path)
                   VALUES (2, 'Sixtyten', 1, 1, 'cached', '/cache/sixtyten.flac');
                 INSERT INTO tracks (id, title, artist_id, album_id, source, remote_url)
                   VALUES (3, 'Dawn Chorus', 1, 1, 'remote', 'http://server/dawn');
                 INSERT INTO tracks (id, title, artist_id, album_id, source, path)
                   VALUES (4, 'Alpha and Omega', 1, 1, 'local', '/music/alpha.flac');
                 INSERT INTO favourites (track_path) VALUES
                   ('/music/math.flac'), ('/cache/sixtyten.flac'), ('http://server/dawn');",
            )
            .unwrap();

        let mut ids = favourite_tracks(&db.conn, None)
            .unwrap()
            .into_iter()
            .map(|t| t.id)
            .collect::<Vec<_>>();
        ids.sort();
        assert_eq!(ids, [1, 2, 3], "the unfavourited fourth track stays out");

        let narrowed = favourite_tracks(&db.conn, Some("dawn")).unwrap();
        assert_eq!(
            narrowed.len(),
            1,
            "search narrows the favourites, not all of them"
        );
        assert_eq!(narrowed[0].id, 3);
    }

    /// A track belongs to an artist by its own credit or its album's, and a
    /// compilation is the case where only the second one holds.
    #[test]
    fn tracks_for_artist_counts_the_album_credit() {
        let db = test_db();
        db.conn
            .execute_batch(
                "INSERT INTO artists (id, name) VALUES (1, 'Aphex Twin'), (2, 'Various');
                 INSERT INTO albums (id, title, artist_id) VALUES
                   (1, 'Selected Ambient Works', 1), (2, 'Artificial Intelligence', 2);
                 -- Own credit, on their own record.
                 INSERT INTO tracks (id, title, artist_id, album_id, source, path)
                   VALUES (1, 'Xtal', 1, 1, 'local', '/music/xtal.flac');
                 -- Own credit, on somebody else's compilation.
                 INSERT INTO tracks (id, title, artist_id, album_id, source, path)
                   VALUES (2, 'Polygon Window', 1, 2, 'local', '/music/polygon.flac');
                 -- Album credit only: uncredited track on their record.
                 INSERT INTO tracks (id, title, album_id, source, path)
                   VALUES (3, 'Untitled', 1, 'local', '/music/untitled.flac');
                 -- Neither.
                 INSERT INTO tracks (id, title, artist_id, album_id, source, path)
                   VALUES (4, 'The Clan Call', 2, 2, 'local', '/music/clan.flac');",
            )
            .unwrap();

        let mut ids = tracks_for_artist(&db.conn, 1)
            .unwrap()
            .into_iter()
            .map(|t| t.id)
            .collect::<Vec<_>>();
        ids.sort();
        assert_eq!(ids, [1, 2, 3]);
    }

    #[test]
    fn an_id_asked_for_twice_comes_back_twice() {
        let db = test_db();
        let a = upsert_track(&db.conn, &sample_meta("One", "Artist", "Album")).unwrap();
        let b = upsert_track(&db.conn, &sample_meta("Two", "Artist", "Album")).unwrap();

        let rows = tracks_by_ids(&db.conn, &[a, b, a]).unwrap();
        assert_eq!(
            rows.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![a, b, a],
            "a queue may hold the same track twice, and a playlist certainly may"
        );
    }

    #[test]
    fn test_upsert_track() {
        let db = test_db();
        let meta = sample_meta("Windowlicker", "Aphex Twin", "Windowlicker EP");
        let id1 = upsert_track(&db.conn, &meta).unwrap();

        // Same path → same track ID (upsert).
        let id2 = upsert_track(&db.conn, &meta).unwrap();
        assert_eq!(id1, id2);

        let stats = library_stats(&db.conn).unwrap();
        assert_eq!(stats.total_tracks, 1);
        assert_eq!(stats.local_tracks, 1);
    }

    #[test]
    fn test_dedup_keeps_discs_apart() {
        let db = test_db();

        // A 2-CD box set: same album, same title, both track 1, differing only in disc.
        let mut cd1 = sample_meta("Overture", "Wagner", "Ring Cycle");
        cd1.disc = Some(1);
        cd1.path = Some("/music/Ring Cycle/CD1/01 - Overture.flac".into());
        let mut cd2 = cd1.clone();
        cd2.disc = Some(2);
        cd2.path = Some("/music/Ring Cycle/CD2/01 - Overture.flac".into());

        let id1 = upsert_track(&db.conn, &cd1).unwrap();
        let id2 = upsert_track(&db.conn, &cd2).unwrap();

        assert_ne!(id1, id2, "discs 1 and 2 must not collapse into one row");
        assert_eq!(library_stats(&db.conn).unwrap().total_tracks, 2);

        let paths: Vec<String> = db
            .conn
            .prepare("SELECT path FROM tracks ORDER BY disc")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(paths, vec![cd1.path.unwrap(), cd2.path.unwrap()]);
    }

    #[test]
    fn test_dedup_never_merges_two_local_files() {
        let db = test_db();

        // Identical tags including disc — two files on disk are two tracks.
        let mut a = sample_meta("Intro", "Various", "Compilation");
        a.path = Some("/music/Compilation/a.flac".into());
        let mut b = a.clone();
        b.path = Some("/music/Compilation/b.flac".into());

        let id_a = upsert_track(&db.conn, &a).unwrap();
        let id_b = upsert_track(&db.conn, &b).unwrap();

        assert_ne!(id_a, id_b);
        assert_eq!(library_stats(&db.conn).unwrap().total_tracks, 2);
    }

    #[test]
    fn test_dedup_never_merges_two_remote_entries() {
        let db = test_db();

        // Two entries on the same server, no disc reported — identical but for
        // their remote ids. Strategy 2 misses, and strategy 3 must not catch them.
        let mut first = sample_meta("Untitled", "Artist", "Album");
        first.source = "remote".into();
        first.path = None;
        first.disc = None;
        first.remote_id = Some("sub-1".into());
        let mut second = first.clone();
        second.remote_id = Some("sub-2".into());

        let id1 = upsert_track(&db.conn, &first).unwrap();
        let id2 = upsert_track(&db.conn, &second).unwrap();

        assert_ne!(
            id1, id2,
            "two server entries must not collapse into one row"
        );
        assert_eq!(library_stats(&db.conn).unwrap().total_tracks, 2);
    }

    /// The remote copy of a track: no path, a remote id, correct tags.
    fn remote_meta(title: &str, artist: &str, album: &str, remote_id: &str) -> TrackMeta {
        let mut meta = sample_meta(title, artist, album);
        meta.source = "remote".into();
        meta.path = None;
        meta.remote_id = Some(remote_id.into());
        meta.remote_url = Some(format!("https://server/rest/stream?id={remote_id}"));
        meta.sample_rate = None;
        meta.bit_depth = None;
        meta
    }

    #[test]
    fn test_corrected_tags_remerge_with_the_remote_copy() {
        let db = test_db();

        // The file as first indexed: ID3v1 truncated the title and the album, and
        // the track number never made it. Nothing about it can content-match.
        let mut bad = sample_meta(
            "Golden Skans (David E Sugar R",
            "Klaxons",
            "Golden Skans (David E Sugar R",
        );
        bad.path = Some("/music/klaxons/01.mp3".into());
        bad.track_number = None;
        let local_id = upsert_track(&db.conn, &bad).unwrap();

        let remote = remote_meta(
            "Golden Skans (David E Sugar Remix)",
            "Klaxons",
            "Golden Skans (David E Sugar Remix)",
            "sub-42",
        );
        let remote_id = upsert_track(&db.conn, &remote).unwrap();
        assert_ne!(local_id, remote_id, "bad tags cannot content-match");
        assert_eq!(library_stats(&db.conn).unwrap().total_tracks, 2);

        // Tags read correctly this time. The path still matches, so strategy 1
        // wins — the re-merge is what has to notice the remote copy.
        let mut fixed = bad.clone();
        fixed.title = "Golden Skans (David E Sugar Remix)".into();
        fixed.album = "Golden Skans (David E Sugar Remix)".into();
        fixed.track_number = Some(1);
        let merged = upsert_track(&db.conn, &fixed).unwrap();

        assert_eq!(merged, local_id, "the row holding the file survives");
        assert_eq!(library_stats(&db.conn).unwrap().total_tracks, 1);

        let row = get_track_row(&db.conn, merged).unwrap().unwrap();
        assert_eq!(row.path.as_deref(), Some("/music/klaxons/01.mp3"));
        assert_eq!(row.remote_id.as_deref(), Some("sub-42"));
        assert_eq!(row.source, "local");
        assert_eq!(row.sample_rate, Some(44100), "local audio properties kept");

        // The album the bad tags invented goes with it.
        let albums: Vec<String> = db
            .conn
            .prepare("SELECT title FROM albums")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(albums, vec!["Golden Skans (David E Sugar Remix)"]);
    }

    #[test]
    fn test_remerge_carries_history_and_lyrics_across() {
        let db = test_db();

        let mut bad = sample_meta("Untitled", "Boards of Canada", "Geogaddi");
        bad.path = Some("/music/boc/05.flac".into());
        let local_id = upsert_track(&db.conn, &bad).unwrap();

        let remote = remote_meta("Sunshine Recorder", "Boards of Canada", "Geogaddi", "sub-7");
        let remote_id = upsert_track(&db.conn, &remote).unwrap();

        // Both rows have been played, and only the remote one has lyrics.
        crate::db::queries::record_play(&db.conn, local_id, Some(1_000)).unwrap();
        crate::db::queries::record_play(&db.conn, remote_id, Some(2_000)).unwrap();
        crate::db::queries::cache_lyrics(&db.conn, remote_id, "lrclib", true, "[00:01.00] la")
            .unwrap();

        let mut fixed = bad.clone();
        fixed.title = "Sunshine Recorder".into();
        let merged = upsert_track(&db.conn, &fixed).unwrap();
        assert_eq!(merged, local_id);

        assert_eq!(
            crate::db::queries::play_count(&db.conn, merged).unwrap(),
            2,
            "both rows' plays were plays of this track"
        );
        assert!(
            crate::db::queries::get_cached_lyrics(&db.conn, merged)
                .unwrap()
                .is_some()
        );
        let orphans: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM play_history WHERE track_id NOT IN (SELECT id FROM tracks)",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(orphans, 0);
    }

    #[test]
    fn test_remerge_never_folds_two_local_files() {
        let db = test_db();

        let mut first = sample_meta("Intro", "Various", "Compilation");
        first.path = Some("/music/comp/a.flac".into());
        let mut second = first.clone();
        second.title = "Untitled".into();
        second.path = Some("/music/comp/b.flac".into());

        let id_a = upsert_track(&db.conn, &first).unwrap();
        let id_b = upsert_track(&db.conn, &second).unwrap();

        // b's tags are corrected into an exact match for a. Two files on disk are
        // still two tracks.
        second.title = "Intro".into();
        assert_eq!(upsert_track(&db.conn, &second).unwrap(), id_b);
        assert_ne!(id_a, id_b);
        assert_eq!(library_stats(&db.conn).unwrap().total_tracks, 2);
    }

    #[test]
    fn test_remerge_folds_a_renamed_remote_entry_into_the_local_file() {
        let db = test_db();

        let local = sample_meta("Windowlicker", "Aphex Twin", "Windowlicker");
        let local_id = upsert_track(&db.conn, &local).unwrap();

        // The server first reported the title wrong, so the sync made its own row.
        let mut remote = remote_meta("Windowlickr", "Aphex Twin", "Windowlicker", "sub-1");
        let remote_id = upsert_track(&db.conn, &remote).unwrap();
        assert_ne!(local_id, remote_id);

        // The server's metadata is fixed; strategy 2 matches its own row, and the
        // re-merge has to spot the local file it now describes.
        remote.title = "Windowlicker".into();
        let merged = upsert_track(&db.conn, &remote).unwrap();

        assert_eq!(merged, remote_id, "the row holding the remote id survives");
        assert_eq!(library_stats(&db.conn).unwrap().total_tracks, 1);
        let row = get_track_row(&db.conn, merged).unwrap().unwrap();
        assert_eq!(row.path, local.path);
        assert_eq!(row.remote_id.as_deref(), Some("sub-1"));
        assert_eq!(row.source, "local");
    }

    #[test]
    fn test_force_remove_lifts_the_fraction_brake_only() {
        let db = test_db();
        for i in 0..STALE_CHECK_MIN_ROWS + 20 {
            let mut meta = sample_meta(&format!("Track{}", i), "Artist", "Album");
            meta.track_number = Some(i as i32);
            meta.path = Some(format!("/music/Album/{}.flac", i));
            upsert_track(&db.conn, &meta).unwrap();
        }
        let total = library_stats(&db.conn).unwrap().total_tracks as usize;

        let removed = remove_stale_tracks(&db.conn, Path::new("/music"), true).unwrap();
        assert_eq!(removed.len(), total, "every missing file should go");
        assert_eq!(library_stats(&db.conn).unwrap().total_tracks, 0);
        assert!(
            removed.iter().all(|p| p.starts_with("/music/Album/")),
            "the removed paths should be reported back"
        );
    }

    #[test]
    fn test_remote_upsert_preserves_local_audio_properties() {
        let db = test_db();

        // Local scan: full audio properties.
        let local = sample_meta("Song", "Artist", "Album");
        let id = upsert_track(&db.conn, &local).unwrap();

        // Remote sync knows the codec suffix and nothing else about the file.
        let mut remote = sample_meta("Song", "Artist", "Album");
        remote.source = "remote".into();
        remote.path = None;
        remote.remote_id = Some("sub-1".into());
        remote.sample_rate = None;
        remote.bit_depth = None;
        remote.channels = None;
        remote.size_bytes = None;
        remote.mtime = None;
        remote.codec = None;
        assert_eq!(upsert_track(&db.conn, &remote).unwrap(), id);

        let codec: Option<String> = db
            .conn
            .query_row("SELECT codec FROM tracks WHERE id = ?1", params![id], |r| {
                r.get(0)
            })
            .unwrap();
        let num = |col: &str| -> Option<i64> {
            db.conn
                .query_row(
                    &format!("SELECT {} FROM tracks WHERE id = ?1", col),
                    params![id],
                    |r| r.get(0),
                )
                .unwrap()
        };

        assert_eq!(codec.as_deref(), Some("FLAC"));
        assert_eq!(num("sample_rate"), Some(44100));
        assert_eq!(num("bit_depth"), Some(16));
        assert_eq!(num("channels"), Some(2));
        assert_eq!(num("size_bytes"), Some(30_000_000));
        assert_eq!(num("mtime"), Some(1700000000));
    }

    #[test]
    fn test_dedup_matches_across_differing_artist_credits() {
        let db = test_db();

        // Local tags name the band. Navidrome hands back the same recording with
        // every contributor spliced onto the credit, so the two used to land as
        // separate artists and therefore separate tracks on one album page.
        let local = sample_meta("Treading Water", "Petrol Girls", "Talk of Violence");
        let id = upsert_track(&db.conn, &local).unwrap();

        let mut remote = sample_meta(
            "Treading Water",
            "Petrol Girls • Ren Aldridge",
            "Talk of Violence",
        );
        remote.album_artist = Some("Petrol Girls".into());
        remote.source = "remote".into();
        remote.path = None;
        remote.remote_id = Some("sub-1".into());

        assert_eq!(
            upsert_track(&db.conn, &remote).unwrap(),
            id,
            "one recording, however each source spells the credit"
        );

        let rows: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1, "the album page must not show the track twice");
    }

    #[test]
    fn test_dedup_without_a_track_number_still_needs_the_artist() {
        let db = test_db();

        // No track number means no position on the release, and the artist is
        // then the only thing separating two different recordings that share a
        // title. Step 4 declines rather than guess.
        let mut local = sample_meta("Untitled", "One", "Split");
        local.album_artist = Some("Various Artists".into());
        local.track_number = None;
        let first = upsert_track(&db.conn, &local).unwrap();

        let mut remote = sample_meta("Untitled", "Two", "Split");
        remote.album_artist = Some("Various Artists".into());
        remote.track_number = None;
        remote.source = "remote".into();
        remote.path = None;
        remote.remote_id = Some("sub-1".into());
        let second = upsert_track(&db.conn, &remote).unwrap();

        assert_ne!(first, second, "different artists, no slot to match on");
    }

    #[test]
    fn test_migration_folds_tracks_split_by_an_artist_credit() {
        let db = test_db();

        // The state an older dedup key left behind: one recording, two rows,
        // because the server names contributors the local tags do not. A sync
        // matches the remote row by its own id, so only the migration can pair
        // them back up.
        let local = sample_meta("Rewild", "Petrol Girls", "Talk of Violence");
        let winner = upsert_track(&db.conn, &local).unwrap();

        let mut remote = sample_meta("Rewild", "Petrol Girls • Ren Aldridge", "Talk of Violence");
        remote.album_artist = Some("Petrol Girls".into());
        remote.source = "remote".into();
        remote.path = None;
        remote.remote_id = Some("sub-9".into());
        db.conn
            .execute(
                "INSERT INTO tracks (album_id, artist_id, disc, track_number, title,
                                     duration_ms, source, remote_id, remote_url)
                 SELECT album_id, artist_id, disc, track_number, title, duration_ms,
                        'remote', 'sub-9', 'http://server/9'
                   FROM tracks WHERE id = ?1",
                params![winner],
            )
            .unwrap();
        let loser: i64 = db.conn.last_insert_rowid();
        assert_ne!(loser, winner);

        merge_split_cross_source_tracks(&db.conn).unwrap();

        let rows: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1, "the pair collapses to one row");

        let (path, remote_id): (Option<String>, Option<String>) = db
            .conn
            .query_row(
                "SELECT path, remote_id FROM tracks WHERE id = ?1",
                params![winner],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert!(path.is_some(), "the local row survives with its file");
        assert_eq!(
            remote_id.as_deref(),
            Some("sub-9"),
            "and inherits how the server knows it"
        );
    }

    #[test]
    fn test_migration_leaves_an_ambiguous_pair_alone() {
        let db = test_db();

        // Two rows that both carry a path are two files, whatever the tags say.
        let first = upsert_track(&db.conn, &sample_meta("Rewild", "A", "Album")).unwrap();
        let mut second = sample_meta("Rewild", "A", "Album");
        second.path = Some("/music/Album/Rewild (alt).flac".into());
        let second = upsert_track(&db.conn, &second).unwrap();
        assert_ne!(first, second);

        merge_split_cross_source_tracks(&db.conn).unwrap();

        let rows: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 2, "two files stay two tracks");
    }

    #[test]
    fn test_upsert_does_not_repoint_at_a_different_live_file() {
        let db = test_db();
        let tmp = tempfile::tempdir().unwrap();
        let existing = tmp.path().join("original.flac");
        std::fs::write(&existing, b"x").unwrap();

        let mut first = sample_meta("Song", "Artist", "Album");
        first.path = Some(existing.to_string_lossy().into_owned());
        let id = upsert_track(&db.conn, &first).unwrap();

        // A remote_id match carrying a different path must not steal the row from
        // a file that is still on disk.
        let mut second = first.clone();
        second.path = Some(tmp.path().join("other.flac").to_string_lossy().into_owned());
        second.remote_id = None;
        db.conn
            .execute(
                "UPDATE tracks SET remote_id = 'r1' WHERE id = ?1",
                params![id],
            )
            .unwrap();
        second.remote_id = Some("r1".into());
        assert_eq!(upsert_track(&db.conn, &second).unwrap(), id);

        let path: String = db
            .conn
            .query_row("SELECT path FROM tracks WHERE id = ?1", params![id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(path, existing.to_string_lossy());
    }

    #[test]
    fn test_stale_removal_clears_all_foreign_keys() {
        let db = test_db();
        let id = upsert_track(&db.conn, &sample_meta("Gone", "Artist", "Album")).unwrap();

        db.conn
            .execute(
                "INSERT INTO lyrics_cache (track_id, source, content, fetched_at)
                 VALUES (?1, 'lrclib', 'la la', 1)",
                params![id],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO play_history (track_id, played_at) VALUES (?1, 1)",
                params![id],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO track_vectors (track_id, embedding) VALUES (?1, x'00')",
                params![id],
            )
            .unwrap();
        crate::db::queries::update_scan_cache(&db.conn, "/music/Album/Gone.flac", 1, 2, id)
            .unwrap();

        assert_eq!(
            remove_stale_tracks(&db.conn, Path::new("/music"), false)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(library_stats(&db.conn).unwrap().total_tracks, 0);
    }

    #[test]
    fn test_stale_removal_survives_orphaned_scan_cache_row() {
        let db = test_db();
        let id = upsert_track(&db.conn, &sample_meta("Gone", "Artist", "Album")).unwrap();

        // A scan_cache row left behind under a path the track no longer has.
        crate::db::queries::update_scan_cache(&db.conn, "/music/Album/old-name.flac", 1, 2, id)
            .unwrap();

        assert_eq!(
            remove_stale_tracks(&db.conn, Path::new("/music"), false)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(library_stats(&db.conn).unwrap().total_tracks, 0);

        let orphans: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM scan_cache", [], |row| row.get(0))
            .unwrap();
        assert_eq!(orphans, 0);
    }

    #[test]
    fn test_stale_removal_ignores_sibling_folder_with_shared_prefix() {
        let db = test_db();

        let mut main = sample_meta("Song", "Artist", "Album");
        main.path = Some("/Volumes/Music/Album/Song.flac".into());
        upsert_track(&db.conn, &main).unwrap();

        let mut backup = sample_meta("Song", "Artist", "Album");
        backup.path = Some("/Volumes/Music Backup/Album/Song.flac".into());
        backup.disc = Some(2);
        upsert_track(&db.conn, &backup).unwrap();
        assert_eq!(library_stats(&db.conn).unwrap().total_tracks, 2);

        // Scanning /Volumes/Music must not reach into /Volumes/Music Backup.
        assert_eq!(
            remove_stale_tracks(&db.conn, Path::new("/Volumes/Music"), false)
                .unwrap()
                .len(),
            1
        );

        let survivor: String = db
            .conn
            .query_row("SELECT path FROM tracks", [], |row| row.get(0))
            .unwrap();
        assert_eq!(survivor, "/Volumes/Music Backup/Album/Song.flac");
    }

    #[test]
    fn test_stale_removal_refuses_wholesale_disappearance() {
        let db = test_db();
        for i in 0..STALE_CHECK_MIN_ROWS + 20 {
            let mut meta = sample_meta(&format!("Track{}", i), "Artist", "Album");
            meta.track_number = Some(i as i32);
            meta.path = Some(format!("/music/Album/{}.flac", i));
            upsert_track(&db.conn, &meta).unwrap();
        }
        let before = library_stats(&db.conn).unwrap().total_tracks;

        let err = remove_stale_tracks(&db.conn, Path::new("/music"), false).unwrap_err();
        assert!(
            matches!(err, DbError::UnsafeBulkDelete(_)),
            "expected refusal, got {:?}",
            err
        );
        assert_eq!(library_stats(&db.conn).unwrap().total_tracks, before);
    }

    #[test]
    fn test_stale_removal_allows_a_normal_deletion() {
        let db = test_db();
        let tmp = tempfile::tempdir().unwrap();
        let folder = tmp.path();

        // 120 tracks on disk, one of them deleted.
        for i in 0..STALE_CHECK_MIN_ROWS + 20 {
            let file = folder.join(format!("{}.flac", i));
            if i > 0 {
                std::fs::write(&file, b"x").unwrap();
            }
            let mut meta = sample_meta(&format!("Track{}", i), "Artist", "Album");
            meta.track_number = Some(i as i32);
            meta.path = Some(file.to_string_lossy().into_owned());
            upsert_track(&db.conn, &meta).unwrap();
        }

        assert_eq!(
            remove_stale_tracks(&db.conn, folder, false).unwrap().len(),
            1
        );
        assert_eq!(
            library_stats(&db.conn).unwrap().total_tracks,
            STALE_CHECK_MIN_ROWS + 19
        );
    }

    #[test]
    fn test_resolve_playback_local_wins() {
        let db = test_db();

        // Insert a local track.
        let local = sample_meta("Song", "Artist", "Album");
        let local_id = upsert_track(&db.conn, &local).unwrap();

        match resolve_playback_path(&db.conn, local_id).unwrap() {
            // Path won't exist on disk in test, so falls through.
            // But we can at least verify it doesn't panic.
            Some(_) | None => {}
        }
    }

    #[test]
    fn test_resolve_playback_remote_fallback() {
        let db = test_db();

        let mut meta = sample_meta("Song", "Artist", "Album");
        meta.source = "remote".into();
        meta.path = None;
        meta.remote_id = Some("r42".into());
        meta.remote_url = Some("https://example.com/stream/r42".into());
        let id = upsert_track(&db.conn, &meta).unwrap();

        let source = resolve_playback_path(&db.conn, id).unwrap().unwrap();
        match source {
            PlaybackSource::Remote(url) => {
                assert!(url.contains("r42"));
            }
            _ => panic!("expected Remote source"),
        }
    }

    #[test]
    fn test_nonexistent_track_resolution() {
        let db = test_db();
        let result = resolve_playback_path(&db.conn, 99999).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_dedup_local_then_remote() {
        let db = test_db();

        // Insert local track first.
        let local = sample_meta("Windowlicker", "Aphex Twin", "Windowlicker EP");
        let local_id = upsert_track(&db.conn, &local).unwrap();

        // Sync same track from remote — should merge, not duplicate.
        let mut remote = sample_meta("Windowlicker", "Aphex Twin", "Windowlicker EP");
        remote.source = "remote".into();
        remote.path = None;
        remote.remote_id = Some("sub-42".into());
        remote.remote_url = Some("https://example.com/stream/sub-42".into());
        let remote_id = upsert_track(&db.conn, &remote).unwrap();

        // Same row.
        assert_eq!(local_id, remote_id);

        // Only 1 track total.
        let stats = library_stats(&db.conn).unwrap();
        assert_eq!(stats.total_tracks, 1);

        // Source should be "local" since it has a path.
        assert_eq!(stats.local_tracks, 1);
        assert_eq!(stats.remote_tracks, 0);

        // But it should have the remote_id merged in.
        let row: (Option<String>, Option<String>, Option<String>) = db
            .conn
            .query_row(
                "SELECT path, remote_id, remote_url FROM tracks WHERE id = ?1",
                params![local_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert!(row.0.is_some()); // local path preserved
        assert_eq!(row.1.as_deref(), Some("sub-42")); // remote_id merged
        assert!(row.2.is_some()); // remote_url merged
    }

    #[test]
    fn test_dedup_remote_then_local() {
        let db = test_db();

        // Insert remote track first.
        let mut remote = sample_meta("Vordhosbn", "Aphex Twin", "Drukqs");
        remote.source = "remote".into();
        remote.path = None;
        remote.remote_id = Some("sub-99".into());
        remote.remote_url = Some("https://example.com/stream/sub-99".into());
        let remote_id = upsert_track(&db.conn, &remote).unwrap();

        // Scan local file — same track, should merge.
        let local = sample_meta("Vordhosbn", "Aphex Twin", "Drukqs");
        let local_id = upsert_track(&db.conn, &local).unwrap();

        // Same row.
        assert_eq!(remote_id, local_id);

        // Only 1 track.
        assert_eq!(library_stats(&db.conn).unwrap().total_tracks, 1);

        // Source flipped to "local" since it now has a path.
        assert_eq!(library_stats(&db.conn).unwrap().local_tracks, 1);

        // Remote info preserved.
        let rid: Option<String> = db
            .conn
            .query_row(
                "SELECT remote_id FROM tracks WHERE id = ?1",
                params![local_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(rid.as_deref(), Some("sub-99"));
    }

    #[test]
    fn test_remove_stale_preserves_remote_backed() {
        let db = test_db();

        // Create a merged local+remote track (path exists in DB but not on disk).
        let mut meta = sample_meta("Ageispolis", "Aphex Twin", "SAW 85-92");
        meta.path = Some("/nonexistent/SAW 85-92/Ageispolis.flac".into());
        meta.remote_id = Some("sub-10".into());
        meta.remote_url = Some("https://example.com/stream/sub-10".into());
        let id = upsert_track(&db.conn, &meta).unwrap();

        // Verify it starts as local.
        let source: String = db
            .conn
            .query_row(
                "SELECT source FROM tracks WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(source, "local");

        // Remove stale tracks in the folder — file doesn't exist on disk.
        let removed =
            remove_stale_tracks(&db.conn, Path::new("/nonexistent/SAW 85-92"), false).unwrap();
        assert_eq!(removed.len(), 1);

        // Track should still exist (not deleted), demoted to remote-only.
        let row: (
            Option<String>,
            String,
            Option<i64>,
            Option<i64>,
            Option<String>,
        ) = db
            .conn
            .query_row(
                "SELECT path, source, mtime, size_bytes, remote_id FROM tracks WHERE id = ?1",
                params![id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
        assert!(row.0.is_none(), "path should be NULL");
        assert_eq!(row.1, "remote", "source should be 'remote'");
        assert!(row.2.is_none(), "mtime should be NULL");
        assert!(row.3.is_none(), "size_bytes should be NULL");
        assert_eq!(row.4.as_deref(), Some("sub-10"), "remote_id preserved");

        // Playback should fall through to remote stream.
        let playback = resolve_playback_path(&db.conn, id).unwrap().unwrap();
        match playback {
            PlaybackSource::Remote(url) => assert!(url.contains("sub-10")),
            _ => panic!("expected Remote playback source"),
        }
    }

    #[test]
    fn test_remove_stale_deletes_pure_local() {
        let db = test_db();

        // Pure local track — no remote_id.
        let meta = sample_meta("PureLocal", "Artist", "Album");
        // sample_meta generates path "/music/Album/PureLocal.flac" which won't exist.
        let id = upsert_track(&db.conn, &meta).unwrap();

        assert_eq!(library_stats(&db.conn).unwrap().total_tracks, 1);

        let removed = remove_stale_tracks(&db.conn, Path::new("/music/Album"), false).unwrap();
        assert_eq!(removed.len(), 1);

        // Track should be fully deleted.
        assert_eq!(library_stats(&db.conn).unwrap().total_tracks, 0);

        // Verify the row is gone.
        let exists: bool = db
            .conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM tracks WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!exists, "pure local track should be deleted");
    }

    #[test]
    fn test_reattach_on_rescan() {
        let db = test_db();

        // Create a merged local+remote track with a non-existent path.
        let mut meta = sample_meta("Xtal", "Aphex Twin", "SAW 85-92");
        meta.path = Some("/nonexistent/SAW 85-92/Xtal.flac".into());
        meta.remote_id = Some("sub-20".into());
        meta.remote_url = Some("https://example.com/stream/sub-20".into());
        let original_id = upsert_track(&db.conn, &meta).unwrap();

        // Simulate stale removal (drive unplugged).
        remove_stale_tracks(&db.conn, Path::new("/nonexistent/SAW 85-92"), false).unwrap();

        // Verify demoted to remote-only.
        let source: String = db
            .conn
            .query_row(
                "SELECT source FROM tracks WHERE id = ?1",
                params![original_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(source, "remote");

        // Simulate re-scan: same track shows up again with a path.
        // upsert_track content match (strategy 3) should re-merge the path.
        let mut rescan = sample_meta("Xtal", "Aphex Twin", "SAW 85-92");
        rescan.path = Some("/nonexistent/SAW 85-92/Xtal.flac".into());
        let rescan_id = upsert_track(&db.conn, &rescan).unwrap();

        // Same row — content match merged it back.
        assert_eq!(original_id, rescan_id);

        // Source should flip back to "local" since it has a path again.
        let row: (Option<String>, String, Option<String>) = db
            .conn
            .query_row(
                "SELECT path, source, remote_id FROM tracks WHERE id = ?1",
                params![rescan_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            row.0.as_deref(),
            Some("/nonexistent/SAW 85-92/Xtal.flac"),
            "path re-attached"
        );
        assert_eq!(row.1, "local", "source flipped back to local");
        assert_eq!(row.2.as_deref(), Some("sub-20"), "remote_id preserved");

        // Only 1 track — no duplication.
        assert_eq!(library_stats(&db.conn).unwrap().total_tracks, 1);
    }

    #[test]
    fn test_genres_by_artist_ids() {
        let db = test_db();
        let mut meta1 = sample_meta("Track1", "ArtistA", "Album1");
        meta1.genre = Some("Rock".into());
        upsert_track(&db.conn, &meta1).unwrap();

        let mut meta2 = sample_meta("Track2", "ArtistA", "Album1");
        meta2.genre = Some("Jazz".into());
        meta2.track_number = Some(2);
        meta2.path = Some("/music/Album1/Track2.flac".into());
        upsert_track(&db.conn, &meta2).unwrap();

        let mut meta3 = sample_meta("Track3", "ArtistB", "Album2");
        meta3.genre = Some("Metal".into());
        upsert_track(&db.conn, &meta3).unwrap();

        // Look up ArtistA's ID.
        let artist_a_id: i64 = db
            .conn
            .query_row("SELECT id FROM artists WHERE name = 'ArtistA'", [], |row| {
                row.get(0)
            })
            .unwrap();
        let artist_b_id: i64 = db
            .conn
            .query_row("SELECT id FROM artists WHERE name = 'ArtistB'", [], |row| {
                row.get(0)
            })
            .unwrap();

        let genres = genres_by_artist_ids(&db.conn, &[artist_a_id, artist_b_id]).unwrap();
        let a_genres = genres.get(&artist_a_id).unwrap();
        assert!(a_genres.contains("rock"));
        assert!(a_genres.contains("jazz"));
        let b_genres = genres.get(&artist_b_id).unwrap();
        assert!(b_genres.contains("metal"));
    }

    #[test]
    fn test_genres_by_artist_ids_empty() {
        let db = test_db();
        let genres = genres_by_artist_ids(&db.conn, &[]).unwrap();
        assert!(genres.is_empty());
    }

    #[test]
    fn test_genres_by_album_ids() {
        let db = test_db();
        let mut meta1 = sample_meta("Track1", "Artist", "AlbumX");
        meta1.genre = Some("Ambient".into());
        upsert_track(&db.conn, &meta1).unwrap();

        let mut meta2 = sample_meta("Track2", "Artist", "AlbumX");
        meta2.genre = Some("IDM".into());
        meta2.track_number = Some(2);
        meta2.path = Some("/music/AlbumX/Track2.flac".into());
        upsert_track(&db.conn, &meta2).unwrap();

        let album_id: i64 = db
            .conn
            .query_row("SELECT id FROM albums WHERE title = 'AlbumX'", [], |row| {
                row.get(0)
            })
            .unwrap();

        let genres = genres_by_album_ids(&db.conn, &[album_id]).unwrap();
        let album_genres = genres.get(&album_id).unwrap();
        assert!(album_genres.contains("ambient"));
        assert!(album_genres.contains("idm"));
    }

    #[test]
    fn test_favourite_artist_ids_batch() {
        let db = test_db();
        let meta = sample_meta("FavTrack", "FavArtist", "FavAlbum");
        upsert_track(&db.conn, &meta).unwrap();

        // Add to favourites.
        crate::db::queries::add_favourite(
            &db.conn,
            std::path::Path::new("/music/FavAlbum/FavTrack.flac"),
        )
        .unwrap();

        let artist_id: i64 = db
            .conn
            .query_row(
                "SELECT id FROM artists WHERE name = 'FavArtist'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        let fav_ids = favourite_artist_ids_batch(&db.conn).unwrap();
        assert!(fav_ids.contains(&artist_id));
    }

    #[test]
    fn test_favourite_artist_ids_batch_empty() {
        let db = test_db();
        let fav_ids = favourite_artist_ids_batch(&db.conn).unwrap();
        assert!(fav_ids.is_empty());
    }

    #[test]
    fn test_favourite_album_ids_batch() {
        let db = test_db();
        let meta = sample_meta("FavTrack", "FavArtist", "FavAlbum");
        upsert_track(&db.conn, &meta).unwrap();

        crate::db::queries::add_favourite(
            &db.conn,
            std::path::Path::new("/music/FavAlbum/FavTrack.flac"),
        )
        .unwrap();

        let album_id: i64 = db
            .conn
            .query_row(
                "SELECT id FROM albums WHERE title = 'FavAlbum'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        let fav_ids = favourite_album_ids_batch(&db.conn).unwrap();
        assert!(fav_ids.contains(&album_id));
    }

    #[test]
    fn test_favourite_album_ids_batch_empty() {
        let db = test_db();
        let fav_ids = favourite_album_ids_batch(&db.conn).unwrap();
        assert!(fav_ids.is_empty());
    }

    #[test]
    fn test_set_cached_path_records_size_and_date() {
        let db = test_db();
        let mut meta = sample_meta("Song", "Artist", "Album");
        meta.source = "remote".into();
        meta.path = None;
        meta.remote_id = Some("r1".into());
        meta.remote_url = Some("https://example.com/r1".into());
        let id = upsert_track(&db.conn, &meta).unwrap();

        // Create a temp file to simulate a cached download.
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut tmp.as_file().try_clone().unwrap(), &[0u8; 1024]).unwrap();
        let path = tmp.path().to_string_lossy().to_string();

        set_cached_path(&db.conn, id, &path).unwrap();

        let (cached_path, size, download_date): (Option<String>, Option<i64>, Option<i64>) = db
            .conn
            .query_row(
                "SELECT cached_path, cache_size_bytes, cache_download_date FROM tracks WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();

        assert_eq!(cached_path.as_deref(), Some(path.as_str()));
        assert!(size.unwrap() > 0, "cache_size_bytes should be positive");
        assert!(
            download_date.unwrap() > 0,
            "cache_download_date should be set"
        );
    }

    #[test]
    fn test_total_cache_size() {
        let db = test_db();

        // Start with zero.
        assert_eq!(total_cache_size(&db.conn).unwrap(), 0);

        // Insert a cached track with known size.
        let mut meta = sample_meta("Song", "Artist", "Album");
        meta.source = "remote".into();
        meta.path = None;
        meta.remote_id = Some("r1".into());
        let id = upsert_track(&db.conn, &meta).unwrap();

        db.conn
            .execute(
                "UPDATE tracks SET cached_path = '/cache/song.flac', cache_size_bytes = 50000000 WHERE id = ?1",
                params![id],
            )
            .unwrap();

        assert_eq!(total_cache_size(&db.conn).unwrap(), 50_000_000);
    }

    #[test]
    fn test_clear_cache_for_tracks() {
        let db = test_db();
        let mut meta = sample_meta("Song", "Artist", "Album");
        meta.source = "remote".into();
        meta.path = None;
        meta.remote_id = Some("r1".into());
        let id = upsert_track(&db.conn, &meta).unwrap();

        db.conn
            .execute(
                "UPDATE tracks SET cached_path = '/cache/song.flac', cache_size_bytes = 1000, cache_download_date = 12345 WHERE id = ?1",
                params![id],
            )
            .unwrap();

        clear_cache_for_tracks(&db.conn, &[id]).unwrap();

        let (path, size, date): (Option<String>, Option<i64>, Option<i64>) = db
            .conn
            .query_row(
                "SELECT cached_path, cache_size_bytes, cache_download_date FROM tracks WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();

        assert!(path.is_none());
        assert!(size.is_none());
        assert!(date.is_none());
    }

    #[test]
    fn test_cached_albums_lru_excludes_favourites() {
        let db = test_db();

        // Create two remote-cached albums.
        for (album, tracks) in &[("AlbumA", vec!["T1", "T2"]), ("AlbumB", vec!["T3", "T4"])] {
            for (i, title) in tracks.iter().enumerate() {
                let mut meta = sample_meta(title, "Artist", album);
                meta.source = "remote".into();
                meta.path = None;
                meta.remote_id = Some(format!("r-{}", title));
                meta.track_number = Some((i + 1) as i32);
                let id = upsert_track(&db.conn, &meta).unwrap();
                let cached = format!("/cache/{}/{}.flac", album, title);
                db.conn
                    .execute(
                        "UPDATE tracks SET cached_path = ?1, cache_size_bytes = 10000000 WHERE id = ?2",
                        params![cached, id],
                    )
                    .unwrap();
            }
        }

        // Favourite a track from AlbumB.
        crate::db::queries::add_favourite(&db.conn, std::path::Path::new("/cache/AlbumB/T3.flac"))
            .unwrap();

        let albums = cached_albums_lru(&db.conn).unwrap();

        // AlbumB should be excluded (has a favourite), only AlbumA returned.
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].album_title, "AlbumA");
    }

    #[test]
    fn test_cached_albums_lru_sorted_by_last_play() {
        let db = test_db();

        // Create two cached albums.
        let mut album_ids = Vec::new();
        for (album, played_at) in &[("OldAlbum", 1000), ("NewAlbum", 9000)] {
            let mut meta = sample_meta("Track", "Artist", album);
            meta.source = "remote".into();
            meta.path = None;
            meta.remote_id = Some(format!("r-{}", album));
            let id = upsert_track(&db.conn, &meta).unwrap();
            let cached = format!("/cache/{}/Track.flac", album);
            db.conn
                .execute(
                    "UPDATE tracks SET cached_path = ?1, cache_size_bytes = 10000000 WHERE id = ?2",
                    params![cached, id],
                )
                .unwrap();

            // Record play history.
            db.conn
                .execute(
                    "INSERT INTO play_history (track_id, played_at) VALUES (?1, ?2)",
                    params![id, played_at],
                )
                .unwrap();

            album_ids.push(id);
        }

        let albums = cached_albums_lru(&db.conn).unwrap();
        assert_eq!(albums.len(), 2);
        // OldAlbum (played_at=1000) should come first (evicted first).
        assert_eq!(albums[0].album_title, "OldAlbum");
        assert_eq!(albums[1].album_title, "NewAlbum");
    }

    #[test]
    fn test_clear_cached_paths_clears_all_tracking() {
        let db = test_db();
        let mut meta = sample_meta("Song", "Artist", "Album");
        meta.source = "remote".into();
        meta.path = None;
        meta.remote_id = Some("r1".into());
        let id = upsert_track(&db.conn, &meta).unwrap();

        db.conn
            .execute(
                "UPDATE tracks SET cached_path = '/x', cache_size_bytes = 100, cache_download_date = 999 WHERE id = ?1",
                params![id],
            )
            .unwrap();

        clear_cached_paths(&db.conn).unwrap();

        let (path, size, date): (Option<String>, Option<i64>, Option<i64>) = db
            .conn
            .query_row(
                "SELECT cached_path, cache_size_bytes, cache_download_date FROM tracks WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();

        assert!(path.is_none());
        assert!(size.is_none());
        assert!(date.is_none());
    }
}
