//! What a listener does to the library: favourites, ratings and scrobbles.

use super::*;

pub(super) async fn star(State(state): State<Arc<AppState>>, RawQuery(raw): RawQuery) -> Response {
    set_starred(state, raw, true).await
}

pub(super) async fn unstar(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    set_starred(state, raw, false).await
}

/// `star` and `unstar`. `id`, `albumId` and `artistId` may each repeat, and
/// an `id` may name an album or artist, by its uid or prefix, as well as a song.
pub(super) async fn set_starred(state: Arc<AppState>, raw: Option<String>, star: bool) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let auth = params.auth();
        respond_db_caller(&state, &auth, Role::User, |db, caller, b| {
            let user = caller.user_id;
            let mut targets = Vec::new();
            for raw in params.all("id") {
                let (kind, id) = resolve_entity(db, raw)?;
                targets.push((kind.unwrap_or(EntityKind::Song), id));
            }
            for (key, kind) in [
                ("albumId", EntityKind::Album),
                ("artistId", EntityKind::Artist),
            ] {
                for raw in params.all(key) {
                    targets.push((kind, resolve_as(db, raw, kind, key)?));
                }
            }
            if targets.is_empty() {
                return Err(SubsonicError::missing_param("id"));
            }
            let songs = targets.iter().any(|(kind, _)| *kind == EntityKind::Song);
            for (kind, id) in targets {
                set_star(db, user, kind, id, star)?;
            }
            favourited(db, user, &caller.username, songs);
            Ok(b)
        })
    })
    .await
}

/// Favourite one artist, record or track for `user`, or stop, as `star` and
/// `unstar` do, and tell the account's apps to pick it up. What the web UI's
/// hearts call, so a heart there is the same favourite an app makes.
pub(crate) fn favourite(
    db: &Database,
    user: i64,
    username: &str,
    kind: EntityKind,
    id: i64,
    star: bool,
) -> Result<(), String> {
    set_star(db, user, kind, id, star).map_err(|e| e.message)?;
    favourited(db, user, username, kind == EntityKind::Song);
    Ok(())
}

/// What follows `user` changing their favourites, through any front end:
/// their smart playlists that read favourite tracks are evaluated again, and
/// their other apps sync, so a track favourited on a phone while it plays on
/// the Mac lights up there.
pub(crate) fn favourited(db: &Database, user: i64, username: &str, tracks: bool) {
    if tracks {
        crate::clients::smart_activity(db, user, &[koan_core::smart::Field::Favourite]);
    }
    crate::clients::registry().broadcast(
        Some(username),
        koan_core::remote::link::LinkCommand::Sync { full: false },
    );
}

pub(super) fn set_star(
    db: &Database,
    user: i64,
    kind: EntityKind,
    id: i64,
    star: bool,
) -> Result<(), SubsonicError> {
    use koan_core::helpers::FavouriteKind;
    let (favourite, name) = match kind {
        EntityKind::Song => (FavouriteKind::Track, "Track"),
        EntityKind::Album => (FavouriteKind::Album, "Album"),
        EntityKind::Artist => (FavouriteKind::Artist, "Artist"),
    };
    koan_core::helpers::set_favourite(db, user, favourite, id, Some(star))
        .map_err(|e| SubsonicError::internal(e.to_string()))?
        .ok_or_else(|| SubsonicError::not_found(name))?;
    Ok(())
}

/// `setRating`: `rating` 1 to 5 rates the song, album or artist `id` names,
/// by its uid or prefix; 0 clears it.
pub(super) async fn set_rating(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let auth = params.auth();
        respond_db_user(&state, &auth, Role::User, |db, user, b| {
            let raw = params
                .get("id")
                .ok_or_else(|| SubsonicError::missing_param("id"))?;
            let rating = params
                .get("rating")
                .ok_or_else(|| SubsonicError::missing_param("rating"))?
                .parse::<u8>()
                .ok()
                .filter(|r| *r <= 5)
                .ok_or_else(|| SubsonicError::bad_param("rating"))?;
            let (kind, id) = resolve_entity(db, raw)?;
            let internal = |e: rusqlite::Error| SubsonicError::internal(e.to_string());
            let kind = match kind.unwrap_or(EntityKind::Song) {
                EntityKind::Song => {
                    queries::get_track_row(&db.conn, id)
                        .map_err(|e| SubsonicError::internal(e.to_string()))?
                        .ok_or_else(|| SubsonicError::not_found("Track"))?;
                    queries::RatingKind::Track
                }
                EntityKind::Album => {
                    queries::get_album(&db.conn, id)
                        .map_err(|e| SubsonicError::internal(e.to_string()))?
                        .ok_or_else(|| SubsonicError::not_found("Album"))?;
                    queries::RatingKind::Album
                }
                EntityKind::Artist => {
                    queries::get_artist(&db.conn, id)
                        .map_err(|e| SubsonicError::internal(e.to_string()))?
                        .ok_or_else(|| SubsonicError::not_found("Artist"))?;
                    queries::RatingKind::Artist
                }
            };
            queries::set_rating(&db.conn, user, kind, id, rating).map_err(internal)?;
            Ok(b)
        })
    })
    .await
}

pub(super) async fn get_starred2(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    offload_response(move || {
        respond_db_caller(&state, &params, Role::Readonly, |db, caller, b| {
            let user = caller.user_id;
            // A query per kind, not per favourite: clients read this on every sync.
            let tracks = queries::favourite_tracks(&db.conn, user, None)
                .map_err(|e| SubsonicError::internal(e.to_string()))?;
            let extras = song_extras(db, caller, &tracks)?;

            let albums = queries::list_albums(
                &db.conn,
                &queries::AlbumQuery {
                    favourites_of: Some(user),
                    ..Default::default()
                },
            )
            .map_err(|e| SubsonicError::internal(e.to_string()))?;
            // By id rather than `list_artists`, which lists only artists who
            // own an album: a favourited guest artist would drop out.
            let artist_ids = queries::favourite_artist_id_set(&db.conn, user)
                .map_err(|e| SubsonicError::internal(e.to_string()))?;
            let artist_ids: Vec<i64> = artist_ids.into_iter().collect();
            let artists = queries::artist_names(&db.conn, &artist_ids)?;
            let album_extras = album_extras(db, user, &albums, None)?;
            let artist_extras = artist_extras(db, user, artists.iter().map(|(id, _)| *id))?;

            Ok(b.child(
                XmlNode::new(Tag::Starred2)
                    .list(
                        Tag::Artist,
                        artists
                            .iter()
                            .map(|(id, name)| artist_id3_node(*id, name, &artist_extras)),
                    )
                    .list(
                        Tag::Album,
                        albums.iter().map(|a| album_to_xml_node(a, &album_extras)),
                    )
                    .list(
                        Tag::Song,
                        tracks.iter().map(|t| track_to_xml_node(t, &extras)),
                    ),
            ))
        })
    })
    .await
}

/// `id` may repeat, each with its own `time`, since 1.8.0: a client flushing
/// an offline session sends every play at once.
pub(super) async fn scrobble(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let auth = params.auth();
        respond_db_caller(&state, &auth, Role::User, |db, caller, b| {
            let user = caller.user_id;
            // An offline session flushes hundreds at once: the uids among
            // them are resolved in one query rather than one each.
            let raws: Vec<&str> = params.all("id").collect();
            let by_uid = queries::ids_for_uids(
                &db.conn,
                queries::UidKind::Track,
                raws.iter()
                    .copied()
                    .filter(|raw| parse_entity_id(raw).is_none()),
            )?;
            let track_ids: Vec<i64> = raws
                .iter()
                .map(|raw| match parse_entity_id(raw) {
                    Some((_, id)) => Ok(id),
                    None if !queries::is_uid(raw) => Err(SubsonicError::bad_param("id")),
                    None => by_uid
                        .get(*raw)
                        .copied()
                        .ok_or_else(|| SubsonicError::not_found(EntityKind::Song.noun())),
                })
                .collect::<Result<_, _>>()?;
            if track_ids.is_empty() {
                return Err(SubsonicError::missing_param("id"));
            }

            // `submission=false` is a now-playing notice, not a play.
            if params.get("submission") == Some("false") {
                if let (Ok(user), Some(&track_id)) = (
                    queries::auth::resolve_user(&db.conn, user),
                    track_ids.first(),
                ) {
                    koan_core::scrobbling::now_playing(user, track_id);
                }
                return Ok(b);
            }

            // `time` is when the client played it, which can be well in the past
            // after an offline session.
            let now = koan_core::clock::unix_secs();
            let times: Vec<Option<i64>> = params.all("time").map(|t| t.parse().ok()).collect();
            let plays: Vec<(i64, i64)> = track_ids
                .iter()
                .enumerate()
                .map(|(i, &track_id)| {
                    let played_at = times
                        .get(i)
                        .copied()
                        .flatten()
                        .map_or(now, |time_ms| time_ms / 1000);
                    (track_id, played_at)
                })
                .collect();
            // The foreign key is the existence check: one id that names no
            // track fails the batch, and the transaction leaves none of it.
            match queries::record_plays_at(&db.conn, user, &plays, queries::SOURCE_SUBSONIC) {
                Ok(()) => {
                    koan_core::scrobbling::wake();
                    use koan_core::smart::Field;
                    crate::clients::smart_activity(
                        db,
                        user,
                        &[Field::PlayCount, Field::LastPlayed],
                    );
                    history_changed(&caller.username);
                    Ok(b)
                }
                Err(koan_core::db::connection::DbError::Sqlite(
                    rusqlite::Error::SqliteFailure(e, _),
                )) if e.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY => {
                    Err(SubsonicError::not_found("Track"))
                }
                Err(e) => Err(SubsonicError::internal(e)),
            }
        })
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subsonic::testing::*;

    #[tokio::test]
    async fn ratings_set_clear_and_order_highest() {
        let (state, _dir) = test_state();
        let [alpha, _, gamma] = seed_shelves(&state)[..] else {
            unreachable!()
        };
        let album_of = |track| {
            let db = Database::open(state.pool.path()).unwrap();
            let album = queries::get_track_row(&db.conn, track)
                .unwrap()
                .unwrap()
                .album_id
                .unwrap();
            uid_of(&state, queries::UidKind::Album, album)
        };
        let rate = |id: String, rating: &'static str| {
            let state = state.clone();
            async move {
                json_of(
                    build_test_router(state),
                    &format!(
                        "/rest/setRating?{}&id={id}&rating={rating}",
                        auth_query("f=json")
                    ),
                )
                .await
            }
        };

        let (alpha_album, gamma_album) = (album_of(alpha), album_of(gamma));
        rate(alpha_album.clone(), "3").await;
        rate(gamma_album.clone(), "5").await;
        assert_eq!(
            album_list2(&state, "type=highest").await,
            ["Gamma", "Alpha"]
        );
        let v = json_of(
            build_test_router(state.clone()),
            &format!("/rest/getAlbum?{}&id={alpha_album}", auth_query("f=json")),
        )
        .await;
        assert_eq!(v["album"]["userRating"], 3, "{v}");

        let song = uid_of(&state, queries::UidKind::Track, alpha);
        rate(song.clone(), "4").await;
        let song_rating = |state: Arc<AppState>, song: String| async move {
            json_of(
                build_test_router(state),
                &format!("/rest/getSong?{}&id={song}", auth_query("f=json")),
            )
            .await["song"]["userRating"]
                .clone()
        };
        assert_eq!(song_rating(state.clone(), song.clone()).await, 4);
        rate(song.clone(), "0").await;
        assert!(song_rating(state.clone(), song.clone()).await.is_null());

        let v = rate(song, "6").await;
        assert_eq!(v["error"]["code"], 10, "a rating above 5 is refused");

        rate(gamma_album, "0").await;
        assert_eq!(album_list2(&state, "type=highest").await, ["Alpha"]);
    }

    #[tokio::test]
    async fn favourites_carry_starred_in_json_and_xml() {
        let (state, _dir) = test_state();
        let [alpha, _, _] = seed_shelves(&state)[..] else {
            unreachable!()
        };
        // Not favourited, and by another artist: every shelf track is Test
        // Artist's, the artist the test stars.
        let elsewhere = {
            let db = Database::open(state.pool.path()).unwrap();
            let mut meta = track_meta("/music/elsewhere.flac", "Away", "Elsewhere", 1);
            meta.artist = "Other Artist".into();
            meta.album_artist = Some("Other Artist".into());
            queries::upsert_track(&db.conn, &meta).unwrap()
        };
        let uids = |track| {
            let db = Database::open(state.pool.path()).unwrap();
            let row = queries::get_track_row(&db.conn, track).unwrap().unwrap();
            (
                uid_of(&state, queries::UidKind::Track, track),
                uid_of(&state, queries::UidKind::Album, row.album_id.unwrap()),
                uid_of(&state, queries::UidKind::Artist, row.artist_id.unwrap()),
            )
        };
        let (song, album, artist) = uids(alpha);
        let (_, other_album, other_artist) = uids(elsewhere);
        json_of(
            build_test_router(state.clone()),
            &format!(
                "/rest/star?{}&id={song}&albumId={album}&artistId={artist}",
                auth_query("f=json")
            ),
        )
        .await;
        let get = |path: String| {
            let state = state.clone();
            async move {
                json_of(
                    build_test_router(state),
                    &format!("/rest/{path}&{}", auth_query("f=json")),
                )
                .await
            }
        };
        let is_iso = |v: &serde_json::Value| {
            v.as_str()
                .is_some_and(|s| s.len() == 24 && s.contains('T') && s.ends_with('Z'))
        };

        let v = get(format!("getAlbum?id={album}")).await;
        assert!(is_iso(&v["album"]["starred"]), "{v}");
        assert!(is_iso(&v["album"]["song"][0]["starred"]), "{v}");
        let v = get(format!("getArtist?id={artist}")).await;
        assert!(is_iso(&v["artist"]["starred"]), "{v}");
        assert!(is_iso(&v["artist"]["album"][0]["starred"]), "{v}");
        let v = get("search3?query=Alpha".to_owned()).await;
        assert!(is_iso(&v["searchResult3"]["song"][0]["starred"]), "{v}");

        let v = get(format!("getAlbum?id={other_album}")).await;
        assert!(v["album"]["starred"].is_null(), "{v}");
        assert!(v["album"]["song"][0]["starred"].is_null(), "{v}");
        let v = get(format!("getArtist?id={other_artist}")).await;
        assert!(v["artist"]["starred"].is_null(), "{v}");
        let mate = json_of(
            build_test_router(state.clone()),
            &format!("/rest/getAlbum?id={album}&u=mate&p=hunter22&v=1.16.1&c=test&f=json"),
        )
        .await;
        assert!(
            mate["album"]["starred"].is_null(),
            "another account's: {mate}"
        );
        assert!(mate["album"]["song"][0]["starred"].is_null(), "{mate}");

        let xml = |path: String| {
            let state = state.clone();
            async move {
                get_response(
                    build_test_router(state),
                    &format!("/rest/{path}&{}", auth_query("")),
                )
                .await
                .1
            }
        };
        let body = xml(format!("getAlbum?id={album}")).await;
        assert_eq!(body.matches(" starred=\"").count(), 2, "{body}");
        let body = xml(format!("getArtist?id={artist}")).await;
        assert_eq!(body.matches(" starred=\"").count(), 2, "{body}");
        let body = xml(format!("getAlbum?id={other_album}")).await;
        assert!(!body.contains(" starred=\""), "{body}");
    }

    #[tokio::test]
    async fn test_star_and_get_starred() {
        let (state, _dir) = test_state();
        seed_data(&state);

        let db = Database::open(state.pool.path()).unwrap();
        let tracks = queries::all_tracks(&db.conn).unwrap();
        let track_id = tracks[0].id;

        let app = build_test_router(state.clone());
        let (_, body) = get_response(
            app,
            &format!("/rest/star?{}&id={}", auth_query(""), track_id),
        )
        .await;
        assert!(body.contains("status=\"ok\""));

        let app = build_test_router(state);
        let (_, body) = get_response(app, &format!("/rest/getStarred2?{}", auth_query(""))).await;
        assert!(body.contains("Test Song"));
    }

    #[tokio::test]
    async fn test_scrobble() {
        let (state, _dir) = test_state();
        seed_data(&state);

        let db = Database::open(state.pool.path()).unwrap();
        let tracks = queries::all_tracks(&db.conn).unwrap();

        let app = build_test_router(state);
        let (_, body) = get_response(
            app,
            &format!("/rest/scrobble?{}&id={}", auth_query(""), tracks[0].id),
        )
        .await;
        assert!(body.contains("status=\"ok\""));
    }

    #[tokio::test]
    async fn test_now_playing_scrobble_records_no_play() {
        let (state, _dir) = test_state();
        seed_data(&state);

        let db = Database::open(state.pool.path()).unwrap();
        let track_id = queries::all_tracks(&db.conn).unwrap()[0].id;

        let app = build_test_router(state);
        let (_, body) = get_response(
            app,
            &format!(
                "/rest/scrobble?{}&id={}&submission=false",
                auth_query(""),
                track_id
            ),
        )
        .await;
        assert!(body.contains("status=\"ok\""));
        assert_eq!(
            queries::play_count(&db.conn, koan_core::db::queries::LOCAL_USER, track_id).unwrap(),
            0
        );
    }

    /// A client that lost the answer to a batch sends it again; each play is
    /// recorded once.
    #[tokio::test]
    async fn a_scrobble_batch_sent_twice_records_each_play_once() {
        let (state, _dir) = test_state();
        seed_data(&state);
        let db = Database::open(state.pool.path()).unwrap();
        let track_id = queries::all_tracks(&db.conn).unwrap()[0].id;
        let path = format!(
            "/rest/scrobble?{}&id={track_id}&time=1000000&id={track_id}&time=2000000",
            auth_query("f=json")
        );
        for _ in 0..2 {
            let v = json_of(build_test_router(state.clone()), &path).await;
            assert_eq!(v["status"], "ok", "{v}");
        }
        assert_eq!(
            queries::play_count(&db.conn, koan_core::db::queries::LOCAL_USER, track_id).unwrap(),
            2
        );
    }

    /// A batch naming one track that does not exist records none of it, so a
    /// client retrying after fixing the batch does not double the rest.
    #[tokio::test]
    async fn a_scrobble_batch_with_a_missing_track_records_nothing() {
        let (state, _dir) = test_state();
        seed_data(&state);
        let db = Database::open(state.pool.path()).unwrap();
        let track_id = queries::all_tracks(&db.conn).unwrap()[0].id;

        let v = json_of(
            build_test_router(state),
            &format!(
                "/rest/scrobble?{}&id={track_id}&id=999999",
                auth_query("f=json")
            ),
        )
        .await;
        assert_eq!(v["error"]["code"], 70, "{v}");
        assert_eq!(
            queries::play_count(&db.conn, koan_core::db::queries::LOCAL_USER, track_id).unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn test_star_album_by_prefixed_id() {
        let (state, _dir) = test_state();
        seed_data(&state);
        let db = Database::open(state.pool.path()).unwrap();
        let album_id = queries::all_tracks(&db.conn).unwrap()[0].album_id.unwrap();

        let app = build_test_router(state.clone());
        let (_, body) = get_response(
            app,
            &format!("/rest/star?{}&id=al-{}", auth_query(""), album_id),
        )
        .await;
        assert!(body.contains("status=\"ok\""), "{}", body);

        // The album, not one of its tracks.
        assert!(
            queries::load_favourites(&db.conn, koan_core::db::queries::LOCAL_USER)
                .unwrap()
                .is_empty()
        );
        assert!(
            queries::favourite_album_id_set(&db.conn, koan_core::db::queries::LOCAL_USER)
                .unwrap()
                .contains(&album_id)
        );
    }

    /// Clients star in bulk: `id`, `albumId` and `artistId` repeat, and all
    /// three can arrive in one request.
    #[tokio::test]
    async fn test_star_repeated_ids_albums_and_artists() {
        let (state, _dir) = test_state();
        let db = Database::open(state.pool.path()).unwrap();
        for (path, title, album) in [
            ("/music/a.flac", "Song A", "Album A"),
            ("/music/b.flac", "Song B", "Album B"),
        ] {
            queries::upsert_track(&db.conn, &track_meta(path, title, album, 1)).unwrap();
        }
        let tracks = queries::all_tracks(&db.conn).unwrap();
        let (a, b) = (&tracks[0], &tracks[1]);

        let app = build_test_router(state.clone());
        let (_, body) = get_response(
            app,
            &format!(
                "/rest/star?{}&id={}&id={}&albumId={}&artistId={}",
                auth_query(""),
                a.id,
                b.id,
                a.album_id.unwrap(),
                a.artist_id.unwrap()
            ),
        )
        .await;
        assert!(body.contains("status=\"ok\""), "{}", body);

        let app = build_test_router(state.clone());
        let (_, body) =
            get_response(app, &format!("/rest/getStarred2?{}&f=json", auth_query(""))).await;
        let starred: serde_json::Value = serde_json::from_str(&body).unwrap();
        let starred = &starred["subsonic-response"]["starred2"];
        assert_eq!(starred["song"].as_array().unwrap().len(), 2, "{}", body);
        assert_eq!(starred["album"].as_array().unwrap().len(), 1, "{}", body);
        assert_eq!(starred["artist"].as_array().unwrap().len(), 1, "{}", body);

        let app = build_test_router(state.clone());
        let (_, body) = get_response(
            app,
            &format!(
                "/rest/unstar?{}&albumId={}&artistId={}",
                auth_query(""),
                a.album_id.unwrap(),
                a.artist_id.unwrap()
            ),
        )
        .await;
        assert!(body.contains("status=\"ok\""), "{}", body);
        assert!(
            queries::favourite_album_id_set(&db.conn, koan_core::db::queries::LOCAL_USER)
                .unwrap()
                .is_empty()
        );
        assert!(
            queries::favourite_artist_id_set(&db.conn, koan_core::db::queries::LOCAL_USER)
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn test_star_without_an_id_is_an_error() {
        let (state, _dir) = test_state();
        let app = build_test_router(state);
        let (_, body) = get_response(app, &format!("/rest/star?{}", auth_query(""))).await;
        assert!(body.contains("status=\"failed\""), "{}", body);
    }
}
