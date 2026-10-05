//! Playlists kept as files in the library folders, read during scans:
//! Navidrome's smart playlist files (`.nsp`) and M3U lists (`.m3u`, `.m3u8`).
//!
//! Each file becomes a playlist with `source_path` set to it, owned by the
//! implicit user (the first admin on a server) and private, as Navidrome
//! imports them. The file is authoritative: a scan that finds it changed
//! rewrites the playlist (its rules, or its tracks), one that finds it gone
//! deletes the playlist, and the playlist's contents take no edits. A file
//! that does not parse is skipped with a log line, and a playlist already
//! read from it keeps what it had.

use std::path::{Path, PathBuf};

use rusqlite::params;

use crate::db::connection::{Database, DbError};
use crate::db::queries::{self, LOCAL_USER, smart};

/// Whether a path is a playlist file a scan reads.
pub fn is_playlist_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| {
            ["nsp", "m3u", "m3u8"]
                .iter()
                .any(|k| ext.eq_ignore_ascii_case(k))
        })
}

fn is_m3u(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("m3u") || ext.eq_ignore_ascii_case("m3u8"))
}

/// Read the playlist files found by a scan, and drop playlists whose file is
/// gone from under `settled`: directories the scan covered completely and
/// found readable. Returns how many playlists were created or changed.
pub fn import(db: &Database, files: &[PathBuf], settled: &[PathBuf]) -> usize {
    let mut changed = 0;
    for path in files {
        match import_one(db, path) {
            Ok(true) => changed += 1,
            Ok(false) => {}
            Err(e) => log::warn!("playlist {} not imported: {e}", path.display()),
        }
    }
    if let Err(e) = forget_missing(db, settled) {
        log::warn!("could not check for removed playlist files: {e}");
    }
    changed
}

fn import_one(db: &Database, path: &Path) -> Result<bool, String> {
    if is_m3u(path) {
        return import_m3u(db, path);
    }
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let nsp = crate::smart::from_nsp(&text)?;
    let source = path.to_string_lossy();
    let name = nsp.name.unwrap_or_else(|| {
        path.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| source.to_string())
    });
    let conn = &db.conn;
    let existing: Option<(i64, String, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT id, name, comment, rules FROM playlists WHERE source_path = ?1",
            params![source],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .ok();
    let db_err = |e: DbError| e.to_string();
    match existing {
        Some((id, old_name, old_comment, old_rules)) => {
            let same_rules = old_rules
                .as_deref()
                .and_then(|r| crate::smart::Rules::parse(r).ok())
                .is_some_and(|r| r == nsp.rules);
            if same_rules && old_name == name && old_comment == nsp.comment {
                return Ok(false);
            }
            queries::atomically(conn, || {
                conn.execute(
                    "UPDATE playlists SET name = ?2, comment = ?3 WHERE id = ?1",
                    params![id, name, nsp.comment],
                )?;
                smart::set_rules(conn, id, Some(&nsp.rules))
            })
            .map_err(db_err)?;
        }
        None => {
            queries::atomically(conn, || {
                let id = smart::create_smart_playlist(
                    conn,
                    LOCAL_USER,
                    &name,
                    nsp.comment.as_deref(),
                    &nsp.rules,
                )?;
                conn.execute(
                    "UPDATE playlists SET source_path = ?2 WHERE id = ?1",
                    params![id, source],
                )?;
                Ok::<_, DbError>(())
            })
            .map_err(db_err)?;
            log::info!("imported smart playlist '{name}' from {}", path.display());
        }
    }
    Ok(true)
}

/// What an M3U file says: a name, if it gives one, and its entries as written.
#[derive(Debug, PartialEq)]
struct M3u {
    name: Option<String>,
    entries: Vec<String>,
}

/// Lines that are not comments are entries; `#PLAYLIST:` names the list.
/// Extended M3U's `#EXTINF` and the rest are comments as far as this goes:
/// the track's own tags describe it better.
fn parse_m3u(text: &str) -> M3u {
    let mut name = None;
    let mut entries = Vec::new();
    for line in text.lines() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if let Some(n) = line.strip_prefix("#PLAYLIST:") {
            name = Some(n.trim().to_owned()).filter(|n| !n.is_empty());
        } else if !line.is_empty() && !line.starts_with('#') {
            entries.push(line.to_owned());
        }
    }
    M3u { name, entries }
}

/// `.m3u8` is UTF-8 by definition; plain `.m3u` is often Latin-1, from the
/// players that first wrote it. Valid UTF-8 is taken as such either way.
fn decode(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).unwrap_or_else(|e| e.into_bytes().iter().map(|&b| b as char).collect())
}

/// The file an entry names, if it names one: absolute, relative to the
/// playlist's directory, or a `file://` URL. Stream URLs name none. Windows
/// separators are read as separators.
fn entry_path(entry: &str, dir: &Path) -> Option<PathBuf> {
    let path = if entry.starts_with("file://") {
        url::Url::parse(entry).ok()?.to_file_path().ok()?
    } else if entry.contains("://") {
        return None;
    } else {
        let entry = entry.replace('\\', "/");
        dir.join(entry)
    };
    // Lexically, as written: `..` steps out of the directory named, whatever
    // a symlink along the way points at.
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            c => out.push(c.as_os_str()),
        }
    }
    Some(out)
}

fn import_m3u(db: &Database, path: &Path) -> Result<bool, String> {
    let m3u = parse_m3u(&decode(std::fs::read(path).map_err(|e| e.to_string())?));
    let dir = path.parent().unwrap_or(Path::new("/"));
    let source = path.to_string_lossy();
    let name = m3u.name.unwrap_or_else(|| {
        path.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| source.to_string())
    });

    // Tracks are stored as the directory spells their paths; a list written
    // elsewhere may spell an accent the other way.
    let mut spelling = super::spelling::Spelling::default();
    let conn = &db.conn;
    let db_err = |e: DbError| e.to_string();
    let mut tracks = Vec::with_capacity(m3u.entries.len());
    let mut missing = 0;
    for entry in &m3u.entries {
        let found = match entry_path(entry, dir) {
            Some(p) => {
                let p = spelling.on_disk(&p);
                queries::track_id_by_path(conn, &p.to_string_lossy()).map_err(db_err)?
            }
            None => None,
        };
        match found {
            Some(id) => tracks.push(id),
            None => missing += 1,
        }
    }
    if missing > 0 {
        log::info!(
            "{}: {missing} of {} entries are not in the library",
            path.display(),
            m3u.entries.len()
        );
    }

    let existing: Option<(i64, String)> = conn
        .query_row(
            "SELECT id, name FROM playlists WHERE source_path = ?1",
            params![source],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    match existing {
        Some((id, old_name)) => queries::atomically(conn, || {
            let renamed = old_name != name && queries::rename_playlist(conn, id, &name)?;
            let refilled = queries::replace_entries(conn, id, &tracks)?;
            Ok::<_, DbError>(renamed || refilled)
        })
        .map_err(db_err),
        None if tracks.is_empty() => Err(format!(
            "none of its {} entries are in the library",
            m3u.entries.len()
        )),
        None => {
            queries::atomically(conn, || {
                let id = queries::create_playlist(conn, LOCAL_USER, &name, None)?;
                conn.execute(
                    "UPDATE playlists SET source_path = ?2 WHERE id = ?1",
                    params![id, source],
                )?;
                queries::set_playlist_tracks(conn, id, &tracks)
            })
            .map_err(db_err)?;
            log::info!("imported playlist '{name}' from {}", path.display());
            Ok(true)
        }
    }
}

/// Delete playlists read from a file under `settled` that no longer exists.
fn forget_missing(db: &Database, settled: &[PathBuf]) -> Result<(), DbError> {
    if settled.is_empty() {
        return Ok(());
    }
    let sourced: Vec<(i64, String)> = db
        .conn
        .prepare("SELECT id, source_path FROM playlists WHERE source_path IS NOT NULL")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    for (id, source) in sourced {
        let path = Path::new(&source);
        // `try_exists` errs when it cannot tell, which is not "gone".
        if settled.iter().any(|dir| path.starts_with(dir)) && matches!(path.try_exists(), Ok(false))
        {
            queries::delete_playlist(&db.conn, id)?;
            log::info!("{source} is gone; deleted its playlist");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db(dir: &Path) -> Database {
        Database::open(&dir.join("test.db")).unwrap()
    }

    fn sourced(db: &Database) -> Vec<(String, Option<String>)> {
        db.conn
            .prepare("SELECT name, rules FROM playlists WHERE source_path IS NOT NULL")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    #[test]
    fn a_file_becomes_a_smart_playlist_and_follows_its_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let db = test_db(tmp.path());
        let music = tmp.path().join("music");
        std::fs::create_dir_all(&music).unwrap();
        let file = music.join("Loved.nsp");
        std::fs::write(&file, r#"{"all":[{"is":{"loved":true}}]}"#).unwrap();

        assert_eq!(
            import(
                &db,
                std::slice::from_ref(&file),
                std::slice::from_ref(&music)
            ),
            1
        );
        let rows = sourced(&db);
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].0, "Loved",
            "named after the file when it names nothing"
        );
        assert_eq!(
            import(&db, std::slice::from_ref(&file), &[]),
            0,
            "unchanged"
        );

        std::fs::write(
            &file,
            r#"{"name":"Most played","all":[{"gt":{"playcount":3}}]}"#,
        )
        .unwrap();
        assert_eq!(import(&db, std::slice::from_ref(&file), &[]), 1);
        let rows = sourced(&db);
        assert_eq!(rows.len(), 1, "the same playlist, rewritten");
        assert_eq!(rows[0].0, "Most played");
        assert!(rows[0].1.as_deref().unwrap().contains("playCount"));

        std::fs::write(&file, r#"{"all":[{"gt":{"rating":3}}]}"#).unwrap();
        assert_eq!(import(&db, std::slice::from_ref(&file), &[]), 0);
        assert_eq!(
            sourced(&db)[0].0,
            "Most played",
            "a bad file keeps the last good rules"
        );

        std::fs::remove_file(&file).unwrap();
        import(&db, &[], std::slice::from_ref(&music));
        assert!(sourced(&db).is_empty(), "gone with its file");
    }

    #[test]
    fn m3u_entries_are_comments_aside_and_names_optional() {
        let m3u = parse_m3u(
            "\u{feff}#EXTM3U\n#PLAYLIST: Late\n#EXTINF:240,Burial - Archangel\nBurial/Untrue/02.flac\n\n  http://radio/stream \n",
        );
        assert_eq!(m3u.name.as_deref(), Some("Late"));
        assert_eq!(
            m3u.entries,
            ["Burial/Untrue/02.flac", "http://radio/stream"]
        );
    }

    #[test]
    fn entries_resolve_relative_absolute_url_and_windows() {
        let dir = Path::new("/music/Playlists");
        assert_eq!(
            entry_path("../Burial/Untrue/02.flac", dir),
            Some(PathBuf::from("/music/Burial/Untrue/02.flac"))
        );
        assert_eq!(
            entry_path("..\\Burial\\02.flac", dir),
            Some(PathBuf::from("/music/Burial/02.flac"))
        );
        assert_eq!(
            entry_path("/elsewhere/a.mp3", dir),
            Some(PathBuf::from("/elsewhere/a.mp3"))
        );
        assert_eq!(
            entry_path("file:///music/A%20B/c.flac", dir),
            Some(PathBuf::from("/music/A B/c.flac"))
        );
        assert_eq!(entry_path("https://example.com/a.mp3", dir), None);
    }

    #[test]
    fn latin1_m3u_is_read() {
        assert_eq!(decode(vec![b'B', 0xe9, b'b', b'e']), "Bébe");
        assert_eq!(decode("Bébé".as_bytes().to_vec()), "Bébé");
    }

    #[test]
    fn an_m3u_becomes_a_playlist_that_follows_its_file() {
        let tmp = tempfile::tempdir().unwrap();
        let db = test_db(tmp.path());
        let music = tmp.path().join("music");
        std::fs::create_dir_all(music.join("lists")).unwrap();
        let track = |name: &str| {
            let mut meta = queries::sample_meta(name, "Artist", "Album");
            meta.path = Some(
                music
                    .join(format!("{name}.flac"))
                    .to_string_lossy()
                    .into_owned(),
            );
            queries::upsert_track(&db.conn, &meta).unwrap()
        };
        let (a, b) = (track("A"), track("B"));
        let file = music.join("lists").join("Mix.m3u8");
        std::fs::write(&file, "#EXTM3U\n../B.flac\n../missing.flac\n../A.flac\n").unwrap();

        assert_eq!(import(&db, std::slice::from_ref(&file), &[]), 1);
        let id: i64 = db
            .conn
            .query_row(
                "SELECT id FROM playlists WHERE source_path IS NOT NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            queries::playlist_track_ids(&db.conn, id).unwrap(),
            vec![b, a]
        );
        let row = queries::get_playlist(&db.conn, id).unwrap().unwrap();
        assert_eq!(row.name, "Mix");
        assert!(row.readonly, "the file decides what it holds");
        assert!(row.rules.is_none());

        assert_eq!(
            import(&db, std::slice::from_ref(&file), &[]),
            0,
            "unchanged"
        );
        std::fs::write(&file, "#PLAYLIST:Renamed\n../A.flac\n").unwrap();
        assert_eq!(import(&db, std::slice::from_ref(&file), &[]), 1);
        assert_eq!(queries::playlist_track_ids(&db.conn, id).unwrap(), vec![a]);
        assert_eq!(
            queries::get_playlist(&db.conn, id).unwrap().unwrap().name,
            "Renamed"
        );
    }

    #[test]
    fn an_m3u_naming_nothing_in_the_library_is_not_imported() {
        let tmp = tempfile::tempdir().unwrap();
        let db = test_db(tmp.path());
        let file = tmp.path().join("Radio.m3u");
        std::fs::write(&file, "http://radio/stream\n").unwrap();
        assert_eq!(import(&db, std::slice::from_ref(&file), &[]), 0);
        assert!(sourced(&db).is_empty());
    }

    #[test]
    fn a_file_outside_the_settled_directories_is_not_forgotten() {
        let tmp = tempfile::tempdir().unwrap();
        let db = test_db(tmp.path());
        let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let file = a.join("All.nsp");
        std::fs::write(&file, r#"{"all":[]}"#).unwrap();
        import(&db, std::slice::from_ref(&file), &[]);
        std::fs::remove_file(&file).unwrap();

        import(&db, &[], std::slice::from_ref(&b));
        assert_eq!(sourced(&db).len(), 1);
    }
}
