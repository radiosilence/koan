//! Playlists kept as files in the library folders: Navidrome's smart playlist
//! files (`.nsp`), read during scans.
//!
//! Each file becomes a playlist with `source_path` set to it, owned by the
//! implicit user (the first admin on a server) and private, as Navidrome
//! imports them. The file is authoritative for the rules: a scan that finds
//! it changed rewrites the playlist's rules, and one that finds it gone
//! deletes the playlist. A file that does not parse is skipped with a log
//! line, and a playlist already read from it keeps its last good rules.

use std::path::{Path, PathBuf};

use rusqlite::params;

use crate::db::connection::{Database, DbError};
use crate::db::queries::{self, LOCAL_USER, smart};

/// Whether a path is a playlist file a scan reads.
pub fn is_playlist_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("nsp"))
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
