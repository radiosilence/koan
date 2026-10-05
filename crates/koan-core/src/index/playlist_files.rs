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

pub(crate) fn is_m3u(path: &Path) -> bool {
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
/// UTF-16 is taken when the file opens with its byte order mark, as Windows
/// tools write it.
fn decode(bytes: Vec<u8>) -> String {
    let utf16 = |rest: &[u8], unit: fn([u8; 2]) -> u16| {
        let units: Vec<u16> = rest.as_chunks::<2>().0.iter().map(|&c| unit(c)).collect();
        String::from_utf16_lossy(&units)
    };
    match bytes.as_slice() {
        [0xff, 0xfe, rest @ ..] => return utf16(rest, u16::from_le_bytes),
        [0xfe, 0xff, rest @ ..] => return utf16(rest, u16::from_be_bytes),
        _ => {}
    }
    String::from_utf8(bytes).unwrap_or_else(|e| e.into_bytes().iter().map(|&b| b as char).collect())
}

/// Resolve every playlist read from an M3U file again: after tracks were
/// added, entries naming them may now be in the library. A file unchanged
/// that resolves to the same tracks writes nothing.
pub fn refresh_m3u(db: &Database) {
    let sources: Vec<String> = match db
        .conn
        .prepare("SELECT source_path FROM playlists WHERE source_path IS NOT NULL")
        .and_then(|mut stmt| {
            stmt.query_map([], |r| r.get(0))?
                .collect::<Result<Vec<String>, _>>()
        }) {
        Ok(sources) => sources,
        Err(e) => {
            log::warn!("could not list playlist files to resolve again: {e}");
            return;
        }
    };
    for source in sources {
        let path = Path::new(&source);
        if is_m3u(path)
            && path.is_file()
            && let Err(e) = import_m3u(db, path)
        {
            log::warn!("playlist {source} not resolved again: {e}");
        }
    }
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

/// Rewrite an M3U file's entries after files moved: `moved` maps where each
/// was to where it is, and `was_at` is where the list itself was, which its
/// relative entries are read against. Entries naming a moved file are given
/// its new place; relative ones are kept relative where they still can be.
/// Everything else in the file is left as it was. Whether it was rewritten.
///
/// What organize runs after moving files, since an M3U names its tracks by
/// path and the next scan would otherwise read every moved one as missing.
pub(crate) fn follow_moves(
    path: &Path,
    was_at: &Path,
    moved: &std::collections::HashMap<PathBuf, PathBuf>,
) -> std::io::Result<bool> {
    let bytes = std::fs::read(path)?;
    if bytes.starts_with(&[0xff, 0xfe]) || bytes.starts_with(&[0xfe, 0xff]) {
        return Err(std::io::Error::other("UTF-16 playlists are not rewritten"));
    }
    let utf8 = std::str::from_utf8(&bytes).is_ok();
    let text = decode(bytes);
    let old_dir = was_at.parent().unwrap_or(Path::new("/"));
    let new_dir = path.parent().unwrap_or(Path::new("/"));

    let mut changed = false;
    let mut lines = Vec::new();
    for line in text.split('\n') {
        let (body, cr) = match line.strip_suffix('\r') {
            Some(body) => (body, "\r"),
            None => (line, ""),
        };
        let bom = if body.starts_with('\u{feff}') {
            "\u{feff}"
        } else {
            ""
        };
        let entry = body.trim().trim_start_matches('\u{feff}');
        let rewritten = (!entry.is_empty() && !entry.starts_with('#'))
            .then(|| entry_path(entry, old_dir))
            .flatten()
            .and_then(|old| {
                let url = entry.starts_with("file://");
                let relative = !url && !Path::new(&entry.replace('\\', "/")).is_absolute();
                let target = match moved.get(&old) {
                    Some(target) => target.clone(),
                    None if relative && old_dir != new_dir => old,
                    None => return None,
                };
                if url {
                    return url::Url::from_file_path(&target).ok().map(String::from);
                }
                let written = match target.strip_prefix(new_dir) {
                    Ok(inside) if relative => inside,
                    _ => target.as_path(),
                };
                Some(written.to_string_lossy().into_owned())
            });
        match rewritten {
            Some(entry) => {
                changed = true;
                lines.push(format!("{bom}{entry}{cr}"));
            }
            None => lines.push(line.to_owned()),
        }
    }
    if !changed {
        return Ok(false);
    }

    let text = lines.join("\n");
    // A list that was not UTF-8 is written back as the Latin-1 it was read as,
    // where the new paths allow.
    let bytes = match text.chars().map(|c| u8::try_from(c as u32)).collect() {
        Ok(latin1) if !utf8 => latin1,
        _ => text.into_bytes(),
    };
    let temp = path.with_extension("koan-rewrite");
    std::fs::write(&temp, bytes)?;
    if let Ok(meta) = std::fs::metadata(path) {
        let _ = std::fs::set_permissions(&temp, meta.permissions());
    }
    if let Err(e) = std::fs::rename(&temp, path) {
        let _ = std::fs::remove_file(&temp);
        return Err(e);
    }
    Ok(true)
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
        // Only stream URLs: nothing a scan could ever add. Files not in the
        // library yet may be, so a list of them is kept, empty, and resolved
        // again as tracks arrive.
        None if m3u.entries.iter().all(|e| entry_path(e, dir).is_none()) => {
            Err("it lists no files".into())
        }
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
        // A path that cannot be reached is not "gone".
        if settled.iter().any(|dir| path.starts_with(dir)) && super::known_missing(path) {
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
    fn an_m3u_of_streams_only_is_not_imported() {
        let tmp = tempfile::tempdir().unwrap();
        let db = test_db(tmp.path());
        let file = tmp.path().join("Radio.m3u");
        std::fs::write(&file, "http://radio/stream\n").unwrap();
        assert_eq!(import(&db, std::slice::from_ref(&file), &[]), 0);
        assert!(sourced(&db).is_empty());
    }

    /// A list read before the folder it names was indexed fills in once the
    /// tracks arrive.
    #[test]
    fn an_m3u_naming_tracks_not_yet_indexed_fills_in_later() {
        let tmp = tempfile::tempdir().unwrap();
        let db = test_db(tmp.path());
        let music = tmp.path().join("music");
        std::fs::create_dir_all(&music).unwrap();
        let file = music.join("Later.m3u");
        std::fs::write(&file, "Album/A.flac\n").unwrap();

        assert_eq!(import(&db, std::slice::from_ref(&file), &[]), 1);
        let id: i64 = db
            .conn
            .query_row(
                "SELECT id FROM playlists WHERE source_path IS NOT NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            queries::playlist_track_ids(&db.conn, id)
                .unwrap()
                .is_empty()
        );

        let mut meta = queries::sample_meta("A", "Artist", "Album");
        meta.path = Some(music.join("Album/A.flac").to_string_lossy().into_owned());
        let a = queries::upsert_track(&db.conn, &meta).unwrap();
        refresh_m3u(&db);
        assert_eq!(queries::playlist_track_ids(&db.conn, id).unwrap(), vec![a]);
    }

    #[test]
    fn utf16_with_a_byte_order_mark_is_read() {
        let le: Vec<u8> = [0xff, 0xfe]
            .into_iter()
            .chain("Bébé\n".encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        assert_eq!(decode(le), "Bébé\n");
        let be: Vec<u8> = [0xfe, 0xff]
            .into_iter()
            .chain("Bébé".encode_utf16().flat_map(u16::to_be_bytes))
            .collect();
        assert_eq!(decode(be), "Bébé");
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
