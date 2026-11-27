//! Entity ids: published uids, the prefixed and bare row ids older clients
//! hold, and resolving either to a row.

use super::*;

/// koan publishes each artist, album and song by its uid, which no two rows of
/// any kind share. Clients that synced before uids existed still hold row ids,
/// which artists, albums and songs number separately: those arrive bare or with
/// a type prefix, and without one, `getCoverArt?id=5` cannot say whether it
/// means album 5 or track 5. Navidrome spells the prefixes the same way.
pub(super) const ARTIST_PREFIX: &str = "ar-";
pub(super) const ALBUM_PREFIX: &str = "al-";
pub(super) const SONG_PREFIX: &str = "mf-";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EntityKind {
    Artist,
    Album,
    Song,
}

impl EntityKind {
    pub(super) fn uid_kind(self) -> queries::UidKind {
        match self {
            Self::Artist => queries::UidKind::Artist,
            Self::Album => queries::UidKind::Album,
            Self::Song => queries::UidKind::Track,
        }
    }

    pub(super) fn noun(self) -> &'static str {
        match self {
            Self::Artist => "Artist",
            Self::Album => "Album",
            Self::Song => "Song",
        }
    }
}

impl EntityKind {
    pub(super) fn of(kind: queries::UidKind) -> Option<Self> {
        match kind {
            queries::UidKind::Artist => Some(Self::Artist),
            queries::UidKind::Album => Some(Self::Album),
            queries::UidKind::Track => Some(Self::Song),
            queries::UidKind::Playlist => None,
        }
    }
}

/// Parse a row id: `ar-3`, `al-3`, `mf-3`, or a bare `3`.
pub(super) fn parse_entity_id(raw: &str) -> Option<(Option<EntityKind>, i64)> {
    for (prefix, kind) in [
        (ARTIST_PREFIX, EntityKind::Artist),
        (ALBUM_PREFIX, EntityKind::Album),
        (SONG_PREFIX, EntityKind::Song),
    ] {
        if let Some(rest) = raw.strip_prefix(prefix) {
            return rest.parse().ok().map(|id| (Some(kind), id));
        }
    }
    raw.parse().ok().map(|id| (None, id))
}

/// The row an id names, with its kind when the id carries one: a uid always
/// does, a bare row id never.
pub(super) fn resolve_entity(
    db: &Database,
    raw: &str,
) -> Result<(Option<EntityKind>, i64), SubsonicError> {
    if let Some(parsed) = parse_entity_id(raw) {
        return Ok(parsed);
    }
    if !queries::is_uid(raw) {
        return Err(SubsonicError::bad_param("id"));
    }
    queries::find_uid(&db.conn, raw)
        .map_err(|e| SubsonicError::internal(e.to_string()))?
        .and_then(|(kind, id)| Some((Some(EntityKind::of(kind)?), id)))
        .ok_or_else(|| SubsonicError::not_found("Item"))
}

/// The row of `kind` an id names. A uid is looked up among that kind only, so
/// an album's uid given to `getSong` is not found rather than read as a song;
/// a row id's prefix is ignored, as it always was.
pub(super) fn resolve_as(
    db: &Database,
    raw: &str,
    kind: EntityKind,
    param: &str,
) -> Result<i64, SubsonicError> {
    if let Some((_, id)) = parse_entity_id(raw) {
        return Ok(id);
    }
    if !queries::is_uid(raw) {
        return Err(SubsonicError::bad_param(param));
    }
    queries::resolve_id(&db.conn, kind.uid_kind(), raw)
        .map_err(|e| SubsonicError::internal(e.to_string()))?
        .ok_or_else(|| SubsonicError::not_found(kind.noun()))
}

/// The `id` parameter as a row of `kind`.
pub(super) fn require_id(
    db: &Database,
    raw: Option<&str>,
    kind: EntityKind,
) -> Result<i64, SubsonicError> {
    let raw = raw.ok_or_else(|| SubsonicError::missing_param("id"))?;
    resolve_as(db, raw, kind, "id")
}

/// The `id` parameter for endpoints that serve more than one kind of entity.
pub(super) fn require_entity(
    db: &Database,
    raw: Option<&str>,
) -> Result<(Option<EntityKind>, i64), SubsonicError> {
    let raw = raw.ok_or_else(|| SubsonicError::missing_param("id"))?;
    resolve_entity(db, raw)
}

/// Song ids from a repeated parameter. One that names no song, malformed or
/// another kind's included, refuses the whole call: a playlist write that kept
/// the rest and answered ok would lose a track without anyone being told. An
/// empty value names nothing and is skipped, as `songId=` empties a playlist.
pub(super) fn song_ids<'a>(
    db: &Database,
    raws: impl Iterator<Item = &'a str>,
) -> Result<Vec<i64>, SubsonicError> {
    let internal = |e: rusqlite::Error| SubsonicError::internal(e.to_string());
    let mut exists = db
        .conn
        .prepare_cached("SELECT EXISTS (SELECT 1 FROM tracks WHERE id = ?1)")
        .map_err(internal)?;
    raws.filter(|raw| !raw.is_empty())
        .map(|raw| {
            let missing = || SubsonicError::not_found(&format!("Song {raw}"));
            match parse_entity_id(raw) {
                Some((None | Some(EntityKind::Song), id)) => {
                    if exists.query_row([id], |r| r.get(0)).map_err(internal)? {
                        Ok(id)
                    } else {
                        Err(missing())
                    }
                }
                Some(_) => Err(missing()),
                None if queries::is_uid(raw) => {
                    queries::id_for_uid(&db.conn, queries::UidKind::Track, raw)
                        .map_err(internal)?
                        .ok_or_else(missing)
                }
                None => Err(missing()),
            }
        })
        .collect()
}

/// The uid each row in a response is published as, read once per response.
/// A row without one falls back to its prefixed row id, which is still
/// accepted everywhere.
#[derive(Default)]
pub(super) struct Uids {
    pub(super) artists: HashMap<i64, String>,
    pub(super) albums: HashMap<i64, String>,
    pub(super) tracks: HashMap<i64, String>,
}

impl Uids {
    pub(super) fn load(
        db: &Database,
        artists: impl IntoIterator<Item = i64>,
        albums: impl IntoIterator<Item = i64>,
        tracks: impl IntoIterator<Item = i64>,
    ) -> Result<Self, SubsonicError> {
        let read = |kind, ids: Vec<i64>| {
            // Nothing to look up is no reason for a query.
            if ids.is_empty() {
                return Ok(HashMap::new());
            }
            queries::uids_for(&db.conn, kind, ids)
                .map_err(|e| SubsonicError::internal(e.to_string()))
        };
        Ok(Self {
            artists: read(
                queries::UidKind::Artist,
                artists.into_iter().collect::<Vec<_>>(),
            )?,
            albums: read(queries::UidKind::Album, albums.into_iter().collect())?,
            tracks: read(queries::UidKind::Track, tracks.into_iter().collect())?,
        })
    }

    pub(super) fn artist(&self, id: i64) -> String {
        published(&self.artists, ARTIST_PREFIX, id)
    }

    pub(super) fn album(&self, id: i64) -> String {
        published(&self.albums, ALBUM_PREFIX, id)
    }

    pub(super) fn track(&self, id: i64) -> String {
        published(&self.tracks, SONG_PREFIX, id)
    }
}

pub(super) fn published(uids: &HashMap<i64, String>, prefix: &str, id: i64) -> String {
    uids.get(&id)
        .cloned()
        .unwrap_or_else(|| format!("{prefix}{id}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subsonic::testing::*;

    #[test]
    fn test_parse_entity_id() {
        assert_eq!(parse_entity_id("5"), Some((None, 5)));
        assert_eq!(parse_entity_id("mf-5"), Some((Some(EntityKind::Song), 5)));
        assert_eq!(parse_entity_id("al-5"), Some((Some(EntityKind::Album), 5)));
        assert_eq!(parse_entity_id("ar-5"), Some((Some(EntityKind::Artist), 5)));
        assert_eq!(parse_entity_id("not-an-id"), None);
    }

    /// A malformed id comes back as a Subsonic error, not as axum's bare HTTP 400
    /// with a plain-text body that no client can parse.
    #[tokio::test]
    async fn test_bad_id_is_a_subsonic_error() {
        let (state, _dir) = test_state();
        let app = build_test_router(state);
        let (status, body) = get_response(
            app,
            &format!("/rest/stream?{}&id=not-a-number", auth_query("")),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("status=\"failed\""));
        assert!(body.contains("code=\"10\""));
    }

    /// A uid names its row whatever endpoint takes it, and only a row of the
    /// kind the endpoint serves.
    #[tokio::test]
    async fn test_uids_name_one_row_of_one_kind() {
        let (state, _dir) = test_state();
        seed_data(&state);
        let db = Database::open(state.pool.path()).unwrap();
        let track = queries::all_tracks(&db.conn).unwrap()[0].clone();
        let album_id = track.album_id.unwrap();
        let song = uid_of(&state, queries::UidKind::Track, track.id);
        let album = uid_of(&state, queries::UidKind::Album, album_id);

        let get = |path: String| {
            let app = build_test_router(state.clone());
            async move { get_response(app, &path).await.1 }
        };
        let body = get(format!("/rest/getSong?{}&id={song}", auth_query(""))).await;
        assert!(body.contains("title=\"Test Song\""), "{body}");
        let body = get(format!("/rest/getSong?{}&id={album}", auth_query(""))).await;
        assert!(body.contains("status=\"failed\""), "{body}");
        let body = get(format!("/rest/getAlbum?{}&id={album}", auth_query(""))).await;
        assert!(body.contains("status=\"ok\""), "{body}");
        let body = get(format!(
            "/rest/getSong?{}&id=01a0ee12-0000-7000-8000-000000000000",
            auth_query("")
        ))
        .await;
        assert!(body.contains("status=\"failed\""), "{body}");

        // `star` takes any kind by `id`: the uid says which.
        let body = get(format!("/rest/star?{}&id={album}", auth_query(""))).await;
        assert!(body.contains("status=\"ok\""), "{body}");
        assert!(
            queries::load_favourites(&db.conn, queries::LOCAL_USER)
                .unwrap()
                .is_empty()
        );
        assert!(
            queries::favourite_album_id_set(&db.conn, queries::LOCAL_USER)
                .unwrap()
                .contains(&album_id)
        );
    }
}
