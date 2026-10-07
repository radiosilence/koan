//! Index maintenance: rebuilding, counting and forgetting what a folder or the server put in the library.

use std::path::Path;

use crate::config::Config;
use crate::db::connection::Database;
use crate::db::queries;

use super::*;

/// What a library rebuild re-reads.
#[derive(Debug, Clone, Copy, Default)]
pub struct RebuildSummary {
    pub tracks: u64,
    pub albums: u64,
    pub artists: u64,
}

/// Read the whole library again from its sources.
///
/// What each file and server entry said is forgotten, so the next scan reads
/// every file and the next sync walks the whole server, and every track,
/// album and artist takes what its sources say now. The rows themselves stay,
/// and each source takes its own back by path or server id, so play history,
/// playlists, favourites and lyrics are kept. A row nothing claims again goes:
/// a file's when the scan of its folder finds it missing, a server entry's
/// when a complete sync does not list it.
pub fn rebuild_index(db: &Database) -> Result<RebuildSummary, crate::db::connection::DbError> {
    let count = |sql: &str| -> u64 {
        db.conn
            .query_row(sql, [], |r| r.get::<_, i64>(0))
            .unwrap_or(0) as u64
    };
    let summary = RebuildSummary {
        tracks: count("SELECT COUNT(*) FROM tracks"),
        albums: count("SELECT COUNT(*) FROM albums"),
        artists: count("SELECT COUNT(*) FROM artists"),
    };

    db.conn.execute_batch(
        "BEGIN;
         DELETE FROM local_files;
         DELETE FROM remote_entries;
         DELETE FROM scan_cache;
         UPDATE remote_servers SET library_version = NULL;
         COMMIT;",
    )?;
    Ok(summary)
}

/// How many tracks came from this folder.
///
/// The trailing separator matters: without it `/Volumes/Music` also counts
/// `/Volumes/Music Backup`.
pub fn tracks_under(db: &Database, folder: &Path) -> u64 {
    let (lower, upper) = queries::folder_prefix_range(&crate::index::spelling::on_disk(folder));
    db.conn
        .query_row(
            "SELECT COUNT(*) FROM tracks WHERE path >= ?1 AND path < ?2",
            [&lower, &upper],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0) as u64
}

/// How many tracks the server accounts for.
pub fn tracks_from_server(db: &Database) -> u64 {
    db.conn
        .query_row(
            "SELECT COUNT(*) FROM tracks WHERE remote_id IS NOT NULL",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0) as u64
}

/// Forget every track under a folder.
///
/// Removing a folder from the library should remove what it put there —
/// otherwise the library keeps showing records whose files it will never look
/// at again, and there is no way back to an empty library short of clearing the
/// whole index.
///
/// A track that also exists on the server keeps its row and loses only its local
/// path: it is still playable, just by download rather than from disk.
///
/// Albums and artists left holding nothing go too, or the browser fills with
/// empty shelves.
pub fn forget_folder(db: &Database, folder: &Path) -> Result<u64, crate::db::connection::DbError> {
    // Rows are keyed by the disk's spelling; a folder named the other way would forget nothing.
    let folder = &crate::index::spelling::on_disk(folder);
    let (lower, upper) = queries::folder_prefix_range(folder);
    // A scan of it underway would index again what this forgets: stop it, and
    // let it finish committing before anything goes.
    crate::index::lane::cancel_under(folder);
    let _lane = crate::index::lane::wait();

    let tx = crate::db::queries::write_transaction(&db.conn)?;
    // The folder's files, and the tracks a rebuilt index has not yet re-read
    // from it.
    let tracks: Vec<i64> = {
        let mut stmt = tx.prepare(
            "SELECT track_id FROM local_files WHERE path >= ?1 AND path < ?2
             UNION
             SELECT t.id FROM tracks t WHERE t.path >= ?1 AND t.path < ?2
                AND NOT EXISTS (SELECT 1 FROM local_files f WHERE f.track_id = t.id)",
        )?;
        let rows = stmt.query_map([&lower, &upper], |r| r.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    // A track also on the server keeps its row, minus the file.
    queries::sources::forget_tracks(&tx, &tracks, queries::sources::Forget::Demote)?;
    tx.commit()?;
    Ok(tracks.len() as u64)
}

/// Forget everything that only existed on the server.
///
/// Signing out should leave the library with what is actually on this machine.
/// A track held both locally and remotely keeps its row and loses the server's
/// copy; one that only ever came from the server goes.
pub fn forget_remote(db: &Database) -> Result<u64, crate::db::connection::DbError> {
    let tx = crate::db::queries::write_transaction(&db.conn)?;
    let ids: Vec<String> = {
        let mut stmt = tx.prepare("SELECT remote_id FROM remote_entries")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut removed = 0;
    for id in &ids {
        let track: i64 = tx.query_row(
            "SELECT track_id FROM remote_entries WHERE remote_id = ?1",
            [id],
            |r| r.get(0),
        )?;
        queries::sources::remove(&tx, queries::sources::Kind::Remote, id)?;
        let kept: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM tracks WHERE id = ?1)",
            [track],
            |r| r.get(0),
        )?;
        removed += u64::from(!kept);
    }
    // What waited for this server, and how far its history and the
    // account's EQ profiles were read. The profiles themselves stay.
    tx.execute_batch(
        "DELETE FROM history_outbox;
         DELETE FROM favourite_outbox;
         UPDATE remote_servers SET history_cursor = NULL;
         DELETE FROM dsp_synced;
         DELETE FROM dsp_sync_cursor;",
    )?;
    tx.commit()?;
    Ok(removed)
}

#[cfg(test)]
mod rebuild_tests {
    use super::*;
    use crate::db::queries::sample_meta;

    fn test_db() -> Database {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        Database { conn }
    }

    #[test]
    fn cached_paths_follow_a_moved_cache_directory() {
        let old = tempfile::tempdir().unwrap();
        let new = tempfile::tempdir().unwrap();
        let db = test_db();

        let mut rows = Vec::new();
        for name in ["moved", "gone", "current"] {
            let mut meta = sample_meta(name, "Artist", "Album");
            meta.source = "remote".into();
            meta.path = None;
            meta.remote_id = Some(name.into());
            let id = queries::upsert_track(&db.conn, &meta).unwrap();
            let tail = format!("Artist/Album/{name}.flac");
            // Every copy now lives under the new directory except "gone".
            if name != "gone" {
                let file = new.path().join(&tail);
                std::fs::create_dir_all(file.parent().unwrap()).unwrap();
                std::fs::write(&file, b"audio").unwrap();
            }
            let stored = if name == "current" {
                new.path()
            } else {
                old.path()
            }
            .join(&tail);
            queries::set_cached_path(&db.conn, id, &stored.to_string_lossy()).unwrap();
            rows.push((id, tail));
        }

        assert_eq!(relocate_cached_paths(&db, new.path()).unwrap(), 1);

        let cached = |id: i64| -> String {
            db.conn
                .query_row("SELECT cached_path FROM tracks WHERE id = ?1", [id], |r| {
                    r.get(0)
                })
                .unwrap()
        };
        let expect = |root: &Path, tail: &str| root.join(tail).to_string_lossy().into_owned();
        assert_eq!(
            cached(rows[0].0),
            expect(new.path(), &rows[0].1),
            "re-rooted"
        );
        assert_eq!(
            cached(rows[1].0),
            expect(old.path(), &rows[1].1),
            "no file, left alone"
        );
        assert_eq!(
            cached(rows[2].0),
            expect(new.path(), &rows[2].1),
            "already current"
        );
        assert_eq!(
            relocate_cached_paths(&db, new.path()).unwrap(),
            0,
            "idempotent"
        );
    }

    #[test]
    fn clearing_one_download_leaves_the_others_and_the_library_alone() {
        let dir = tempfile::tempdir().unwrap();
        let db = test_db();

        let mut cached = Vec::new();
        for name in ["one", "two"] {
            let mut meta = sample_meta(name, "Artist", "Album");
            meta.source = "remote".into();
            meta.path = None;
            meta.remote_id = Some(name.into());
            let id = queries::upsert_track(&db.conn, &meta).unwrap();
            let file = dir.path().join(format!("{name}.opus"));
            std::fs::write(&file, vec![0u8; 2048]).unwrap();
            queries::set_cached_path(&db.conn, id, &file.to_string_lossy()).unwrap();
            cached.push((id, file));
        }

        let cleared = clear_downloads_for(&db, &[cached[0].0]);
        assert_eq!(cleared.files, 1);
        assert_eq!(cleared.bytes, 2048);
        assert!(!cached[0].1.exists(), "the copy asked for is gone");
        assert!(cached[1].1.exists(), "the other one is untouched");

        // The row survives — a remote track is still in the library, it just
        // has to be fetched again.
        assert_eq!(queries::library_stats(&db.conn).unwrap().remote_tracks, 2);
        assert_eq!(queries::library_stats(&db.conn).unwrap().cached_tracks, 1);
        assert!(
            queries::cached_paths_for(&db.conn, &[cached[0].0])
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn clearing_a_download_that_is_already_gone_is_not_a_failure() {
        let db = test_db();
        let mut meta = sample_meta("ghost", "Artist", "Album");
        meta.source = "remote".into();
        meta.path = None;
        meta.remote_id = Some("ghost".into());
        let id = queries::upsert_track(&db.conn, &meta).unwrap();
        queries::set_cached_path(&db.conn, id, "/nowhere/at/all.opus").unwrap();

        let cleared = clear_downloads_for(&db, &[id]);
        assert_eq!(cleared.files, 0, "nothing was there to remove");
        // Forgotten regardless: the row claimed a copy that does not exist.
        assert!(
            queries::cached_paths_for(&db.conn, &[id])
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn sweeping_removes_half_finished_downloads_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache");
        std::fs::create_dir_all(cache.join("Artist")).unwrap();

        let finished = cache.join("Artist/whole.opus");
        let half = cache.join("Artist/half.opus.part");
        std::fs::write(&finished, vec![0u8; 1024]).unwrap();
        std::fs::write(&half, vec![0u8; 4096]).unwrap();

        let cfg = Config {
            remote: crate::config::RemoteConfig {
                cache_dir: Some(cache.clone()),
                ..Default::default()
            },
            ..Default::default()
        };

        let swept = sweep_partial_downloads(&cfg);
        assert_eq!(swept.files, 1);
        assert_eq!(swept.bytes, 4096);
        assert!(!half.exists(), "the unfinished one is gone");
        assert!(finished.exists(), "a downloaded track is not touched");
    }

    /// The figure Settings shows and what a clear reports are one walk of
    /// one directory, so they cannot disagree.
    #[test]
    fn the_cache_is_measured_as_a_clear_counts_it() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache");
        std::fs::create_dir_all(cache.join("Artist/Album")).unwrap();
        std::fs::write(cache.join("Artist/Album/01.flac"), vec![0u8; 3000]).unwrap();
        std::fs::write(cache.join("Artist/Album/02.flac"), vec![0u8; 5000]).unwrap();
        let cfg = Config {
            remote: crate::config::RemoteConfig {
                cache_dir: Some(cache.clone()),
                ..Default::default()
            },
            ..Default::default()
        };
        let db = Database::open(&dir.path().join("koan.db")).unwrap();
        assert_eq!(measure_cache(&cfg), 8000);
        let cleared = clear_download_cache(&db, &cfg);
        assert_eq!((cleared.files, cleared.bytes), (2, 8000));
        assert_eq!(measure_cache(&cfg), 0, "nothing left to count");
    }

    #[test]
    fn sweeping_an_empty_cache_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = Config {
            remote: crate::config::RemoteConfig {
                cache_dir: Some(dir.path().join("nothing-here")),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(sweep_partial_downloads(&cfg).files, 0);
    }

    #[test]
    fn clearing_no_tracks_does_nothing() {
        let db = test_db();
        assert_eq!(clear_downloads_for(&db, &[]).files, 0);
    }

    #[test]
    fn a_rebuild_re_reads_the_library_into_the_rows_it_has() {
        let db = test_db();
        let mut meta = sample_meta("Windowlicker", "Aphex Twin", "Windowlicker EP");
        meta.path = Some("/music/windowlicker.flac".into());
        let track_id = queries::upsert_track(&db.conn, &meta).unwrap();

        queries::toggle_favourite(&db.conn, crate::db::queries::LOCAL_USER, track_id).unwrap();
        db.conn
            .execute(
                "INSERT INTO lyrics_cache (track_id, source, content, fetched_at)
                 VALUES (?1, 'test', 'la la la', 0)",
                [track_id],
            )
            .unwrap();

        let summary = rebuild_index(&db).unwrap();
        assert_eq!(summary.tracks, 1);
        assert_eq!(summary.albums, 1);
        let count = |sql: &str| -> i64 { db.conn.query_row(sql, [], |r| r.get(0)).unwrap() };
        assert_eq!(
            count("SELECT COUNT(*) FROM local_files"),
            0,
            "every file is read again"
        );

        // The scan reads the file again and takes its row back.
        assert_eq!(queries::upsert_track(&db.conn, &meta).unwrap(), track_id);
        assert_eq!(count("SELECT COUNT(*) FROM tracks"), 1);
        assert_eq!(count("SELECT COUNT(*) FROM favourites"), 1);
        assert_eq!(count("SELECT COUNT(*) FROM lyrics_cache"), 1);
    }

    #[test]
    fn a_rebuilt_file_that_is_gone_goes_with_its_folder_scan() {
        let db = test_db();
        let tmp = tempfile::tempdir().unwrap();
        let mut meta = sample_meta("Windowlicker", "Aphex Twin", "Windowlicker");
        meta.path = Some(tmp.path().join("gone.flac").to_string_lossy().into_owned());
        queries::upsert_track(&db.conn, &meta).unwrap();
        rebuild_index(&db).unwrap();

        queries::remove_stale_tracks(&db.conn, tmp.path(), false).unwrap();
        let tracks: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(tracks, 0);
    }

    #[test]
    fn rebuilding_an_empty_library_is_not_an_error() {
        let db = test_db();
        let summary = rebuild_index(&db).unwrap();
        assert_eq!(summary.tracks, 0);
    }
}
