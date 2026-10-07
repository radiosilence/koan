use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::connection::DbError;

use super::sources;
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

/// Insert or update a track from what one source says about it.
///
/// A `TrackMeta` with a path is a file and one with a server id an entry on
/// the server; one with both is both. Each source keeps its own tags, and the
/// track's columns are derived from its sources, the file's first — see
/// `sources`. Which track a source belongs to is decided in one place,
/// `sources::link`: the same MusicBrainz recording on the same release, or the
/// same album, album artist, disc, number and title with the artist as a
/// tie-break. Two sources of the same kind are never one track, and a match
/// that is ambiguous is declined.
pub fn upsert_track(conn: &Connection, meta: &TrackMeta) -> Result<i64, DbError> {
    upsert_track_status(conn, meta).map(|(id, _)| id)
}

/// `upsert_track`, additionally reporting whether a new row was inserted (`true`)
/// or an existing one updated (`false`).
pub fn upsert_track_status(conn: &Connection, meta: &TrackMeta) -> Result<(i64, bool), DbError> {
    upsert_track_with(conn, meta, None)
}

/// `upsert_track` for a remote sync, which knows the server ids it has seen so
/// far. An entry in the same slot whose id the sync has not seen is taken to be
/// this one under the id it had before: a server that renumbers its library
/// keeps its rows, history and favourites rather than gaining a second copy of
/// every track. An id already seen belongs to an entry of its own, so two
/// entries the server lists with identical tags stay two rows.
pub fn upsert_synced_track(
    conn: &Connection,
    meta: &TrackMeta,
    seen: &HashSet<String>,
) -> Result<i64, DbError> {
    upsert_track_with(conn, meta, Some(seen)).map(|(id, _)| id)
}

fn upsert_track_with(
    conn: &Connection,
    meta: &TrackMeta,
    seen: Option<&HashSet<String>>,
) -> Result<(i64, bool), DbError> {
    // Use a savepoint so this works both standalone and inside an existing
    // transaction (e.g. the chunk transactions in scan_folder).
    conn.execute_batch("SAVEPOINT upsert_track")?;
    let result = (|| {
        let mut recorded = None;
        if meta.path.is_some() {
            recorded = Some(sources::record(conn, sources::Kind::Local, meta, None)?);
        }
        if meta.remote_id.is_some() {
            let (track, inserted) = sources::record(conn, sources::Kind::Remote, meta, seen)?;
            recorded = Some((track, recorded.is_some_and(|(_, i)| i) || inserted));
        }
        recorded.ok_or(DbError::NoSource)
    })();
    match &result {
        Ok(_) => conn.execute_batch("RELEASE upsert_track")?,
        Err(_) => conn.execute_batch("ROLLBACK TO upsert_track; RELEASE upsert_track")?,
    }
    result
}

/// Clear the disc numbers stored as 0, which matching reads as none. Run
/// before the source rows are built, so their keys agree.
pub(crate) fn clear_zero_discs(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute("UPDATE tracks SET disc = NULL WHERE disc = 0", [])?;
    Ok(())
}

/// Drop the remote entries the server no longer lists, keeping each track for
/// whatever else it has: a file keeps its row and only loses the server's copy,
/// and a server-only track whose entry the server renumbered goes to the live
/// entry in its slot, history and favourites with it. The rest are deleted
/// with their downloads.
///
/// An entry is gone when its id is not in `live_tracks`, or its album's id is
/// not in `live_albums`. Either may be `None` when the sync cannot vouch for
/// it: only a sync that listed everything has seen every id the server knows.
pub fn remove_vanished_remote(
    conn: &Connection,
    live_tracks: Option<&HashSet<String>>,
    live_albums: Option<&HashSet<String>>,
) -> Result<usize, DbError> {
    conn.execute_batch(
        "SAVEPOINT remove_vanished;
         CREATE TEMP TABLE IF NOT EXISTS live_ids (kind TEXT, id TEXT, PRIMARY KEY (kind, id));
         DELETE FROM temp.live_ids;",
    )?;
    let removed = (|| {
        let mut insert =
            conn.prepare("INSERT OR IGNORE INTO temp.live_ids (kind, id) VALUES (?1, ?2)")?;
        for (kind, ids) in [("track", live_tracks), ("album", live_albums)] {
            for id in ids.into_iter().flatten() {
                insert.execute(params![kind, id])?;
            }
        }
        let track_gone = if live_tracks.is_some() {
            "remote_id NOT IN (SELECT id FROM temp.live_ids WHERE kind = 'track')"
        } else {
            "0"
        };
        let album_gone = if live_albums.is_some() {
            "album_remote_id IS NOT NULL
             AND album_remote_id NOT IN (SELECT id FROM temp.live_ids WHERE kind = 'album')"
        } else {
            "0"
        };
        let gone: Vec<String> = conn
            .prepare(&format!(
                "SELECT remote_id FROM remote_entries WHERE {track_gone} OR {album_gone}"
            ))?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let mut downloads = sources::remove_vanished(conn, &gone, live_tracks.is_some())?;
        // Tracks a rebuilt index has not yet re-read, whose entry this sync
        // did not claim again.
        if live_tracks.is_some() {
            let unread: Vec<String> = conn
                .prepare(
                    "SELECT t.remote_id FROM tracks t WHERE t.remote_id IS NOT NULL
                        AND NOT EXISTS (SELECT 1 FROM remote_entries r WHERE r.track_id = t.id)
                        AND t.remote_id NOT IN (SELECT id FROM temp.live_ids WHERE kind = 'track')",
                )?
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            for key in &unread {
                downloads.extend(sources::forget_unread(conn, sources::Kind::Remote, key)?);
            }
        }
        Ok::<_, DbError>((gone.len(), downloads))
    })();
    match removed {
        Ok((n, downloads)) => {
            conn.execute_batch("RELEASE remove_vanished")?;
            // A downloaded copy goes with its track; nothing would ever play
            // or clean it up otherwise.
            let mut freed = 0;
            for path in downloads {
                let size = std::fs::metadata(&path).map_or(0, |m| m.len());
                if std::fs::remove_file(&path).is_ok() {
                    freed += size;
                }
            }
            if freed > 0 {
                crate::helpers::cache_shrank(freed);
            }
            Ok(n)
        }
        Err(e) => {
            conn.execute_batch("ROLLBACK TO remove_vanished; RELEASE remove_vanished")?;
            Err(e)
        }
    }
}

/// Fold together the rows one file got by being spelled two ways.
///
/// A drop from Finder used to index a file under the precomposed spelling
/// Foundation hands over, and the next scan stored the same file again under
/// the directory entry's own, decomposed bytes — the two open the same file on
/// a Mac, and `tracks.path` is compared bytewise. Paths are resolved against
/// the directory on the way in now, so only this can bring the pairs it left
/// back together.
///
/// The older row wins: it carries the play history, and the sync link if a
/// server has the recording. It takes the decomposed spelling, which is the one
/// a scan wrote — Foundation never produces a decomposed path, so that row is
/// the one that came from the directory. A pair with no such spelling, or a
/// path with more than two, is left visible rather than guessed at. The scan
/// cache is keyed by path, so it moves by name.
pub(crate) fn merge_spelling_twins(conn: &Connection) -> rusqlite::Result<()> {
    use unicode_normalization::{UnicodeNormalization, is_nfd};

    let rows: Vec<(i64, String)> = {
        let mut stmt = conn.prepare("SELECT id, path FROM tracks WHERE path IS NOT NULL")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut by_spelling: HashMap<String, Vec<(i64, String)>> = HashMap::new();
    for (id, path) in rows.into_iter().filter(|(_, path)| !path.is_ascii()) {
        by_spelling
            .entry(path.nfc().collect())
            .or_default()
            .push((id, path));
    }

    for mut pair in by_spelling.into_values().filter(|group| group.len() == 2) {
        pair.sort_by_key(|(id, _)| *id);
        let (winner, winner_path) = &pair[0];
        let (loser, loser_path) = &pair[1];
        let Some(disk) = [winner_path, loser_path]
            .into_iter()
            .find(|path| is_nfd(path))
        else {
            continue;
        };
        let disk = disk.clone();
        let stale = if winner_path == &disk {
            loser_path
        } else {
            winner_path
        }
        .clone();

        conn.execute(
            "UPDATE tracks SET
                 remote_id = COALESCE(remote_id, (SELECT remote_id FROM tracks WHERE id = ?2)),
                 remote_url = COALESCE(remote_url, (SELECT remote_url FROM tracks WHERE id = ?2)),
                 cached_path = COALESCE(cached_path, (SELECT cached_path FROM tracks WHERE id = ?2)),
                 cache_size_bytes = COALESCE(cache_size_bytes, (SELECT cache_size_bytes FROM tracks WHERE id = ?2)),
                 cache_download_date = COALESCE(cache_download_date, (SELECT cache_download_date FROM tracks WHERE id = ?2)),
                 genre = COALESCE(genre, (SELECT genre FROM tracks WHERE id = ?2)),
                 mbid = COALESCE(mbid, (SELECT mbid FROM tracks WHERE id = ?2))
               WHERE id = ?1",
            params![winner, loser],
        )?;
        sources::fold_rows(conn, *winner, *loser)?;
        conn.execute("DELETE FROM scan_cache WHERE path = ?1", params![stale])?;
        conn.execute(
            "UPDATE tracks SET path = ?1 WHERE id = ?2",
            params![disk, winner],
        )?;
    }

    Ok(())
}

/// Drop an album or artist the last track just left. Correcting a tag moves a row
/// to a different album, and the one it came from is usually a misreading nobody
/// wants left in the browser looking like a record with nothing on it.
pub(crate) fn prune_if_empty(
    conn: &Connection,
    album_id: Option<i64>,
    artist_id: Option<i64>,
) -> rusqlite::Result<()> {
    if let Some(album_id) = album_id {
        let emptied = conn.execute(
            "DELETE FROM albums WHERE id = ?1
               AND NOT EXISTS (SELECT 1 FROM tracks WHERE album_id = ?1)",
            params![album_id],
        )?;
        if emptied > 0 {
            conn.execute(
                "DELETE FROM favourite_albums WHERE album_id = ?1",
                params![album_id],
            )?;
            conn.execute(
                "DELETE FROM album_ratings WHERE album_id = ?1",
                params![album_id],
            )?;
        }
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
                "DELETE FROM favourite_artists WHERE artist_id = ?1",
                params![artist_id],
            )?;
            conn.execute(
                "DELETE FROM artist_ratings WHERE artist_id = ?1",
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

/// Tell the apps' cover caches to forget what they hold for the albums and
/// tracks under `folder`, whose files a rescan has just looked at: a cover
/// image beside them may have been added, replaced or removed, and nothing
/// in their rows says so. Returns how many records were named.
pub fn evict_art_under(conn: &Connection, folder: &Path) -> Result<usize, DbError> {
    let (lower, upper) = super::folder_prefix_range(folder);
    Ok(conn.execute(
        "INSERT INTO art_evictions (kind, id)
         SELECT 'album', album_id FROM tracks
          WHERE path >= ?1 AND path < ?2 AND album_id IS NOT NULL
          GROUP BY album_id
         UNION ALL
         SELECT 'track', id FROM tracks WHERE path >= ?1 AND path < ?2",
        params![lower, upper],
    )?)
}

/// Forget the files under `folder` that no longer exist.
///
/// A track the server also has keeps its row and streams from there; one that
/// was only the file is deleted, with its play history and lyrics. A server
/// entry freed this way pairs with a file that turns up under another path —
/// the same file moved. A folder that is present but unreadable must never look
/// like a folder whose files were deleted, and two brakes enforce that: an IO
/// error is not read as "gone", and a run that would clear more than
/// [`MAX_STALE_FRACTION`] of a folder holding at least [`STALE_CHECK_MIN_ROWS`]
/// tracks is refused with [`DbError::UnsafeBulkDelete`].
///
/// `force_remove` lifts the second brake only, for the case where the files really
/// were deleted. The IO-error and dangling-symlink checks still apply (see
/// [`crate::index::known_missing`]), and the caller is still
/// responsible for not calling this at all when the folder yielded no files.
///
/// Returns the paths removed or demoted, so a caller can show what it did.
pub fn remove_stale_tracks(
    conn: &Connection,
    folder: &Path,
    force_remove: bool,
) -> Result<Vec<String>, DbError> {
    remove_stale_tracks_walked(conn, folder, force_remove, None)
}

/// [`remove_stale_tracks`], after a walk of `folder` that found `walked`: a
/// row at one of those paths is there, and only the rest are asked of the
/// filesystem. A walk sees everything a stat would, so on a network mount
/// this saves a round trip per file in the library; what the walk did not
/// find is still confirmed missing before it goes, so an unmounted share is
/// still kept.
pub fn remove_stale_tracks_walked(
    conn: &Connection,
    folder: &Path,
    force_remove: bool,
    walked: Option<&std::collections::HashSet<String>>,
) -> Result<Vec<String>, DbError> {
    let (lower, upper) = super::folder_prefix_range(folder);

    // Files, and the tracks a rebuilt index has not yet re-read: a track
    // whose file the scan did not claim again.
    let paths: Vec<String> = conn
        .prepare(
            "SELECT path FROM local_files WHERE path >= ?1 AND path < ?2
             UNION
             SELECT t.path FROM tracks t WHERE t.path >= ?1 AND t.path < ?2
                AND NOT EXISTS (SELECT 1 FROM local_files f WHERE f.track_id = t.id)",
        )?
        .query_map(params![lower, upper], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let total = paths.len() as i64;
    let unwalked = paths
        .into_iter()
        .filter(|path| !walked.is_some_and(|w| w.contains(path)));
    // A row the walk did not find under its own spelling but did under the
    // directory's is the same file stored twice; see `sources::fold_twin`.
    let mut spelling = crate::index::spelling::Spelling::default();
    let mut unclaimed = Vec::new();
    for path in unwalked {
        let spelled = walked.and_then(|w| {
            let on_disk = spelling.on_disk(Path::new(&path));
            let on_disk = on_disk.to_string_lossy();
            (on_disk != path.as_str() && w.contains(on_disk.as_ref())).then(|| on_disk.into_owned())
        });
        match spelled {
            Some(on_disk) if sources::fold_twin(conn, &path, &on_disk)? => {}
            _ => unclaimed.push(path),
        }
    }
    let stale: Vec<String> = unclaimed
        .into_iter()
        // A permission error, an ailing mount or a symlink whose target has
        // gone away is "cannot tell", not "deleted".
        .filter(|path| crate::index::known_missing(Path::new(path)))
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

    let mut tracks = Vec::with_capacity(stale.len());
    for path in &stale {
        tracks.extend(sources::track_of_file(conn, path)?);
    }
    sources::forget_tracks(conn, &tracks, sources::Forget::Demote)?;

    Ok(stale)
}

/// Give files gone from under `folder` their tracks back where a scan has
/// just found them at new paths: `arrived` is the tracks it made. See
/// `sources::adopt_moved`. Returns how many were given back.
pub fn adopt_moved_files(
    conn: &Connection,
    folder: &Path,
    arrived: &[i64],
    walked: Option<&std::collections::HashSet<String>>,
) -> Result<usize, DbError> {
    if arrived.is_empty() {
        return Ok(0);
    }
    let (lower, upper) = super::folder_prefix_range(folder);
    let paths: Vec<String> = conn
        .prepare("SELECT path FROM local_files WHERE path >= ?1 AND path < ?2")?
        .query_map(params![lower, upper], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut adopted = 0;
    // A file the walk found has not moved.
    for path in paths
        .into_iter()
        .filter(|path| !walked.is_some_and(|w| w.contains(path)))
    {
        if crate::index::known_missing(Path::new(&path))
            && sources::adopt_moved(conn, &path, arrived)?
        {
            adopted += 1;
        }
    }
    Ok(adopted)
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
    random_tracks_where(
        conn,
        count,
        &RandomFilter {
            artist_id,
            ..Default::default()
        },
    )
}

/// What `random_tracks_where` draws from.
#[derive(Debug, Clone, Copy, Default)]
pub struct RandomFilter<'a> {
    /// Tracks credited to this artist or on their albums.
    pub artist_id: Option<i64>,
    /// Matched case-insensitively against the track's own tag.
    pub genre: Option<&'a str>,
    /// Release year bounds of the track's album, inclusive. Tracks without a
    /// dated album are left out when either is set.
    pub year_from: Option<i32>,
    pub year_to: Option<i32>,
    /// Track ids never drawn.
    pub exclude: &'a [i64],
}

/// `count` random tracks matching `filter`.
///
/// The draw orders bare ids and the joins run for the picked rows only, so a
/// handful from a large library does not join every track to sort it.
pub fn random_tracks_where(
    conn: &Connection,
    count: u32,
    filter: &RandomFilter,
) -> Result<Vec<TrackRow>, DbError> {
    let mut wheres: Vec<String> = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    if let Some(aid) = filter.artist_id {
        wheres.push(
            "(r.artist_id = ? OR r.album_id IN (SELECT id FROM albums WHERE artist_id = ?))".into(),
        );
        params.push(Box::new(aid));
        params.push(Box::new(aid));
    }
    if let Some(genre) = filter.genre {
        wheres.push("r.genre = ? COLLATE NOCASE".into());
        params.push(Box::new(genre.to_owned()));
    }
    let year =
        "(SELECT CAST(substr(ra.date, 1, 4) AS INTEGER) FROM albums ra WHERE ra.id = r.album_id)";
    if let Some(from) = filter.year_from {
        wheres.push(format!("{year} >= ?"));
        params.push(Box::new(from));
    }
    if let Some(to) = filter.year_to {
        wheres.push(format!("{year} <= ?"));
        params.push(Box::new(to));
    }
    if !filter.exclude.is_empty() {
        wheres.push("r.id NOT IN (SELECT value FROM json_each(?))".into());
        params.push(Box::new(super::json_list(filter.exclude)));
    }
    params.push(Box::new(count));
    let draw = if wheres.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", wheres.join(" AND "))
    };
    let sql = format!(
        "SELECT t.id, t.album_id, t.artist_id, a.name, aa.name, al.title,
                t.disc, t.track_number, t.title, t.duration_ms, t.path,
                t.codec, t.sample_rate, t.bit_depth, t.channels, t.bitrate,
                t.genre, t.source, t.remote_id, t.cached_path
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         LEFT JOIN artists aa ON al.artist_id = aa.id
         WHERE t.id IN (SELECT r.id FROM tracks r {draw} ORDER BY RANDOM() LIMIT ?)
         ORDER BY RANDOM()"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(params.iter()), row_to_track_row)?
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

/// Fetch many tracks in one query, in the order the ids were given, rather
/// than one round trip per track.
pub fn tracks_by_ids(conn: &Connection, ids: &[i64]) -> Result<Vec<TrackRow>, DbError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare_cached(
        "SELECT t.id, t.album_id, t.artist_id, a.name, aa.name, al.title,
                t.disc, t.track_number, t.title, t.duration_ms, t.path,
                t.codec, t.sample_rate, t.bit_depth, t.channels, t.bitrate,
                t.genre, t.source, t.remote_id, t.cached_path
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         LEFT JOIN artists aa ON al.artist_id = aa.id
         WHERE t.id IN (SELECT value FROM json_each(?1))",
    )?;
    let rows = stmt
        .query_map([super::json_list(ids)], row_to_track_row)?
        .collect::<Result<Vec<_>, _>>()?;

    // SQL returns them in whatever order it likes; callers care about the order
    // they asked for, because that is the order they will be queued in.
    //
    // Looked up rather than taken: an id asked for twice must come back twice,
    // or a track queued again, or a playlist holding the same song twice, loses
    // its second copy.
    let by_id: HashMap<i64, TrackRow> = rows.into_iter().map(|r| (r.id, r)).collect();
    Ok(ids.iter().filter_map(|id| by_id.get(id).cloned()).collect())
}

/// Get a single track by ID with full metadata.
pub fn get_track_row(conn: &Connection, track_id: i64) -> Result<Option<TrackRow>, DbError> {
    let result = conn
        .prepare_cached(
            "SELECT t.id, t.album_id, t.artist_id, a.name, aa.name, al.title,
                t.disc, t.track_number, t.title, t.duration_ms, t.path,
                t.codec, t.sample_rate, t.bit_depth, t.channels, t.bitrate,
                t.genre, t.source, t.remote_id, t.cached_path
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         LEFT JOIN artists aa ON al.artist_id = aa.id
         WHERE t.id = ?1",
        )?
        .query_row(params![track_id], row_to_track_row);

    match result {
        Ok(row) => Ok(Some(row)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Look up a track ID by its local file path.
pub fn track_id_by_path(conn: &Connection, path: &str) -> Result<Option<i64>, DbError> {
    let result = conn
        .prepare_cached(
            "SELECT id FROM tracks WHERE path = ?1 OR cached_path = ?1 OR remote_url = ?1",
        )?
        .query_row(params![path], |row| row.get(0));
    match result {
        Ok(id) => Ok(Some(id)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Clear all cached_path values (used when purging the download cache).
pub fn clear_cached_paths(conn: &Connection) -> Result<(), DbError> {
    // Only the rows that hold a download: an unqualified UPDATE rewrites every
    // track in the library through the WAL.
    conn.execute(
        "UPDATE tracks SET cached_path = NULL, cache_size_bytes = NULL, cache_download_date = NULL,
                cache_pinned = 0
         WHERE cached_path IS NOT NULL",
        params![],
    )?;
    Ok(())
}

/// Where the named tracks were downloaded to, for the ones that were.
pub fn cached_paths_for(conn: &Connection, track_ids: &[i64]) -> Result<Vec<String>, DbError> {
    if track_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare_cached(
        "SELECT cached_path FROM tracks
         WHERE id IN (SELECT value FROM json_each(?1)) AND cached_path IS NOT NULL",
    )?;
    let rows = stmt.query_map([super::json_list(track_ids)], |row| row.get(0))?;
    Ok(rows.filter_map(Result::ok).collect())
}

/// Forget where the named tracks were downloaded to. The rows stay: a remote
/// track is still in the library, it just has to be fetched again to play.
pub fn clear_cached_paths_for(conn: &Connection, track_ids: &[i64]) -> Result<(), DbError> {
    if track_ids.is_empty() {
        return Ok(());
    }
    conn.execute(
        "UPDATE tracks SET cached_path = NULL, cache_size_bytes = NULL, cache_download_date = NULL,
                cache_pinned = 0
         WHERE id IN (SELECT value FROM json_each(?1))",
        [super::json_list(track_ids)],
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

/// Mark these tracks' downloads as asked for, so eviction takes them last.
/// Tracks with no download are left alone.
pub fn pin_cached(conn: &Connection, track_ids: &[i64]) -> Result<(), DbError> {
    if track_ids.is_empty() {
        return Ok(());
    }
    conn.execute(
        "UPDATE tracks SET cache_pinned = 1
         WHERE id IN (SELECT value FROM json_each(?1)) AND cached_path IS NOT NULL",
        [super::json_list(track_ids)],
    )?;
    Ok(())
}

/// One downloaded file eviction may remove.
#[derive(Debug, Clone)]
pub struct CachedFile {
    pub track_id: i64,
    pub path: String,
    pub size: i64,
    pub pinned: bool,
}

/// Downloaded files in the order eviction takes them: everything fetched to
/// play before anything downloaded on request, least recently used first
/// within each. A file whose album has a favourite in it is never listed.
///
/// A download counts as a use: a record fetched for offline listening has
/// never been played, and ranking it by plays alone would make it the first to go.
pub fn cached_files_lru(conn: &Connection) -> Result<Vec<CachedFile>, DbError> {
    // play_history is pre-aggregated rather than queried per track. Rows with
    // no recorded use sort first (NULL before any value).
    let mut stmt = conn.prepare(
        "SELECT t.id, t.cached_path, COALESCE(t.cache_size_bytes, 0), t.cache_pinned != 0
         FROM tracks t
         LEFT JOIN (SELECT track_id, MAX(played_at) as last_play
                    FROM play_history GROUP BY track_id) ph_max
                ON ph_max.track_id = t.id
         WHERE t.cached_path IS NOT NULL
           AND NOT EXISTS (SELECT 1 FROM favourites f JOIN tracks ft ON ft.id = f.track_id
                           WHERE ft.id = t.id OR ft.album_id = t.album_id)
         ORDER BY t.cache_pinned != 0,
                  NULLIF(MAX(COALESCE(ph_max.last_play, 0), COALESCE(t.cache_download_date, 0)), 0),
                  t.album_id, t.disc, t.track_number",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(CachedFile {
            track_id: row.get(0)?,
            path: row.get(1)?,
            size: row.get(2)?,
            pinned: row.get(3)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// What downloading each of these tracks would add to the cache, for those
/// not already in it. Tracks with a library file or no server entry cost
/// nothing. The server rarely says how large a file is, so the size is
/// reckoned from its bitrate and length, or from length at CD rate when the
/// bitrate is missing too.
pub fn download_estimates(
    conn: &Connection,
    track_ids: &[i64],
) -> Result<HashMap<i64, i64>, DbError> {
    if track_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let mut stmt = conn.prepare_cached(
        "SELECT t.id,
                CASE WHEN EXISTS(SELECT 1 FROM local_files l WHERE l.track_id = t.id) THEN 0
                     ELSE COALESCE(r.size_bytes, r.bitrate * r.duration_ms / 8,
                                   r.duration_ms * 1411 / 8, 0)
                END
         FROM tracks t
         JOIN remote_entries r ON r.track_id = t.id
         WHERE t.id IN (SELECT value FROM json_each(?1)) AND t.cached_path IS NULL",
    )?;
    let rows = stmt.query_map([super::json_list(track_ids)], |r| {
        Ok((r.get(0)?, r.get(1)?))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
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

/// Resolve the best playback source for a track. Local > Cached > Remote.
pub fn resolve_playback_path(
    conn: &Connection,
    track_id: i64,
) -> Result<Option<PlaybackSource>, DbError> {
    let row = conn
        .prepare_cached("SELECT path, cached_path, remote_url FROM tracks WHERE id = ?1")?
        .query_row(params![track_id], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .optional()?;
    Ok(row.and_then(|(path, cached_path, remote_url)| {
        choose_playback_source(
            path.as_deref(),
            cached_path.as_deref(),
            remote_url.as_deref(),
        )
    }))
}

/// The first of a track's copies that is there: its file, then its download,
/// then its stream.
pub fn choose_playback_source(
    path: Option<&str>,
    cached_path: Option<&str>,
    remote_url: Option<&str>,
) -> Option<PlaybackSource> {
    if let Some(p) = path.map(PathBuf::from).filter(|p| p.exists()) {
        return Some(PlaybackSource::Local(p));
    }
    if let Some(p) = cached_path.map(PathBuf::from).filter(|p| p.exists()) {
        return Some(PlaybackSource::Cached(p));
    }
    remote_url.map(|url| PlaybackSource::Remote(url.to_owned()))
}

/// What a queue item needs that a `TrackRow` does not carry.
#[derive(Debug, Clone, Default)]
pub struct QueueItemExtras {
    pub remote_url: Option<String>,
    pub album_date: Option<String>,
}

/// [`QueueItemExtras`] for many tracks, in one query, keyed by track id.
pub fn queue_item_extras(
    conn: &Connection,
    track_ids: &[i64],
) -> Result<HashMap<i64, QueueItemExtras>, DbError> {
    if track_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let mut stmt = conn.prepare_cached(
        "SELECT t.id, t.remote_url, al.date FROM tracks t
         LEFT JOIN albums al ON al.id = t.album_id
         WHERE t.id IN (SELECT value FROM json_each(?1))",
    )?;
    let rows = stmt.query_map([super::json_list(track_ids)], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            QueueItemExtras {
                remote_url: row.get(1)?,
                album_date: row.get(2)?,
            },
        ))
    })?;
    rows.collect::<Result<HashMap<_, _>, _>>()
        .map_err(Into::into)
}

/// One page of every track, in id order.
///
/// What a client walking the whole library pages through. Ordered by the
/// primary key, so the page is a range scan rather than a sort, and a track
/// added mid-walk lands after the offset rather than shifting every page past
/// it.
pub fn tracks_page(conn: &Connection, limit: u32, offset: u32) -> Result<Vec<TrackRow>, DbError> {
    let mut stmt = conn.prepare_cached(
        "SELECT t.id, t.album_id, t.artist_id, a.name, aa.name, al.title,
                t.disc, t.track_number, t.title, t.duration_ms, t.path,
                t.codec, t.sample_rate, t.bit_depth, t.channels, t.bitrate,
                t.genre, t.source, t.remote_id, t.cached_path
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         LEFT JOIN artists aa ON al.artist_id = aa.id
         ORDER BY t.id
         LIMIT ?1 OFFSET ?2",
    )?;
    let rows = stmt
        .query_map(params![limit, offset], row_to_track_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Get tracks for a specific album, ordered by disc/track number.
pub fn tracks_for_album(conn: &Connection, album_id: i64) -> Result<Vec<TrackRow>, DbError> {
    let mut stmt = conn.prepare_cached(
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
    let mut stmt = conn.prepare_cached(
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

/// Get distinct genres for a batch of artist IDs in a single query.
/// Returns a map from artist_id → set of lowercased genre strings.
pub fn genres_by_artist_ids(
    conn: &Connection,
    ids: &[i64],
) -> Result<HashMap<i64, HashSet<String>>, DbError> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let mut stmt = conn.prepare_cached(
        "SELECT t.artist_id, t.genre FROM tracks t
         WHERE t.artist_id IN (SELECT value FROM json_each(?1)) AND t.genre IS NOT NULL
         UNION
         SELECT al.artist_id, t.genre FROM tracks t
         JOIN albums al ON t.album_id = al.id
         WHERE al.artist_id IN (SELECT value FROM json_each(?1)) AND t.genre IS NOT NULL",
    )?;
    let rows = stmt.query_map([super::json_list(ids)], |row| {
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
    let mut stmt = conn.prepare_cached(
        "SELECT t.album_id, t.genre FROM tracks t
         WHERE t.album_id IN (SELECT value FROM json_each(?1)) AND t.genre IS NOT NULL",
    )?;
    let rows = stmt.query_map([super::json_list(ids)], |row| {
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

/// The ids of a user's favourited tracks, as a subquery binding the user as
/// `user`.
///
/// Favourites are keyed by path, and a track is reached by any of three: its
/// file, its download, or its stream URL. Three indexed lookups rather than one
/// join with an `OR` across the three columns: SQLite cannot use an index for
/// that `OR`, so it read every track in the library and probed favourites for
/// each — fifty milliseconds to find a hundred rows.
pub(crate) fn favourite_track_ids_sql(user: &str) -> String {
    format!("SELECT track_id FROM favourites WHERE user_id = {user}")
}

/// Get all artist IDs that have at least one favourited track, in a single query.
pub fn favourite_artist_ids_batch(conn: &Connection, user: i64) -> Result<HashSet<i64>, DbError> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT DISTINCT artist_id FROM tracks
         WHERE artist_id IS NOT NULL AND id IN ({})",
        favourite_track_ids_sql("?1")
    ))?;
    let rows = stmt.query_map([super::auth::resolve_user(conn, user)?], |row| {
        row.get::<_, i64>(0)
    })?;
    let mut ids = HashSet::new();
    for row in rows {
        ids.insert(row?);
    }
    Ok(ids)
}

/// Get all favourited track IDs in a single query.
pub fn favourite_track_ids_batch(conn: &Connection, user: i64) -> Result<HashSet<i64>, DbError> {
    let mut stmt = conn.prepare_cached(&favourite_track_ids_sql("?1"))?;
    let rows = stmt.query_map([super::auth::resolve_user(conn, user)?], |row| {
        row.get::<_, i64>(0)
    })?;
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
/// Matched through [`favourite_track_ids_sql`].
pub fn favourite_tracks(
    conn: &Connection,
    user: i64,
    search: Option<&str>,
) -> Result<Vec<TrackRow>, DbError> {
    let mut sql = format!(
        "SELECT t.id, t.album_id, t.artist_id, a.name, aa.name, al.title,
                t.disc, t.track_number, t.title, t.duration_ms, t.path,
                t.codec, t.sample_rate, t.bit_depth, t.channels, t.bitrate,
                t.genre, t.source, t.remote_id, t.cached_path
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         LEFT JOIN artists aa ON al.artist_id = aa.id
         WHERE t.id IN ({})",
        favourite_track_ids_sql("?1")
    );
    let mut params: Vec<Box<dyn rusqlite::ToSql>> =
        vec![Box::new(super::auth::resolve_user(conn, user)?)];
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
pub fn favourite_album_ids_batch(conn: &Connection, user: i64) -> Result<HashSet<i64>, DbError> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT DISTINCT album_id FROM tracks
         WHERE album_id IS NOT NULL AND id IN ({})",
        favourite_track_ids_sql("?1")
    ))?;
    let rows = stmt.query_map([super::auth::resolve_user(conn, user)?], |row| {
        row.get::<_, i64>(0)
    })?;
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
    use crate::db::queries::{library_stats, sample_meta, search_tracks};

    fn test_db() -> Database {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        Database { conn }
    }

    #[test]
    fn a_track_lists_each_source_file_first() {
        let db = test_db();
        let local = sample_meta("Archangel", "Burial", "Untrue");
        let id = upsert_track(&db.conn, &local).unwrap();
        let mut remote = local.clone();
        remote.path = None;
        remote.source = "remote".into();
        remote.remote_id = Some("song-1".into());
        remote.mbid = Some("recording".into());
        assert_eq!(upsert_track(&db.conn, &remote).unwrap(), id, "one track");

        let sources = crate::db::queries::sources_of_track(&db.conn, id).unwrap();
        let kinds: Vec<&str> = sources.iter().map(|m| m.source.as_str()).collect();
        assert_eq!(kinds, ["local", "remote"]);
        assert_eq!(sources[0].path, local.path);
        assert_eq!(sources[1].remote_id.as_deref(), Some("song-1"));
        assert_eq!(sources[1].mbid.as_deref(), Some("recording"));
        assert!(
            crate::db::queries::sources_of_track(&db.conn, id + 1)
                .unwrap()
                .is_empty()
        );
    }

    /// A resync passes every track through the upsert, and nearly all of them
    /// are as they were. None of that should reach the disk.
    #[test]
    fn an_unchanged_track_is_not_rewritten() {
        let db = test_db();
        let local = sample_meta("Archangel", "Burial", "Untrue");
        let mut remote = sample_meta("Near Dark", "Burial", "Untrue");
        remote.path = None;
        remote.source = "remote".into();
        remote.remote_id = Some("0191d0c4-7c1e-7a2b-9f00-1a2b3c4d5e6f".into());
        remote.remote_url = Some("https://music.example/rest/stream?id=a".into());
        remote.album_remote_id = Some("0191d0c4-7c1e-7a2b-9f00-1a2b3c4d5e70".into());
        remote.artist_remote_id = Some("0191d0c4-7c1e-7a2b-9f00-1a2b3c4d5e71".into());
        remote.album_mbid = Some("release".into());
        remote.album_added_at = Some("2026-01-01T00:00:00Z".into());
        let seen: HashSet<String> = remote.remote_id.iter().cloned().collect();
        upsert_track(&db.conn, &local).unwrap();
        upsert_synced_track(&db.conn, &remote, &seen).unwrap();

        let before = db.conn.total_changes();
        upsert_track(&db.conn, &local).unwrap();
        upsert_synced_track(&db.conn, &remote, &seen).unwrap();
        assert_eq!(db.conn.total_changes(), before, "nothing was written");
        assert_eq!(search_tracks(&db.conn, "Archangel").unwrap().len(), 1);
        assert_eq!(search_tracks(&db.conn, "Near Dark").unwrap().len(), 1);
    }

    #[test]
    fn a_new_server_uid_is_inserted_with_the_row() {
        let db = test_db();
        let uid = "0191d0c4-7c1e-7a2b-9f00-1a2b3c4d5e6f";
        let mut meta = sample_meta("Near Dark", "Burial", "Untrue");
        meta.path = None;
        meta.remote_id = Some(uid.into());
        let id = upsert_track(&db.conn, &meta).unwrap();
        let stored: String = db
            .conn
            .query_row("SELECT uid FROM tracks WHERE id = ?1", [id], |r| r.get(0))
            .unwrap();
        assert_eq!(stored, uid);
    }

    #[test]
    fn a_changed_track_is_still_rewritten() {
        let db = test_db();
        let mut meta = sample_meta("Archangel", "Burial", "Untrue");
        meta.album_added_at = Some("2026-02-01T00:00:00Z".into());
        let id = upsert_track(&db.conn, &meta).unwrap();

        meta.title = "Archangel (Remastered)".into();
        meta.genre = Some("Dubstep".into());
        meta.bit_depth = Some(24);
        meta.codec = Some("ALAC".into());
        meta.album_added_at = Some("2026-01-01T00:00:00Z".into());
        assert_eq!(upsert_track(&db.conn, &meta).unwrap(), id);

        let (title, genre, bit_depth): (String, String, i32) = db
            .conn
            .query_row(
                "SELECT title, genre, bit_depth FROM tracks WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            (title.as_str(), genre.as_str(), bit_depth),
            ("Archangel (Remastered)", "Dubstep", 24)
        );
        assert_eq!(search_tracks(&db.conn, "Remastered").unwrap().len(), 1);
        assert_eq!(search_tracks(&db.conn, "Dubstep").unwrap().len(), 1);
        assert!(search_tracks(&db.conn, "Electronic").unwrap().is_empty());
        let (codec, added): (String, String) = db
            .conn
            .query_row("SELECT codec, added_at FROM albums", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(
            (codec.as_str(), added.as_str()),
            ("ALAC", "2026-01-01T00:00:00Z"),
            "the album takes the new codec and the earlier date"
        );

        meta.album_added_at = Some("2026-03-01T00:00:00Z".into());
        upsert_track(&db.conn, &meta).unwrap();
        let added: String = db
            .conn
            .query_row("SELECT added_at FROM albums", [], |r| r.get(0))
            .unwrap();
        assert_eq!(added, "2026-01-01T00:00:00Z", "a later date does not win");
    }

    #[test]
    fn favourite_tracks_come_back_as_rows_narrowed_by_search() {
        use crate::db::queries::toggle_favourite;
        let db = test_db();
        let amber = upsert_track(&db.conn, &sample_meta("Amber", "Autechre", "Amber")).unwrap();
        let mut foil = sample_meta("Foil", "Autechre", "Amber");
        foil.track_number = Some(2);
        let foil = upsert_track(&db.conn, &foil).unwrap();

        let titles = |q| {
            favourite_tracks(&db.conn, crate::db::queries::LOCAL_USER, q)
                .unwrap()
                .into_iter()
                .map(|t| t.title)
                .collect::<Vec<_>>()
        };
        assert!(titles(None).is_empty(), "nothing is favourite until it is");

        toggle_favourite(&db.conn, crate::db::queries::LOCAL_USER, amber).unwrap();
        toggle_favourite(&db.conn, crate::db::queries::LOCAL_USER, foil).unwrap();
        assert_eq!(titles(None), ["Amber", "Foil"]);
        assert_eq!(titles(Some("foil")), ["Foil"]);
        assert_eq!(
            titles(Some("autechre")).len(),
            2,
            "matched on the artist name"
        );
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

    fn uid_of(db: &Database, table: &str, id: i64) -> String {
        db.conn
            .query_row(
                &format!("SELECT uid FROM {table} WHERE id = ?1"),
                [id],
                |r| r.get(0),
            )
            .unwrap()
    }

    #[test]
    fn a_track_synced_from_koan_takes_the_servers_uids() {
        let db = test_db();
        let [track, album, artist] = [(); 3].map(|_| uuid::Uuid::now_v7().to_string());
        let mut meta = remote_meta("Archangel", "Burial", "Untrue", &track);
        meta.album_remote_id = Some(album.clone());
        meta.artist_remote_id = Some(artist.clone());

        let id = upsert_track(&db.conn, &meta).unwrap();

        let row = get_track_row(&db.conn, id).unwrap().unwrap();
        assert_eq!(uid_of(&db, "tracks", id), track);
        assert_eq!(uid_of(&db, "albums", row.album_id.unwrap()), album);
        assert_eq!(uid_of(&db, "artists", row.artist_id.unwrap()), artist);
    }

    #[test]
    fn a_local_file_merged_with_its_koan_copy_takes_the_servers_uid() {
        let db = test_db();
        let local = upsert_track(&db.conn, &sample_meta("Archangel", "Burial", "Untrue")).unwrap();
        let minted = uid_of(&db, "tracks", local);

        let server = uuid::Uuid::now_v7().to_string();
        let merged = upsert_track(
            &db.conn,
            &remote_meta("Archangel", "Burial", "Untrue", &server),
        )
        .unwrap();

        assert_eq!(merged, local);
        assert_ne!(minted, server);
        assert_eq!(uid_of(&db, "tracks", local), server);
    }

    #[test]
    fn ids_from_other_servers_leave_the_minted_uid() {
        let db = test_db();
        for remote_id in [
            "3xJ9kQ2pZ",
            "42",
            "0f8fad5b-d9cb-469f-a165-70867728950e",
            "018f8fad5bd9cb769fa16570867728950e",
        ] {
            let id = upsert_track(
                &db.conn,
                &remote_meta(remote_id, "Burial", "Untrue", remote_id),
            )
            .unwrap();
            let uid = uid_of(&db, "tracks", id);
            assert_ne!(uid, remote_id);
            assert!(super::super::is_uid(&uid), "{uid}");
        }
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
        crate::db::queries::record_play(
            &db.conn,
            crate::db::queries::LOCAL_USER,
            local_id,
            Some(1_000),
        )
        .unwrap();
        crate::db::queries::record_play(
            &db.conn,
            crate::db::queries::LOCAL_USER,
            remote_id,
            Some(2_000),
        )
        .unwrap();
        crate::db::queries::cache_lyrics(&db.conn, remote_id, "lrclib", true, "[00:01.00] la")
            .unwrap();

        let mut fixed = bad.clone();
        fixed.title = "Sunshine Recorder".into();
        let merged = upsert_track(&db.conn, &fixed).unwrap();
        assert_eq!(merged, local_id);

        assert_eq!(
            crate::db::queries::play_count(&db.conn, crate::db::queries::LOCAL_USER, merged)
                .unwrap(),
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
    fn test_remerge_keeps_playlist_entries() {
        let db = test_db();

        let mut bad = sample_meta("Untitled", "Boards of Canada", "Geogaddi");
        bad.path = Some("/music/boc/05.flac".into());
        let local_id = upsert_track(&db.conn, &bad).unwrap();
        let remote = remote_meta("Sunshine Recorder", "Boards of Canada", "Geogaddi", "sub-7");
        let remote_id = upsert_track(&db.conn, &remote).unwrap();

        let list = crate::db::queries::create_playlist(
            &db.conn,
            crate::db::queries::LOCAL_USER,
            "Road trip",
            None,
        )
        .unwrap();
        let entry = crate::db::queries::add_tracks(&db.conn, list, &[remote_id]).unwrap()[0];

        let mut fixed = bad.clone();
        fixed.title = "Sunshine Recorder".into();
        let merged = upsert_track(&db.conn, &fixed).unwrap();
        assert_eq!(merged, local_id);

        let entries = crate::db::queries::playlist_entries(&db.conn, list).unwrap();
        assert_eq!(
            entries.len(),
            1,
            "the merge left the playlist entry in place"
        );
        assert_eq!((entries[0].id, entries[0].track.id), (entry, merged));
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

        assert_eq!(
            merged, local_id,
            "the older row survives, with its id and history"
        );
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
    fn test_dedup_reads_disc_zero_as_no_disc() {
        let db = test_db();

        // The file says disc 0; the server leaves the field out.
        let mut local = sample_meta(
            "Miss Broadway (Main Version)",
            "Glass Candy",
            "Miss Broadway",
        );
        local.disc = Some(0);
        let id = upsert_track(&db.conn, &local).unwrap();

        let mut remote = remote_meta(
            "Miss Broadway (Main Version)",
            "Glass Candy",
            "Miss Broadway",
            "sub-1",
        );
        remote.disc = None;
        assert_eq!(upsert_track(&db.conn, &remote).unwrap(), id);

        let disc: Option<i32> = db
            .conn
            .query_row("SELECT disc FROM tracks WHERE id = ?1", params![id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(disc, None);
    }

    #[test]
    fn test_migration_folds_tracks_split_by_a_zero_disc() {
        let db = test_db();

        let local = sample_meta("Sumo", "Simtek", "In The Face EP");
        let winner = upsert_track(&db.conn, &local).unwrap();
        db.conn
            .execute("UPDATE tracks SET disc = 0 WHERE id = ?1", params![winner])
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO tracks (album_id, artist_id, disc, track_number, title,
                                     source, remote_id, remote_url)
                 SELECT album_id, artist_id, NULL, track_number, title,
                        'remote', 'sub-4', 'http://server/4'
                   FROM tracks WHERE id = ?1",
                params![winner],
            )
            .unwrap();

        clear_zero_discs(&db.conn).unwrap();
        sources::build_from_tracks(&db.conn).unwrap();

        let (rows, remote_id): (i64, Option<String>) = db
            .conn
            .query_row("SELECT COUNT(*), MAX(remote_id) FROM tracks", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(rows, 1);
        assert_eq!(remote_id.as_deref(), Some("sub-4"));
    }

    /// A track as tagged by Picard: recording and release ids alongside the names.
    fn with_ids(mut meta: TrackMeta, recording: &str, release: &str) -> TrackMeta {
        meta.mbid = Some(recording.into());
        meta.album_mbid = Some(release.into());
        meta
    }

    fn track_count(db: &Database) -> i64 {
        db.conn
            .query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn test_dedup_matches_by_musicbrainz_ids_whatever_the_album_is_called() {
        let db = test_db();

        // Navidrome appends the release's disambiguation to the album name.
        let local = with_ids(
            sample_meta(
                "Hypnotized",
                "Oliver Koletzki",
                "Renaissance: The Mix Collection",
            ),
            "rec-1",
            "rel-1",
        );
        let id = upsert_track(&db.conn, &local).unwrap();

        let mut remote = with_ids(
            remote_meta(
                "Hypnotized",
                "Oliver Koletzki",
                "Renaissance: The Mix Collection (Unmixed)",
                "sub-1",
            ),
            "rec-1",
            "rel-1",
        );
        remote.disc = None;
        assert_eq!(upsert_track(&db.conn, &remote).unwrap(), id);
        assert_eq!(track_count(&db), 1);
    }

    #[test]
    fn test_dedup_keeps_a_recording_apart_across_releases() {
        let db = test_db();

        // The same recording on the album and on a compilation is two tracks.
        let local = with_ids(
            sample_meta("Azure", "Paul Kalkbrenner", "Album"),
            "rec-1",
            "rel-1",
        );
        let first = upsert_track(&db.conn, &local).unwrap();

        let remote = with_ids(
            remote_meta("Azure", "Paul Kalkbrenner", "Compilation", "sub-1"),
            "rec-1",
            "rel-2",
        );
        assert_ne!(upsert_track(&db.conn, &remote).unwrap(), first);
    }

    #[test]
    fn test_dedup_by_musicbrainz_ids_picks_the_slot_when_a_release_repeats_a_recording() {
        let db = test_db();

        // A mixed disc and an unmixed disc can carry one recording each.
        let mut first = with_ids(
            sample_meta("Azure", "Paul Kalkbrenner", "Mixes"),
            "rec-1",
            "rel-1",
        );
        first.disc = Some(1);
        let first = upsert_track(&db.conn, &first).unwrap();
        let mut second = with_ids(
            sample_meta("Azure", "Paul Kalkbrenner", "Mixes"),
            "rec-1",
            "rel-1",
        );
        second.disc = Some(2);
        second.path = Some("/music/Mixes/2-01 Azure.flac".into());
        let second = upsert_track(&db.conn, &second).unwrap();

        let mut remote = with_ids(
            remote_meta("Azure", "Paul Kalkbrenner", "Mixes (Unmixed)", "sub-1"),
            "rec-1",
            "rel-1",
        );
        remote.disc = Some(2);
        assert_eq!(upsert_track(&db.conn, &remote).unwrap(), second);
        assert_ne!(first, second);
    }

    #[test]
    fn test_rescan_with_musicbrainz_ids_folds_an_already_split_pair() {
        let db = test_db();

        // Scanned before koan read the ids: nothing tied the two rows together.
        let local = sample_meta("Hypnotized", "Oliver Koletzki", "The Mix Collection");
        let id = upsert_track(&db.conn, &local).unwrap();
        let remote = with_ids(
            remote_meta(
                "Hypnotized",
                "Oliver Koletzki",
                "The Mix Collection (Unmixed)",
                "sub-1",
            ),
            "rec-1",
            "rel-1",
        );
        upsert_track(&db.conn, &remote).unwrap();
        assert_eq!(track_count(&db), 2);

        assert_eq!(
            upsert_track(&db.conn, &with_ids(local, "rec-1", "rel-1")).unwrap(),
            id
        );
        assert_eq!(track_count(&db), 1);
        let remote_id: Option<String> = db
            .conn
            .query_row(
                "SELECT remote_id FROM tracks WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(remote_id.as_deref(), Some("sub-1"));
        let albums: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM albums", [], |r| r.get(0))
            .unwrap();
        assert_eq!(albums, 1, "the server's album goes with its last track");
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

        sources::build_from_tracks(&db.conn).unwrap();

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

        sources::build_from_tracks(&db.conn).unwrap();

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
    fn server_deletions_reach_the_client() {
        use std::collections::HashSet;
        let db = test_db();
        let remote = |title: &str, album: &str, rid: &str, album_rid: &str| {
            let mut m = sample_meta(title, "Tove Lo", album);
            m.path = None;
            m.remote_id = Some(rid.into());
            m.album_remote_id = Some(album_rid.into());
            upsert_track(&db.conn, &m).unwrap();
        };
        remote("Habits", "Queen of the Clouds", "t1", "al-1");
        remote("Talking Body", "Queen of the Clouds", "t2", "al-1");
        remote("Habits", "Habits (single)", "t3", "al-2");
        let count = |sql: &str| -> i64 { db.conn.query_row(sql, [], |r| r.get(0)).unwrap() };

        // A sync that could not vouch for every track: the single's album is
        // gone from the listing.
        let albums: HashSet<String> = ["al-1".to_string()].into();
        assert_eq!(
            remove_vanished_remote(&db.conn, None, Some(&albums)).unwrap(),
            1
        );
        assert_eq!(
            count("SELECT COUNT(*) FROM albums WHERE title = 'Habits (single)'"),
            0
        );
        assert_eq!(count("SELECT COUNT(*) FROM tracks"), 2);

        // A sync that listed everything: one track gone from an album that stays.
        let tracks: HashSet<String> = ["t1".to_string()].into();
        assert_eq!(
            remove_vanished_remote(&db.conn, Some(&tracks), Some(&albums)).unwrap(),
            1
        );
        assert_eq!(count("SELECT COUNT(*) FROM tracks"), 1);
        assert_eq!(count("SELECT COUNT(*) FROM albums"), 1);
    }

    /// What a walk found is there without a stat; what it did not is still
    /// confirmed missing before it goes.
    #[test]
    fn stale_removal_trusts_the_walk_and_confirms_the_rest() {
        let db = test_db();
        let tmp = tempfile::tempdir().unwrap();
        let row = |name: &str| {
            let path = tmp.path().join(name).to_string_lossy().into_owned();
            let mut meta = sample_meta(name, "Artist", "Album");
            meta.path = Some(path.clone());
            upsert_track(&db.conn, &meta).unwrap();
            path
        };
        // Walked, then gone before removal ran: the walk is believed.
        let walked = row("walked.flac");
        // Not walked and not there: gone.
        let gone = row("gone.flac");
        // Not walked but there, such as a name the walk skips: kept.
        let present = row(".stversions-copy.flac");
        std::fs::write(&present, b"x").unwrap();
        let seen: std::collections::HashSet<String> = [walked.clone()].into();

        let removed = remove_stale_tracks_walked(&db.conn, tmp.path(), true, Some(&seen)).unwrap();
        assert_eq!(removed, [gone]);
        let paths: Vec<String> = db
            .conn
            .prepare("SELECT path FROM local_files ORDER BY path")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(paths.len(), 2);
        assert!(paths.contains(&walked) && paths.contains(&present));
    }

    #[test]
    fn test_stale_removal_drops_the_album_and_artist_it_empties() {
        let db = test_db();
        let tmp = tempfile::tempdir().unwrap();
        let keep = tmp.path().join("keep.flac");
        std::fs::write(&keep, b"x").unwrap();
        let mut kept = sample_meta("Kept", "Artist", "Staying");
        kept.path = Some(keep.to_string_lossy().into_owned());
        upsert_track(&db.conn, &kept).unwrap();
        let mut moved = sample_meta("Moved", "Other Credit", "Moved Away");
        moved.path = Some(tmp.path().join("gone.flac").to_string_lossy().into_owned());
        upsert_track(&db.conn, &moved).unwrap();

        assert_eq!(
            remove_stale_tracks(&db.conn, tmp.path(), false)
                .unwrap()
                .len(),
            1
        );
        let count = |sql: &str| -> i64 { db.conn.query_row(sql, [], |r| r.get(0)).unwrap() };
        assert_eq!(
            count("SELECT COUNT(*) FROM albums WHERE title = 'Moved Away'"),
            0
        );
        assert_eq!(
            count("SELECT COUNT(*) FROM artists WHERE name = 'Other Credit'"),
            0
        );
        assert_eq!(
            count("SELECT COUNT(*) FROM albums WHERE title = 'Staying'"),
            1
        );
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
        let id = upsert_track(&db.conn, &meta).unwrap();
        crate::db::queries::add_favourite(&db.conn, crate::db::queries::LOCAL_USER, id).unwrap();

        let artist_id: i64 = db
            .conn
            .query_row(
                "SELECT id FROM artists WHERE name = 'FavArtist'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        let fav_ids = favourite_artist_ids_batch(&db.conn, crate::db::queries::LOCAL_USER).unwrap();
        assert!(fav_ids.contains(&artist_id));
    }

    #[test]
    fn test_favourite_artist_ids_batch_empty() {
        let db = test_db();
        let fav_ids = favourite_artist_ids_batch(&db.conn, crate::db::queries::LOCAL_USER).unwrap();
        assert!(fav_ids.is_empty());
    }

    #[test]
    fn test_favourite_album_ids_batch() {
        let db = test_db();
        let meta = sample_meta("FavTrack", "FavArtist", "FavAlbum");
        let id = upsert_track(&db.conn, &meta).unwrap();
        crate::db::queries::add_favourite(&db.conn, crate::db::queries::LOCAL_USER, id).unwrap();

        let album_id: i64 = db
            .conn
            .query_row(
                "SELECT id FROM albums WHERE title = 'FavAlbum'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        let fav_ids = favourite_album_ids_batch(&db.conn, crate::db::queries::LOCAL_USER).unwrap();
        assert!(fav_ids.contains(&album_id));
    }

    #[test]
    fn test_favourite_album_ids_batch_empty() {
        let db = test_db();
        let fav_ids = favourite_album_ids_batch(&db.conn, crate::db::queries::LOCAL_USER).unwrap();
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

        clear_cached_paths_for(&db.conn, &[id]).unwrap();

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
    fn test_cached_files_lru_excludes_favourited_albums() {
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
        let t3: i64 = db
            .conn
            .query_row(
                "SELECT id FROM tracks WHERE cached_path = '/cache/AlbumB/T3.flac'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        crate::db::queries::add_favourite(&db.conn, crate::db::queries::LOCAL_USER, t3).unwrap();

        let files = cached_files_lru(&db.conn).unwrap();

        // AlbumB has a favourite in it, so neither of its files is listed.
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["/cache/AlbumA/T1.flac", "/cache/AlbumA/T2.flac"]);
    }

    #[test]
    fn eviction_keeps_what_the_queue_holds() {
        crate::config::isolate_config_for_tests();
        let db = test_db();
        let mut ids = Vec::new();
        for (album, played_at) in &[("OldAlbum", 1000), ("NewAlbum", 9000)] {
            let mut meta = sample_meta("Track", "Artist", album);
            meta.source = "remote".into();
            meta.path = None;
            meta.remote_id = Some(format!("r-{}", album));
            let id = upsert_track(&db.conn, &meta).unwrap();
            db.conn
                .execute(
                    "UPDATE tracks SET cached_path = ?1, cache_size_bytes = 10000000 WHERE id = ?2",
                    params![format!("/nonexistent/{album}/Track.flac"), id],
                )
                .unwrap();
            db.conn
                .execute(
                    "INSERT INTO play_history (track_id, played_at) VALUES (?1, ?2)",
                    params![id, played_at],
                )
                .unwrap();
            ids.push(id);
        }
        let mut cfg = crate::config::Config::default();
        cfg.remote.cache_limit = Some("15MB".into());

        // The older album would go first, but it is queued.
        let keep = std::collections::HashSet::from([ids[0]]);
        crate::helpers::evict_cache(&db, &cfg, &keep, false);

        let cached: Vec<i64> = ids
            .iter()
            .copied()
            .filter(|id| {
                db.conn
                    .query_row(
                        "SELECT cached_path IS NOT NULL FROM tracks WHERE id = ?1",
                        params![id],
                        |r| r.get(0),
                    )
                    .unwrap()
            })
            .collect();
        assert_eq!(cached, vec![ids[0]]);
    }

    /// One cached track per `(album, last used, pinned)`, 10MB each.
    fn cached_tracks(db: &Database, albums: &[(&str, i64, bool)]) -> Vec<i64> {
        albums
            .iter()
            .map(|(album, used, pinned)| {
                let mut meta = sample_meta("Track", "Artist", album);
                meta.source = "remote".into();
                meta.path = None;
                meta.remote_id = Some(format!("r-{album}"));
                let id = upsert_track(&db.conn, &meta).unwrap();
                db.conn
                    .execute(
                        "UPDATE tracks SET cached_path = ?1, cache_size_bytes = 10000000,
                                cache_download_date = ?2, cache_pinned = ?3
                         WHERE id = ?4",
                        params![format!("/nonexistent/{album}/Track.flac"), used, pinned, id],
                    )
                    .unwrap();
                id
            })
            .collect()
    }

    fn still_cached(db: &Database, ids: &[i64]) -> Vec<i64> {
        ids.iter()
            .copied()
            .filter(|id| {
                db.conn
                    .query_row(
                        "SELECT cached_path IS NOT NULL FROM tracks WHERE id = ?1",
                        params![id],
                        |r| r.get(0),
                    )
                    .unwrap()
            })
            .collect()
    }

    #[test]
    fn pinned_downloads_are_evicted_after_everything_fetched_to_play() {
        crate::config::isolate_config_for_tests();
        let db = test_db();
        // The pinned album is the least recently used, and still goes last.
        let ids = cached_tracks(
            &db,
            &[
                ("Pinned", 1000, true),
                ("Old", 2000, false),
                ("New", 3000, false),
            ],
        );
        let mut cfg = crate::config::Config::default();
        cfg.remote.cache_limit = Some("15MB".into());

        crate::helpers::evict_cache(&db, &cfg, &Default::default(), false);
        assert_eq!(still_cached(&db, &ids), vec![ids[0]]);

        cfg.remote.cache_limit = Some("5MB".into());
        crate::helpers::evict_cache(&db, &cfg, &Default::default(), false);
        assert!(still_cached(&db, &ids).is_empty(), "pinned is not forever");
    }

    #[test]
    fn eviction_takes_the_played_half_of_a_queued_album() {
        crate::config::isolate_config_for_tests();
        let db = test_db();
        let mut ids = Vec::new();
        for n in 1..=4 {
            let mut meta = sample_meta(&format!("T{n}"), "Artist", "Album");
            meta.source = "remote".into();
            meta.path = None;
            meta.remote_id = Some(format!("r-{n}"));
            meta.track_number = Some(n);
            let id = upsert_track(&db.conn, &meta).unwrap();
            db.conn
                .execute(
                    "UPDATE tracks SET cached_path = ?1, cache_size_bytes = 10000000 WHERE id = ?2",
                    params![format!("/nonexistent/T{n}.flac"), id],
                )
                .unwrap();
            ids.push(id);
        }
        let mut cfg = crate::config::Config::default();
        cfg.remote.cache_limit = Some("25MB".into());

        // Two played, two to come.
        let keep = std::collections::HashSet::from([ids[2], ids[3]]);
        crate::helpers::evict_cache(&db, &cfg, &keep, false);
        assert_eq!(still_cached(&db, &ids), vec![ids[2], ids[3]]);
    }

    #[test]
    fn the_window_is_what_fits_beside_pinned_downloads() {
        crate::config::isolate_config_for_tests();
        let db = test_db();
        let pinned = cached_tracks(&db, &[("Pinned", 1000, true)]);
        let mut upcoming = Vec::new();
        for n in 1..=5 {
            let mut meta = sample_meta(&format!("Q{n}"), "Artist", "Queued");
            meta.source = "remote".into();
            meta.path = None;
            meta.remote_id = Some(format!("q-{n}"));
            meta.track_number = Some(n);
            // No size from the server: 10MB, reckoned at 1000kbps.
            meta.size_bytes = None;
            meta.bitrate = Some(1000);
            meta.duration_ms = Some(80_000);
            upcoming.push(upsert_track(&db.conn, &meta).unwrap());
        }

        // 10MB pinned, so a 45MB limit leaves room for three queued tracks.
        let fits = crate::helpers::playback_window(&db, 45_000_000, &upcoming).unwrap();
        assert_eq!(fits, 3);

        // The playing track and the next are held however small the limit.
        let fits = crate::helpers::playback_window(&db, 1, &upcoming).unwrap();
        assert_eq!(fits, 2);

        // A pinned track in the queue is already counted.
        let mut with_pinned = vec![pinned[0]];
        with_pinned.extend(&upcoming);
        let fits = crate::helpers::playback_window(&db, 45_000_000, &with_pinned).unwrap();
        assert_eq!(fits, 4);
    }

    #[test]
    fn test_cached_files_lru_sorted_by_last_play() {
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

        let files = cached_files_lru(&db.conn).unwrap();
        // OldAlbum (played_at=1000) comes first: evicted first.
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            paths,
            ["/cache/OldAlbum/Track.flac", "/cache/NewAlbum/Track.flac"]
        );
    }

    #[test]
    fn test_cached_files_lru_counts_a_fresh_download_as_used() {
        let db = test_db();
        // Played long ago, and downloaded just now for offline listening.
        for (album, played_at, downloaded) in
            [("Played", Some(5000), 1000), ("Fetched", None, 9000)]
        {
            let mut meta = sample_meta("Track", "Artist", album);
            meta.source = "remote".into();
            meta.path = None;
            meta.remote_id = Some(format!("r-{album}"));
            let id = upsert_track(&db.conn, &meta).unwrap();
            db.conn
                .execute(
                    "UPDATE tracks SET cached_path = ?1, cache_size_bytes = 1, cache_download_date = ?2
                     WHERE id = ?3",
                    params![format!("/cache/{album}.flac"), downloaded, id],
                )
                .unwrap();
            if let Some(played_at) = played_at {
                db.conn
                    .execute(
                        "INSERT INTO play_history (track_id, played_at) VALUES (?1, ?2)",
                        params![id, played_at],
                    )
                    .unwrap();
            }
        }

        let files = cached_files_lru(&db.conn).unwrap();
        let order: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(order, ["/cache/Played.flac", "/cache/Fetched.flac"]);
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

    #[test]
    fn test_migration_folds_a_file_indexed_under_two_spellings() {
        use unicode_normalization::UnicodeNormalization;
        let db = test_db();
        let nfc: String = "/music/Roman Flügel - Softice.flac".nfc().collect();
        let nfd: String = "/music/Roman Flügel - Softice.flac".nfd().collect();
        assert_ne!(nfc, nfd);

        // The state a drop left behind: the file under Foundation's spelling,
        // with the sync link, then the next scan's row under the disk's.
        let mut dropped = sample_meta("Softice", "Roman Flügel", "Renaissance");
        dropped.path = Some(nfc.clone());
        dropped.remote_id = Some("sub-7".into());
        let winner = upsert_track(&db.conn, &dropped).unwrap();
        let mut scanned = sample_meta("Softice", "Roman Flügel", "Renaissance");
        scanned.path = Some(nfd.clone());
        let loser = upsert_track(&db.conn, &scanned).unwrap();
        assert_ne!(
            winner, loser,
            "the bytes differ, so the old key made two rows"
        );
        crate::db::queries::add_favourite(&db.conn, crate::db::queries::LOCAL_USER, loser).unwrap();
        for (path, id) in [(&nfc, winner), (&nfd, loser)] {
            db.conn
                .execute(
                    "INSERT INTO scan_cache (path, mtime, size, track_id) VALUES (?1, 1, 1, ?2)",
                    params![path, id],
                )
                .unwrap();
        }

        // The fold runs before the source rows are built from the tracks.
        db.conn
            .execute_batch("DELETE FROM local_files; DELETE FROM remote_entries;")
            .unwrap();
        merge_spelling_twins(&db.conn).unwrap();

        let rows: Vec<(i64, String, Option<String>)> = db
            .conn
            .prepare("SELECT id, path, remote_id FROM tracks")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(rows.len(), 1, "one file, one row");
        assert_eq!(rows[0].0, winner, "the older row keeps its identity");
        assert_eq!(rows[0].1, nfd, "and takes the disk's spelling");
        assert_eq!(rows[0].2.as_deref(), Some("sub-7"), "and its sync link");
        let starred: i64 = db
            .conn
            .query_row("SELECT track_id FROM favourites", [], |r| r.get(0))
            .unwrap();
        assert_eq!(starred, winner, "the favourite follows the row");
        let cached: Vec<(String, i64)> = db
            .conn
            .prepare("SELECT path, track_id FROM scan_cache")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            cached,
            vec![(nfd, winner)],
            "one cache row, under the spelling a scan will ask for"
        );
    }

    #[test]
    fn test_migration_leaves_a_pair_with_no_disk_spelling_alone() {
        use unicode_normalization::UnicodeNormalization;
        let db = test_db();
        // Two precomposed rows can't both have come from the directory, so there
        // is nothing to say which is the file.
        let a: String = "/music/Isolée - Allowance.flac".nfc().collect();
        let b: String = "/music/Isolée - Allowance.flac"
            .nfc()
            .chain(" ".chars())
            .collect();
        for path in [&a, &b] {
            let mut meta = sample_meta("Allowance", "Isolée", "Renaissance");
            meta.path = Some(path.clone());
            upsert_track(&db.conn, &meta).unwrap();
        }
        merge_spelling_twins(&db.conn).unwrap();
        let rows: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 2);
    }

    fn names_of(db: &Database, id: i64) -> (String, String, String, Option<String>) {
        db.conn
            .query_row(
                "SELECT t.title, a.name, al.title, t.mbid FROM tracks t
                   JOIN artists a ON a.id = t.artist_id
                   JOIN albums al ON al.id = t.album_id
                  WHERE t.id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap()
    }

    #[test]
    fn a_merged_track_keeps_its_files_names_through_every_sync() {
        let db = test_db();
        let local = with_ids(
            sample_meta("Hypnotized", "Oliver Koletzki", "Renaissance"),
            "rec-1",
            "rel-1",
        );
        let id = upsert_track(&db.conn, &local).unwrap();
        let album = db
            .conn
            .query_row("SELECT album_id FROM tracks WHERE id = ?1", [id], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap();
        let album_uid = uid_of(&db, "albums", album);

        let mut remote = with_ids(
            remote_meta(
                "Hypnotized",
                "Oliver Koletzki • Fran",
                "Renaissance (Unmixed)",
                "sub-1",
            ),
            "rec-1",
            "rel-1",
        );
        remote.album_artist = Some("Oliver Koletzki".into());
        for _ in 0..2 {
            assert_eq!(upsert_track(&db.conn, &remote).unwrap(), id);
            assert_eq!(upsert_track(&db.conn, &local).unwrap(), id);
            assert_eq!(upsert_track(&db.conn, &remote).unwrap(), id);
            let (_, artist, album_title, _) = names_of(&db, id);
            assert_eq!(artist, "Oliver Koletzki");
            assert_eq!(album_title, "Renaissance");
        }

        // The album the file named keeps its uid, and the server's spellings
        // leave nothing behind in the browser.
        assert_eq!(uid_of(&db, "albums", album), album_uid);
        let albums: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM albums", [], |r| r.get(0))
            .unwrap();
        let artists: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM artists", [], |r| r.get(0))
            .unwrap();
        assert_eq!((albums, artists), (1, 1));
        let hits: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM tracks_fts WHERE tracks_fts MATCH 'Unmixed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hits, 0, "search indexes the names the row holds");
    }

    #[test]
    fn a_corrected_tag_survives_a_sync_of_the_old_one() {
        let db = test_db();
        let mut local = sample_meta("Treading Water", "Petrol Girls", "Talk of Violence");
        local.genre = Some("Punk".into());
        let id = upsert_track(&db.conn, &local).unwrap();
        let mut remote = remote_meta(
            "Treading Water",
            "Petrol Girls",
            "Talk of Violence",
            "sub-1",
        );
        remote.genre = Some("Punk".into());
        assert_eq!(upsert_track(&db.conn, &remote).unwrap(), id);

        local.genre = Some("Post-Hardcore".into());
        local.mbid = Some("rec-corrected".into());
        upsert_track(&db.conn, &local).unwrap();
        remote.mbid = Some("rec-stale".into());
        upsert_track(&db.conn, &remote).unwrap();

        let row = get_track_row(&db.conn, id).unwrap().unwrap();
        assert_eq!(row.genre.as_deref(), Some("Post-Hardcore"));
        assert_eq!(names_of(&db, id).3.as_deref(), Some("rec-corrected"));
    }

    #[test]
    fn a_server_only_track_takes_the_servers_corrections() {
        let db = test_db();
        let mut remote = remote_meta(
            "Treading Water",
            "Petrol Girls",
            "Talk of Violence",
            "sub-1",
        );
        let id = upsert_track(&db.conn, &remote).unwrap();
        remote.title = "Treading Water (Live)".into();
        remote.mbid = Some("rec-2".into());
        assert_eq!(upsert_track(&db.conn, &remote).unwrap(), id);
        let (title, _, _, mbid) = names_of(&db, id);
        assert_eq!(title, "Treading Water (Live)");
        assert_eq!(mbid.as_deref(), Some("rec-2"));
    }

    #[test]
    fn a_corrected_musicbrainz_id_lands() {
        let db = test_db();
        let mut local = with_ids(
            sample_meta("Azure", "Paul Kalkbrenner", "Album"),
            "rec-wrong",
            "rel-wrong",
        );
        let id = upsert_track(&db.conn, &local).unwrap();
        local.mbid = Some("rec-right".into());
        local.album_mbid = Some("rel-right".into());
        upsert_track(&db.conn, &local).unwrap();

        let release: String = db
            .conn
            .query_row(
                "SELECT al.mbid FROM tracks t JOIN albums al ON al.id = t.album_id WHERE t.id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(names_of(&db, id).3.as_deref(), Some("rec-right"));
        assert_eq!(release, "rel-right");

        // A server disagreeing about the release does not overrule the file.
        let remote = with_ids(
            remote_meta("Azure", "Paul Kalkbrenner", "Album", "sub-1"),
            "rec-right",
            "rel-other",
        );
        upsert_track(&db.conn, &remote).unwrap();
        let release: String = db
            .conn
            .query_row(
                "SELECT al.mbid FROM tracks t JOIN albums al ON al.id = t.album_id WHERE t.id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(release, "rel-right");
    }

    #[test]
    fn corrected_tags_remerge_across_differing_artist_credits() {
        let db = test_db();
        let mut bad = sample_meta("Treading Wat", "Petrol Girls", "Talk of Viol");
        bad.path = Some("/music/petrol-girls/01.mp3".into());
        let local_id = upsert_track(&db.conn, &bad).unwrap();

        let mut remote = remote_meta(
            "Treading Water",
            "Petrol Girls • Ren Aldridge",
            "Talk of Violence",
            "sub-1",
        );
        remote.album_artist = Some("Petrol Girls".into());
        upsert_track(&db.conn, &remote).unwrap();
        assert_eq!(track_count(&db), 2);

        let mut fixed = bad.clone();
        fixed.title = "Treading Water".into();
        fixed.album = "Talk of Violence".into();
        assert_eq!(upsert_track(&db.conn, &fixed).unwrap(), local_id);
        assert_eq!(track_count(&db), 1, "the re-merge is the whole cascade");
    }

    #[test]
    fn a_remerge_keeps_the_servers_uid() {
        let db = test_db();
        let mut bad = sample_meta("Archangel", "Burial", "Untru");
        bad.path = Some("/music/burial/01.flac".into());
        let local_id = upsert_track(&db.conn, &bad).unwrap();

        let server = uuid::Uuid::now_v7().to_string();
        upsert_track(
            &db.conn,
            &remote_meta("Archangel", "Burial", "Untrue", &server),
        )
        .unwrap();
        assert_eq!(track_count(&db), 2);

        let mut fixed = bad.clone();
        fixed.album = "Untrue".into();
        assert_eq!(upsert_track(&db.conn, &fixed).unwrap(), local_id);
        assert_eq!(track_count(&db), 1);
        assert_eq!(uid_of(&db, "tracks", local_id), server);
    }

    #[test]
    fn a_relinked_file_keeps_the_servers_uid() {
        let db = test_db();
        let old = uuid::Uuid::now_v7().to_string();
        let mut file = sample_meta("Archangel", "Burial", "Untrue");
        file.remote_id = Some(old.clone());
        let file_id = upsert_track(&db.conn, &file).unwrap();

        let new = uuid::Uuid::now_v7().to_string();
        upsert_track(
            &db.conn,
            &remote_meta("Archangel", "Burial", "Untrue", &new),
        )
        .unwrap();
        remove_vanished_remote(&db.conn, Some(&HashSet::from([new.clone()])), None).unwrap();

        assert_eq!(track_count(&db), 1);
        assert_eq!(uid_of(&db, "tracks", file_id), new);
    }

    fn source_names(db: &Database, table: &str) -> Vec<(String, String)> {
        db.conn
            .prepare(&format!("SELECT title, artist FROM {table} ORDER BY title"))
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    #[test]
    fn each_source_keeps_its_own_tags() {
        let db = test_db();
        let local = sample_meta("Treading Water", "Petrol Girls", "Talk of Violence");
        let id = upsert_track(&db.conn, &local).unwrap();
        let mut remote = remote_meta(
            "Treading Water",
            "Petrol Girls • Ren Aldridge",
            "Talk of Violence",
            "sub-1",
        );
        remote.album_artist = Some("Petrol Girls".into());
        assert_eq!(upsert_track(&db.conn, &remote).unwrap(), id);

        assert_eq!(
            source_names(&db, "local_files"),
            [("Treading Water".into(), "Petrol Girls".into())]
        );
        assert_eq!(
            source_names(&db, "remote_entries"),
            [(
                "Treading Water".into(),
                "Petrol Girls • Ren Aldridge".into()
            )]
        );
    }

    #[test]
    fn a_file_retagged_as_another_track_leaves_the_server_copy() {
        let db = test_db();
        let mut local = sample_meta("Archangel", "Burial", "Untrue");
        let id = upsert_track(&db.conn, &local).unwrap();
        let remote = remote_meta("Archangel", "Burial", "Untrue", "sub-1");
        assert_eq!(upsert_track(&db.conn, &remote).unwrap(), id);

        local.title = "Near Dark".into();
        local.track_number = Some(2);
        let moved = upsert_track(&db.conn, &local).unwrap();

        assert_eq!(track_count(&db), 2);
        let stays = get_track_row(&db.conn, id).unwrap().unwrap();
        let left = get_track_row(&db.conn, moved).unwrap().unwrap();
        assert_eq!(
            (stays.title.as_str(), stays.remote_id.as_deref(), stays.path),
            ("Archangel", Some("sub-1"), None)
        );
        assert_eq!(
            (left.title.as_str(), left.remote_id, left.path),
            ("Near Dark", None, local.path)
        );
    }

    #[test]
    fn an_ambiguous_match_is_declined() {
        let db = test_db();
        // The server holds the same file twice under two ids.
        upsert_track(
            &db.conn,
            &remote_meta("Archangel", "Burial", "Untrue", "sub-1"),
        )
        .unwrap();
        upsert_track(
            &db.conn,
            &remote_meta("Archangel", "Burial", "Untrue", "sub-2"),
        )
        .unwrap();
        upsert_track(&db.conn, &sample_meta("Archangel", "Burial", "Untrue")).unwrap();
        assert_eq!(track_count(&db), 3, "which copy is the file is a guess");
    }

    #[test]
    fn names_match_whatever_their_case_or_normal_form() {
        use unicode_normalization::UnicodeNormalization;
        let db = test_db();
        let local = sample_meta("Sæglópur", "Sigur Rós", "Takk...");
        let id = upsert_track(&db.conn, &local).unwrap();
        let mut remote = remote_meta(
            &"SÆGLÓPUR".nfd().collect::<String>(),
            "SIGUR RÓS",
            "TAKK...",
            "sub-1",
        );
        remote.album_artist = Some("SIGUR RÓS".into());
        assert_eq!(upsert_track(&db.conn, &remote).unwrap(), id);
    }

    #[test]
    fn a_moved_file_takes_over_its_server_copy() {
        let tmp = tempfile::tempdir().unwrap();
        let db = test_db();
        let old = tmp.path().join("old.flac");
        let new = tmp.path().join("new.flac");
        std::fs::write(&new, b"").unwrap();

        let mut local = sample_meta("Archangel", "Burial", "Untrue");
        local.path = Some(old.to_string_lossy().into_owned());
        let id = upsert_track(&db.conn, &local).unwrap();
        upsert_track(
            &db.conn,
            &remote_meta("Archangel", "Burial", "Untrue", "sub-1"),
        )
        .unwrap();
        db.conn
            .execute(
                "INSERT INTO play_history (track_id, played_at) VALUES (?1, 1)",
                params![id],
            )
            .unwrap();

        // The scan meets the file at its new path before noticing the old one gone.
        local.path = Some(new.to_string_lossy().into_owned());
        upsert_track(&db.conn, &local).unwrap();
        assert_eq!(track_count(&db), 2);
        remove_stale_tracks(&db.conn, tmp.path(), false).unwrap();

        assert_eq!(track_count(&db), 1);
        let row = tracks_by_ids(&db.conn, &[id]).unwrap();
        let row = row
            .first()
            .expect("the older row, with its history, survives");
        assert_eq!(row.path, local.path);
        assert_eq!(row.remote_id.as_deref(), Some("sub-1"));
        let plays: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM play_history WHERE track_id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(plays, 1);
    }

    #[test]
    fn a_favourite_follows_a_track_whose_file_is_gone() {
        let tmp = tempfile::tempdir().unwrap();
        let db = test_db();
        let mut local = sample_meta("Archangel", "Burial", "Untrue");
        local.path = Some(tmp.path().join("a.flac").to_string_lossy().into_owned());
        let id = upsert_track(&db.conn, &local).unwrap();
        upsert_track(
            &db.conn,
            &remote_meta("Archangel", "Burial", "Untrue", "sub-1"),
        )
        .unwrap();
        crate::db::queries::add_favourite(&db.conn, crate::db::queries::LOCAL_USER, id).unwrap();

        remove_stale_tracks(&db.conn, tmp.path(), false).unwrap();

        let favourites =
            favourite_track_ids_batch(&db.conn, crate::db::queries::LOCAL_USER).unwrap();
        assert!(favourites.contains(&id), "streamed now, and still starred");
    }

    #[test]
    fn the_servers_album_id_lands_on_the_album_the_file_names() {
        let db = test_db();
        let id = upsert_track(
            &db.conn,
            &with_ids(
                sample_meta("Hypnotized", "Oliver Koletzki", "Renaissance"),
                "rec-1",
                "rel-1",
            ),
        )
        .unwrap();
        let album_uid = uuid::Uuid::now_v7().to_string();
        let artist_uid = uuid::Uuid::now_v7().to_string();
        let mut remote = with_ids(
            remote_meta(
                "Hypnotized",
                "Oliver Koletzki",
                "Renaissance (Unmixed)",
                "sub-1",
            ),
            "rec-1",
            "rel-1",
        );
        remote.album_remote_id = Some(album_uid.clone());
        remote.artist_remote_id = Some(artist_uid.clone());
        upsert_track(&db.conn, &remote).unwrap();

        let (album, artist): (i64, i64) = db
            .conn
            .query_row(
                "SELECT al.id, al.artist_id FROM tracks t JOIN albums al ON al.id = t.album_id
                  WHERE t.id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        let rid: String = db
            .conn
            .query_row("SELECT remote_id FROM albums WHERE id = ?1", [album], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(rid, album_uid, "album favourites reconcile by it");
        assert_eq!(uid_of(&db, "albums", album), album_uid);
        assert_eq!(uid_of(&db, "artists", artist), artist_uid);
    }

    #[test]
    fn a_server_fills_a_number_the_file_lacks() {
        let db = test_db();
        let mut local = with_ids(
            sample_meta("Archangel", "Burial", "Untrue"),
            "rec-1",
            "rel-1",
        );
        local.track_number = None;
        let id = upsert_track(&db.conn, &local).unwrap();
        let remote = with_ids(
            remote_meta("Archangel", "Burial", "Untrue", "sub-1"),
            "rec-1",
            "rel-1",
        );
        assert_eq!(upsert_track(&db.conn, &remote).unwrap(), id);
        let row = get_track_row(&db.conn, id).unwrap().unwrap();
        assert_eq!(
            (row.track_number, row.title.as_str()),
            (Some(1), "Archangel")
        );
    }

    #[test]
    fn two_editions_with_their_own_release_ids_are_two_albums() {
        let db = test_db();
        let edition = |title: &str, number: i32, release: &str| {
            let mut m = with_ids(
                sample_meta(title, "Paul Kalkbrenner", "Album"),
                title,
                release,
            );
            m.track_number = Some(number);
            m
        };
        let mut original = edition("Azure", 1, "rel-original");
        let remaster = edition("Azure", 1, "rel-remaster");
        let mut remaster = TrackMeta {
            path: Some("/music/Album (Remaster)/Azure.flac".into()),
            ..remaster
        };
        let a = upsert_track(&db.conn, &original).unwrap();
        let b = upsert_track(&db.conn, &remaster).unwrap();
        let albums = |db: &Database| -> Vec<(i64, String)> {
            db.conn
                .prepare("SELECT id, mbid FROM albums ORDER BY id")
                .unwrap()
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        let before = albums(&db);
        assert_eq!(before.len(), 2);
        let album_of = |id| get_track_row(&db.conn, id).unwrap().unwrap().album_id;
        assert_ne!(album_of(a), album_of(b));

        // Rescans of either edition leave both where they are.
        for _ in 0..2 {
            original.mtime = original.mtime.map(|t| t + 1);
            upsert_track(&db.conn, &original).unwrap();
            remaster.mtime = remaster.mtime.map(|t| t + 1);
            upsert_track(&db.conn, &remaster).unwrap();
            assert_eq!(albums(&db), before);
        }
    }

    #[test]
    fn a_server_album_named_differently_joins_the_files_album() {
        let db = test_db();
        let file = with_ids(
            sample_meta("Hypnotized", "Oliver Koletzki", "Renaissance"),
            "rec-1",
            "rel-1",
        );
        let held = upsert_track(&db.conn, &file).unwrap();
        let entry = |title: &str, number: i32, id: &str, rec: &str| {
            let mut m = with_ids(
                remote_meta(title, "Oliver Koletzki", "Renaissance (Unmixed)", id),
                rec,
                "rel-1",
            );
            m.track_number = Some(number);
            m.album_remote_id = Some("al-1".into());
            m
        };
        // The server's copy of the file, and a track only the server has.
        upsert_track(&db.conn, &entry("Hypnotized", 1, "s-1", "rec-1")).unwrap();
        let streamed = upsert_track(&db.conn, &entry("Dance Tonight", 2, "s-2", "rec-2")).unwrap();

        let album_of = |id| {
            get_track_row(&db.conn, id)
                .unwrap()
                .unwrap()
                .album_id
                .unwrap()
        };
        assert_eq!(album_of(held), album_of(streamed), "one record, one album");
        let (title, rid): (String, String) = db
            .conn
            .query_row(
                "SELECT title, remote_id FROM albums WHERE id = ?1",
                [album_of(held)],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((title.as_str(), rid.as_str()), ("Renaissance", "al-1"));
        let albums: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM albums", [], |r| r.get(0))
            .unwrap();
        assert_eq!(albums, 1);
    }

    #[test]
    fn an_artist_is_one_row_whatever_its_case_or_normal_form() {
        use unicode_normalization::UnicodeNormalization;
        let db = test_db();
        upsert_track(&db.conn, &sample_meta("Hoppípolla", "Sigur Rós", "Takk...")).unwrap();
        let mut other = sample_meta(
            "Svefn-g-englar",
            &"SIGUR RÓS".nfd().collect::<String>(),
            "Ágætis byrjun",
        );
        other.path = Some("/music/agaetis/01.flac".into());
        upsert_track(&db.conn, &other).unwrap();
        let artists: Vec<String> = db
            .conn
            .prepare("SELECT name FROM artists")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(artists, ["Sigur Rós"], "the first spelling seen names it");
    }

    #[test]
    fn a_server_album_synced_before_the_files_folds_into_theirs() {
        let db = test_db();
        let entry = |title: &str, number: i32, id: &str, rec: &str| {
            let mut m = with_ids(
                remote_meta(title, "Oliver Koletzki", "Renaissance (Unmixed)", id),
                rec,
                "rel-1",
            );
            m.track_number = Some(number);
            m.album_remote_id = Some("al-1".into());
            m
        };
        upsert_track(&db.conn, &entry("Hypnotized", 1, "s-1", "rec-1")).unwrap();
        let streamed = upsert_track(&db.conn, &entry("Dance Tonight", 2, "s-2", "rec-2")).unwrap();
        let held = upsert_track(
            &db.conn,
            &with_ids(
                sample_meta("Hypnotized", "Oliver Koletzki", "Renaissance"),
                "rec-1",
                "rel-1",
            ),
        )
        .unwrap();

        let album_of = |id| {
            get_track_row(&db.conn, id)
                .unwrap()
                .unwrap()
                .album_id
                .unwrap()
        };
        assert_eq!(album_of(held), album_of(streamed));
        let titles: Vec<String> = db
            .conn
            .prepare("SELECT title FROM albums")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(titles, ["Renaissance"], "named as the files name it");
    }
}
