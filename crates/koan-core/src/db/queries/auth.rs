//! Accounts and sessions, from `koan-auth`, and whose data kōan's implicit
//! user's is.

use rusqlite::{Connection, params};

pub use koan_auth::sessions::{
    OAuthGrant, RefreshTokenRow, cleanup_expired_tokens, consume_refresh_token,
    get_valid_refresh_token, revoke_all_user_tokens, revoke_grant, revoke_refresh_token,
    revoke_replayed_grant, store_grant_token, store_refresh_token,
};
pub use koan_auth::users::{
    UserRow, admin_count, delete_user, get_user_by_id, get_user_by_username, has_users, list_users,
    update_password,
};

use crate::auth::Role;

// ---------------------------------------------------------------------------
// Whose data
// ---------------------------------------------------------------------------

/// The user a caller with no account acts as: the macOS app, the TUI, a server
/// with auth disabled, the Subsonic shared secret.
///
/// Favourites, playlists and play history are per user. An install with no
/// admin account keeps them under this id; once there is one, the first admin
/// owns them and this id [resolves](resolve_user) to theirs, so a single-user
/// server and a local library behave the same.
pub const LOCAL_USER: i64 = 0;

/// The first admin account, which answers for [`LOCAL_USER`].
pub fn first_admin(conn: &Connection) -> Result<Option<i64>, rusqlite::Error> {
    conn.prepare_cached("SELECT MIN(id) FROM users WHERE role = 'admin'")?
        .query_row([], |r| r.get(0))
}

/// The id whose rows `user` reads and writes: `user` itself for an account,
/// the first admin (or [`LOCAL_USER`] while there is none) for the implicit user.
pub fn resolve_user(conn: &Connection, user: i64) -> Result<i64, rusqlite::Error> {
    if user != LOCAL_USER {
        return Ok(user);
    }
    Ok(first_admin(conn)?.unwrap_or(LOCAL_USER))
}

/// Whether `user` is the one [`LOCAL_USER`] resolves to: whose favourites and
/// playlists this koan syncs with an upstream server.
pub fn is_local_user(conn: &Connection, user: i64) -> Result<bool, rusqlite::Error> {
    Ok(resolve_user(conn, user)? == resolve_user(conn, LOCAL_USER)?)
}

/// Hand the implicit user's rows to the first admin, once there is one.
///
/// Where a row would duplicate one the admin already has, theirs is kept.
pub fn adopt_local_rows(conn: &Connection) -> Result<(), rusqlite::Error> {
    let Some(admin) = first_admin(conn)? else {
        return Ok(());
    };
    for table in [
        "favourites",
        "favourite_albums",
        "favourite_artists",
        "track_ratings",
        "album_ratings",
        "artist_ratings",
        "bookmarks",
        "play_history",
        "play_history_forgotten",
        "playlists",
        "shares",
    ] {
        // Runs on every open: a read, so it takes no write lock when there is
        // nothing to hand over.
        let pending: bool = conn.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE user_id = ?1)"),
            params![LOCAL_USER],
            |r| r.get(0),
        )?;
        if !pending {
            continue;
        }
        conn.execute(
            &format!("UPDATE OR IGNORE {table} SET user_id = ?1 WHERE user_id = ?2"),
            params![admin, LOCAL_USER],
        )?;
        conn.execute(
            &format!("DELETE FROM {table} WHERE user_id = ?1"),
            params![LOCAL_USER],
        )?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Accounts that change whose data the implicit user's is
// ---------------------------------------------------------------------------

/// Create a new user. Returns the user ID.
pub fn create_user(
    conn: &Connection,
    username: &str,
    password: &str,
    role: Role,
) -> Result<i64, rusqlite::Error> {
    let id = koan_auth::users::create_user(conn, username, password, role)?;
    adopt_local_rows(conn)?;
    Ok(id)
}

/// Update a user's role.
pub fn update_role(conn: &Connection, username: &str, role: Role) -> Result<bool, rusqlite::Error> {
    let updated = koan_auth::users::update_role(conn, username, role)?;
    adopt_local_rows(conn)?;
    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::Database;
    use tempfile::TempDir;

    fn test_db() -> (Database, TempDir) {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test.db");
        let db = Database::open(&db_path).unwrap();
        (db, tmp)
    }

    // -- Per-user data ------------------------------------------------------

    use crate::db::queries::{self, sample_meta, upsert_track};

    fn count(db: &Database, sql: &str) -> i64 {
        db.conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn two_users_star_the_same_track_independently() {
        let (db, _tmp) = test_db();
        let admin = create_user(&db.conn, "owner", "pw", Role::Admin).unwrap();
        let mate = create_user(&db.conn, "mate", "pw", Role::User).unwrap();
        let track = upsert_track(&db.conn, &sample_meta("Scatology", "Coil", "Scatology")).unwrap();
        let album = queries::get_track_row(&db.conn, track)
            .unwrap()
            .unwrap()
            .album_id
            .unwrap();

        queries::add_favourite(&db.conn, admin, track).unwrap();
        queries::add_favourite(&db.conn, mate, track).unwrap();
        queries::remove_favourite(&db.conn, admin, track).unwrap();

        assert!(
            queries::load_favourites(&db.conn, admin)
                .unwrap()
                .is_empty()
        );
        assert!(
            queries::load_favourites(&db.conn, mate)
                .unwrap()
                .contains(&track)
        );
        assert!(queries::toggle_favourite_album(&db.conn, mate, album).unwrap());
        assert!(queries::toggle_favourite_album(&db.conn, admin, album).unwrap());
        assert_eq!(count(&db, "SELECT COUNT(*) FROM favourite_albums"), 2);
    }

    #[test]
    fn the_local_user_is_the_first_admin_once_there_is_one() {
        let (db, _tmp) = test_db();
        let track = upsert_track(&db.conn, &sample_meta("T", "A", "B")).unwrap();
        queries::add_favourite(&db.conn, LOCAL_USER, track).unwrap();
        queries::record_play(&db.conn, LOCAL_USER, track, None).unwrap();
        let list = queries::create_playlist(&db.conn, LOCAL_USER, "Mine", None).unwrap();
        assert_eq!(resolve_user(&db.conn, LOCAL_USER).unwrap(), LOCAL_USER);

        create_user(&db.conn, "mate", "pw", Role::User).unwrap();
        assert_eq!(resolve_user(&db.conn, LOCAL_USER).unwrap(), LOCAL_USER);
        let admin = create_user(&db.conn, "owner", "pw", Role::Admin).unwrap();

        assert_eq!(resolve_user(&db.conn, LOCAL_USER).unwrap(), admin);
        assert!(
            queries::load_favourites(&db.conn, admin)
                .unwrap()
                .contains(&track)
        );
        assert_eq!(queries::play_count(&db.conn, admin, track).unwrap(), 1);
        assert_eq!(
            queries::get_playlist(&db.conn, list)
                .unwrap()
                .unwrap()
                .user_id,
            admin
        );
        assert_eq!(
            count(&db, "SELECT COUNT(*) FROM favourites WHERE user_id = 0"),
            0
        );
    }

    #[test]
    fn playlists_are_the_owners_plus_everyones_public_ones() {
        let (db, _tmp) = test_db();
        let admin = create_user(&db.conn, "owner", "pw", Role::Admin).unwrap();
        let mate = create_user(&db.conn, "mate", "pw", Role::User).unwrap();
        let private = queries::create_playlist(&db.conn, admin, "Private", None).unwrap();
        let public = queries::create_playlist(&db.conn, admin, "Public", None).unwrap();
        db.conn
            .execute("UPDATE playlists SET public = 1 WHERE id = ?1", [public])
            .unwrap();
        let own = queries::create_playlist(&db.conn, mate, "Mate's", None).unwrap();

        let ids = |user| -> Vec<i64> {
            let mut ids: Vec<i64> = queries::list_playlists(&db.conn, user)
                .unwrap()
                .into_iter()
                .map(|p| p.id)
                .collect();
            ids.sort_unstable();
            ids
        };
        assert_eq!(ids(mate), vec![public, own]);
        assert_eq!(ids(admin), vec![private, public]);
        // The implicit user is the first admin.
        assert_eq!(ids(LOCAL_USER), vec![private, public]);

        let row = queries::get_playlist(&db.conn, public).unwrap().unwrap();
        assert!(row.readable_by(mate) && !row.editable_by(mate));
        assert_eq!(row.owner.as_deref(), Some("owner"));
        let row = queries::get_playlist(&db.conn, private).unwrap().unwrap();
        assert!(!row.readable_by(mate));
    }

    #[test]
    fn deleting_an_account_takes_its_data_with_it() {
        let (db, _tmp) = test_db();
        let admin = create_user(&db.conn, "owner", "pw", Role::Admin).unwrap();
        let mate = create_user(&db.conn, "mate", "pw", Role::User).unwrap();
        let track = upsert_track(&db.conn, &sample_meta("T", "A", "B")).unwrap();
        let row = queries::get_track_row(&db.conn, track).unwrap().unwrap();
        for user in [admin, mate] {
            queries::add_favourite(&db.conn, user, track).unwrap();
            queries::set_favourite_album(&db.conn, user, row.album_id.unwrap(), true).unwrap();
            queries::set_favourite_artist(&db.conn, user, row.artist_id.unwrap(), true).unwrap();
            for (kind, id) in [
                (queries::RatingKind::Track, track),
                (queries::RatingKind::Album, row.album_id.unwrap()),
                (queries::RatingKind::Artist, row.artist_id.unwrap()),
            ] {
                queries::set_rating(&db.conn, user, kind, id, 4).unwrap();
            }
            queries::save_bookmark(&db.conn, user, track, 1_000, None).unwrap();
            queries::record_play(&db.conn, user, track, None).unwrap();
            queries::record_play_at(&db.conn, user, track, 1, None, queries::SOURCE_SUBSONIC)
                .unwrap();
            queries::forget_shared_plays(&db.conn, user, &[(track, 1)]).unwrap();
            queries::create_playlist(&db.conn, user, "List", None).unwrap();
            queries::shares::create_share(
                &db.conn,
                user,
                queries::shares::Slice::TRACKS,
                &[track],
                None,
                0,
                None,
            )
            .unwrap();
        }

        assert!(delete_user(&db.conn, mate).unwrap());

        for table in [
            "favourites",
            "favourite_albums",
            "favourite_artists",
            "track_ratings",
            "album_ratings",
            "artist_ratings",
            "bookmarks",
            "play_history",
            "play_history_forgotten",
            "playlists",
            "shares",
        ] {
            assert_eq!(
                count(
                    &db,
                    &format!("SELECT COUNT(*) FROM {table} WHERE user_id = {mate}")
                ),
                0,
                "{table} kept the deleted account's rows"
            );
            assert_eq!(
                count(
                    &db,
                    &format!("SELECT COUNT(*) FROM {table} WHERE user_id = {admin}")
                ),
                1,
                "{table} lost another account's rows"
            );
        }
    }
}
