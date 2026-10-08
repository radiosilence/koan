//! Share links to tracks, albums and playlists.

use crate::config::Config;
use crate::db::connection::Database;
use crate::db::queries;
use crate::db::queries::shares::{ShareKind, Slice};

use super::*;

/// Why a share link could not be made. Each variant is something the user can
/// act on.
#[derive(Debug, thiserror::Error)]
pub enum ShareError {
    #[error("sharing.public_url is not set, so there is no address to give out")]
    NoPublicUrl,
    #[error("none of these tracks are in the library")]
    NothingToShare,
    #[error("none of these tracks are on the server, so a link has nothing to point at")]
    NothingRemote,
    #[error("the server refused to share these: {0}")]
    Server(#[from] crate::remote::client::SubsonicError),
    #[error(transparent)]
    Database(#[from] crate::db::connection::DbError),
}

/// A created share link, and how much of the request it covers.
#[derive(Debug, Clone)]
pub struct ShareOutcome {
    pub url: String,
    /// The server's own ID for the share, for callers that manage them.
    pub id: String,
    /// Tracks the server knows about, which went into the link.
    pub shared: usize,
    /// Tracks with no copy on the server, left out of it.
    pub skipped: usize,
}

/// What a share link is asked to cover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShareTarget {
    /// Loose tracks. A single track shares its album, cued to that track.
    Tracks(Vec<i64>),
    /// An album, optionally cued to one of its tracks.
    Album {
        album_id: i64,
        start_track_id: Option<i64>,
    },
    /// An artist's albums, in release order.
    Artist(i64),
}

/// The slice a target makes and the tracks it covers, in play order. Only
/// tracks in the library are included; the list is fixed from here on.
///
/// A single track becomes its album cued to it: a song is heard in the
/// record it belongs to, the way the app shows it.
pub fn resolve_share(
    conn: &rusqlite::Connection,
    target: &ShareTarget,
) -> Result<(Slice, Vec<i64>), ShareError> {
    let album_tracks = |album_id| -> Result<Vec<i64>, ShareError> {
        Ok(queries::tracks_for_album(conn, album_id)?
            .into_iter()
            .map(|t| t.id)
            .collect())
    };
    let (slice, ids) = match target {
        ShareTarget::Tracks(ids) => {
            let rows = queries::tracks_by_ids(conn, ids)?;
            match (ids.as_slice(), rows.first().and_then(|t| t.album_id)) {
                ([one], Some(album_id)) => {
                    return resolve_share(
                        conn,
                        &ShareTarget::Album {
                            album_id,
                            start_track_id: Some(*one),
                        },
                    );
                }
                _ => {
                    // The order asked for, which is the order the page plays them in.
                    let ids = ids
                        .iter()
                        .copied()
                        .filter(|id| rows.iter().any(|t| t.id == *id))
                        .collect();
                    (Slice::TRACKS, ids)
                }
            }
        }
        ShareTarget::Album {
            album_id,
            start_track_id,
        } => {
            let ids = album_tracks(*album_id)?;
            let slice = Slice {
                kind: ShareKind::Album,
                subject_id: Some(*album_id),
                start_track_id: start_track_id.filter(|s| ids.contains(s)),
            };
            (slice, ids)
        }
        ShareTarget::Artist(artist_id) => {
            let mut ids = Vec::new();
            for album in queries::albums_for_artist(conn, *artist_id)? {
                ids.extend(album_tracks(album.id)?);
            }
            let slice = Slice {
                kind: ShareKind::Artist,
                subject_id: Some(*artist_id),
                start_track_id: None,
            };
            (slice, ids)
        }
    };
    if ids.is_empty() {
        return Err(ShareError::NothingToShare);
    }
    Ok((slice, ids))
}

/// Create a public share link for a slice of the library.
///
/// With a remote Subsonic server configured, the link is made there: a laptop
/// or phone shares through the server it plays from, which may be another
/// koan. Without one this koan is the server, and makes the link itself.
///
/// A link points at the server, so only tracks the server knows about can go in
/// it. A mixed selection shares the part that can be shared and reports the
/// rest rather than failing whole — half a link beats none, as long as the
/// caller says which half.
///
/// `user` is who is sharing, recorded on a link this koan makes itself.
///
/// May be network-bound. Callers keep it off whatever thread draws.
pub fn create_share(
    db: &Database,
    user: i64,
    cfg: &Config,
    target: &ShareTarget,
    description: Option<&str>,
) -> Result<ShareOutcome, ShareError> {
    let Some(client) = subsonic_client(cfg) else {
        return create_native_share(db, user, cfg, target, description);
    };
    // A remote server makes its own kind of link from what it is given, so it
    // is given exactly what was picked.
    let resolved;
    let track_ids = match target {
        ShareTarget::Tracks(ids) => ids.as_slice(),
        _ => {
            resolved = resolve_share(&db.conn, target)?.1;
            resolved.as_slice()
        }
    };

    // One query, not one per track: sharing an artist is thousands of tracks.
    let rows = queries::tracks_by_ids(&db.conn, track_ids)?;

    let shared = rows.iter().filter(|t| t.remote_id.is_some()).count();
    if shared == 0 {
        return Err(ShareError::NothingRemote);
    }

    // A whole record shares as one album rather than as N tracks — the server
    // renders it as the album it is, and the link survives the user adding to
    // it. Only when the selection is the whole album.
    let one_album = rows
        .first()
        .and_then(|f| f.album_id)
        .filter(|first| rows.iter().all(|t| t.album_id == Some(*first)))
        .and_then(|album_id| album_remote_id(&db.conn, album_id, rows.len()));

    let remote_ids: Vec<String> = match one_album {
        Some(rid) => vec![album_share_id(&client, rid)],
        None => rows.into_iter().filter_map(|t| t.remote_id).collect(),
    };

    let refs: Vec<&str> = remote_ids.iter().map(String::as_str).collect();
    let share = client.create_share(&refs, description)?;

    // Navidrome does not always hand back a URL, and a share with no link is
    // useless to the caller — the ID is enough to build it.
    let url = share
        .url
        .clone()
        .unwrap_or_else(|| format!("{}/s/{}", client.base_url(), share.id));

    Ok(ShareOutcome {
        url,
        id: share.id,
        shared,
        skipped: track_ids.len().saturating_sub(shared),
    })
}

/// A share this koan serves at `{sharing.public_url}/share/{id}`.
///
/// What a server's own surfaces make whatever `[remote]` says: a link made
/// upstream would belong to the upstream's account, not the koan user who
/// asked, and could not be listed or revoked here.
pub fn create_native_share(
    db: &Database,
    user: i64,
    cfg: &Config,
    target: &ShareTarget,
    description: Option<&str>,
) -> Result<ShareOutcome, ShareError> {
    let base = cfg
        .sharing
        .public_url
        .as_deref()
        .filter(|u| !u.trim().is_empty())
        .ok_or(ShareError::NoPublicUrl)?;
    let (slice, ids) = resolve_share(&db.conn, target)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let share = queries::shares::create_share(&db.conn, user, slice, &ids, description, now, None)?;
    Ok(ShareOutcome {
        url: share_url(base, &share.id),
        id: share.id,
        shared: ids.len(),
        // Only loose tracks are named one by one, so only they can be missing.
        skipped: match (target, slice.kind) {
            (ShareTarget::Tracks(asked), ShareKind::Tracks) => asked.len() - ids.len(),
            _ => 0,
        },
    })
}

/// A native share's public address.
pub fn share_url(public_url: &str, id: &str) -> String {
    format!("{}/share/{id}", public_url.trim_end_matches('/'))
}

/// An album's id as `createShare` should be given it.
///
/// koan numbers albums and songs separately, publishes album ids bare, and
/// reads a bare id in `createShare` as a song, so album 5 would share song 5.
/// Its `al-` prefix says which is meant. Other servers get the id as they
/// issued it: some also number albums, and would not know the prefix.
fn album_share_id(client: &crate::remote::client::SubsonicClient, remote_id: String) -> String {
    let koan = crate::remote::profile::is_koan(client.auth());
    album_share_id_for(koan, remote_id)
}

fn album_share_id_for(koan: bool, remote_id: String) -> String {
    if koan && remote_id.parse::<i64>().is_ok() {
        format!("al-{remote_id}")
    } else {
        remote_id
    }
}

/// The album's own remote ID, but only when `selected` covers every track on
/// it. Sharing an album link for half an album would hand out more than the
/// user picked.
pub(super) fn album_remote_id(
    conn: &rusqlite::Connection,
    album_id: i64,
    selected: usize,
) -> Option<String> {
    let (remote_id, total): (Option<String>, i64) = conn
        .query_row(
            "SELECT al.remote_id, (SELECT COUNT(*) FROM tracks WHERE album_id = al.id)
             FROM albums al WHERE al.id = ?1",
            [album_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok()?;
    (total == selected as i64).then_some(remote_id).flatten()
}

#[cfg(test)]
mod share_tests {
    use super::*;
    use crate::db::queries::sample_meta;

    fn test_db() -> Database {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        Database { conn }
    }

    /// Three tracks on one album; the album carries a remote ID.
    fn album_of_three(db: &Database) -> (i64, Vec<i64>) {
        let ids: Vec<i64> = ["One", "Two", "Three"]
            .iter()
            .enumerate()
            .map(|(i, title)| {
                let mut meta = sample_meta(title, "Boards of Canada", "Geogaddi");
                meta.path = Some(format!("/music/geogaddi/{i}.flac"));
                meta.track_number = Some(i as i32 + 1);
                queries::upsert_track(&db.conn, &meta).unwrap()
            })
            .collect();
        let album_id: i64 = db
            .conn
            .query_row("SELECT album_id FROM tracks WHERE id = ?1", [ids[0]], |r| {
                r.get(0)
            })
            .unwrap();
        db.conn
            .execute(
                "UPDATE albums SET remote_id = 'al-1' WHERE id = ?1",
                [album_id],
            )
            .unwrap();
        (album_id, ids)
    }

    #[test]
    fn whole_album_collapses_to_the_album_link() {
        let db = test_db();
        let (album_id, ids) = album_of_three(&db);
        assert_eq!(
            album_remote_id(&db.conn, album_id, ids.len()),
            Some("al-1".into())
        );
    }

    #[test]
    fn part_of_an_album_does_not() {
        let db = test_db();
        let (album_id, _) = album_of_three(&db);
        // Sharing an album link for two of three tracks would hand out a track
        // the user did not pick.
        assert_eq!(album_remote_id(&db.conn, album_id, 2), None);
    }

    #[test]
    fn a_local_only_album_has_no_link_to_collapse_to() {
        let db = test_db();
        let (album_id, ids) = album_of_three(&db);
        db.conn
            .execute(
                "UPDATE albums SET remote_id = NULL WHERE id = ?1",
                [album_id],
            )
            .unwrap();
        assert_eq!(album_remote_id(&db.conn, album_id, ids.len()), None);
    }
}

#[cfg(test)]
mod native_share_tests {
    use super::*;
    use crate::db::queries::{sample_meta, upsert_track};

    #[test]
    fn a_standalone_server_shares_natively_in_the_order_asked() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        let db = Database { conn };
        let a = upsert_track(&db.conn, &sample_meta("A", "X", "Y")).unwrap();
        let b = upsert_track(&db.conn, &sample_meta("B", "X", "Y")).unwrap();
        let mut cfg = Config::default();
        assert!(matches!(
            create_share(
                &db,
                queries::LOCAL_USER,
                &cfg,
                &ShareTarget::Tracks(vec![a]),
                None
            ),
            Err(ShareError::NoPublicUrl)
        ));
        cfg.sharing.public_url = Some("https://koan.example/".into());
        let out = create_share(
            &db,
            queries::LOCAL_USER,
            &cfg,
            &ShareTarget::Tracks(vec![b, 9999, a]),
            Some("mix"),
        )
        .unwrap();
        assert_eq!(out.url, format!("https://koan.example/share/{}", out.id));
        assert_eq!((out.shared, out.skipped), (2, 1));
        let share = queries::shares::get_share(&db.conn, &out.id)
            .unwrap()
            .unwrap();
        assert_eq!(share.track_ids, [b, a]);
        assert!(matches!(
            create_share(
                &db,
                queries::LOCAL_USER,
                &cfg,
                &ShareTarget::Tracks(vec![9999]),
                None
            ),
            Err(ShareError::NothingToShare)
        ));
    }

    #[test]
    fn a_server_with_an_upstream_still_shares_natively() {
        // Local-only tracks, which the upstream path refuses as NothingRemote.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        let db = Database { conn };
        let a = upsert_track(&db.conn, &sample_meta("A", "X", "Y")).unwrap();
        let mut cfg = Config::default();
        cfg.remote.enabled = true;
        cfg.remote.url = "https://upstream.invalid".into();
        cfg.remote.username = "someone".into();
        cfg.remote.password = "secret".into();
        cfg.sharing.public_url = Some("https://koan.example".into());
        let out = create_native_share(
            &db,
            queries::LOCAL_USER,
            &cfg,
            &ShareTarget::Tracks(vec![a]),
            None,
        )
        .unwrap();
        assert_eq!(out.url, format!("https://koan.example/share/{}", out.id));
        assert!(
            queries::shares::get_share(&db.conn, &out.id)
                .unwrap()
                .is_some()
        );
    }

    fn album_track(db: &Database, title: &str, album: &str, n: i32, date: &str) -> i64 {
        let mut meta = sample_meta(title, "Rrose", album);
        meta.track_number = Some(n);
        meta.date = Some(date.into());
        upsert_track(&db.conn, &meta).unwrap()
    }

    #[test]
    fn shares_are_slices_fixed_when_made() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        let db = Database { conn };
        let later = album_track(&db, "L1", "Later", 1, "2021");
        let a1 = album_track(&db, "E1", "Earlier", 1, "2015");
        let a2 = album_track(&db, "E2", "Earlier", 2, "2015");
        let album_of = |t| {
            queries::tracks_by_ids(&db.conn, &[t]).unwrap()[0]
                .album_id
                .unwrap()
        };
        let (earlier, later_album) = (album_of(a1), album_of(later));
        let artist = queries::tracks_by_ids(&db.conn, &[a1]).unwrap()[0]
            .artist_id
            .unwrap();

        // One track: its album, cued to it.
        let (slice, ids) = resolve_share(&db.conn, &ShareTarget::Tracks(vec![a2])).unwrap();
        assert_eq!(
            (slice.kind, slice.subject_id, slice.start_track_id),
            (ShareKind::Album, Some(earlier), Some(a2))
        );
        assert_eq!(ids, [a1, a2]);

        // An album, with a cue that is not on it dropped.
        let (slice, ids) = resolve_share(
            &db.conn,
            &ShareTarget::Album {
                album_id: later_album,
                start_track_id: Some(a1),
            },
        )
        .unwrap();
        assert_eq!((slice.kind, slice.start_track_id), (ShareKind::Album, None));
        assert_eq!(ids, [later]);

        // An artist: every album, in release order.
        let (slice, ids) = resolve_share(&db.conn, &ShareTarget::Artist(artist)).unwrap();
        assert_eq!(
            (slice.kind, slice.subject_id),
            (ShareKind::Artist, Some(artist))
        );
        assert_eq!(ids, [a1, a2, later]);

        // Several tracks stay a list, in the order given.
        let (slice, ids) = resolve_share(&db.conn, &ShareTarget::Tracks(vec![later, a1])).unwrap();
        assert_eq!(slice, Slice::TRACKS);
        assert_eq!(ids, [later, a1]);

        assert!(matches!(
            resolve_share(&db.conn, &ShareTarget::Artist(9999)),
            Err(ShareError::NothingToShare)
        ));
    }
}

#[cfg(test)]
mod album_share_id_tests {
    use super::album_share_id_for;

    #[test]
    fn a_koan_album_is_named_as_an_album() {
        assert_eq!(album_share_id_for(true, "46215".into()), "al-46215");
        // Already prefixed, or not koan's numbering: left as issued.
        assert_eq!(album_share_id_for(true, "al-7".into()), "al-7");
        assert_eq!(album_share_id_for(false, "46215".into()), "46215");
        assert_eq!(album_share_id_for(false, "3xJ9kQ2pZ".into()), "3xJ9kQ2pZ");
    }
}
