//! Playlists: reading, writing and ordering an account's lists. Smart lists
//! are served read-only.

use super::*;

pub(super) async fn get_playlists(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    offload_response(move || {
        respond_db_user(&state, &params, Role::Readonly, |db, user, b| {
            refresh_smart(db, user);
            let lists = queries::list_playlists(&db.conn, user)
                .map_err(|e| SubsonicError::internal(e.to_string()))?;

            let mut playlists_node = XmlNode::new(Tag::Playlists).array_of(Tag::Playlist);
            for list in &lists {
                playlists_node = playlists_node.child(playlist_attrs(
                    XmlNode::new(Tag::Playlist),
                    list,
                    &state.username,
                ));
            }

            Ok(b.child(playlists_node))
        })
    })
    .await
}

/// The attributes every `<playlist>` carries, list or detail.
pub(super) fn playlist_attrs(
    node: XmlNode,
    list: &queries::PlaylistRow,
    username: &str,
) -> XmlNode {
    let node = node
        .attr("id", &list.uid)
        .attr("name", &list.name)
        .attr_int("songCount", list.track_count)
        .attr_int("duration", list.duration_ms / 1000)
        .attr("owner", list.owner.as_deref().unwrap_or(username))
        .attr_bool("public", list.public)
        .attr("created", &list.created_at)
        .attr("changed", &list.changed_at)
        // OpenSubsonic: a smart playlist takes no edits to its contents.
        .attr_bool("readonly", list.readonly);
    match &list.comment {
        Some(comment) => node.attr("comment", comment),
        None => node,
    }
}

/// A playlist as a `<playlist>` with its members resolved, for `user`.
pub(super) fn playlist_node(
    db: &Database,
    caller: &Caller,
    list: &queries::PlaylistRow,
    owner: &str,
) -> Result<XmlNode, SubsonicError> {
    let mut node = playlist_attrs(XmlNode::new(Tag::Playlist), list, owner).array_of(Tag::Entry);

    // Playlist members are `<entry>`, not `<song>` — an XML client shown
    // `<song>` sees an empty playlist.
    let tracks = queries::playlist_tracks(&db.conn, list.id)
        .map_err(|e| SubsonicError::internal(e.to_string()))?;
    let extras = song_extras(db, caller, &tracks)?;
    for track in &tracks {
        node = node.child(track_node(track, Tag::Entry, &extras));
    }

    Ok(node)
}

/// Playlist `id` if `user` may see it (their own, or a public one) — and, with
/// `edit`, change it (their own only).
pub(super) fn playlist_for(
    db: &Database,
    user: i64,
    id: i64,
    edit: bool,
) -> Result<queries::PlaylistRow, SubsonicError> {
    let me = queries::auth::resolve_user(&db.conn, user)
        .map_err(|e| SubsonicError::internal(e.to_string()))?;
    let list = queries::get_playlist(&db.conn, id)
        .map_err(|e| SubsonicError::internal(e.to_string()))?
        .filter(|p| p.readable_by(me))
        .ok_or_else(|| SubsonicError::not_found("Playlist"))?;
    if edit && !list.editable_by(me) {
        return Err(SubsonicError::not_authorized());
    }
    Ok(list)
}

/// Evaluate the smart playlists `user` can see that are due, and have every
/// device pull any whose contents moved. A failure leaves the last contents
/// in place, which is what a read should serve anyway.
pub(super) fn refresh_smart(db: &Database, user: i64) {
    match queries::smart::refresh_due(&db.conn, user) {
        Ok(changed) if !changed.is_empty() => crate::clients::changed(),
        Ok(_) => {}
        Err(e) => log::warn!("smart playlists not refreshed: {e}"),
    }
}

/// A smart playlist's contents are its rules', and a file's are the file's:
/// neither is for editing.
pub(super) fn refuse_smart(list: &queries::PlaylistRow) -> Result<(), SubsonicError> {
    if list.readonly {
        return Err(SubsonicError::new(
            SubsonicErrorCode::NotAuthorized,
            "This playlist is read-only: its rules or its file decide what it holds",
        ));
    }
    Ok(())
}

/// After a playlist write: push it to the upstream, if there is one, and have
/// the account's devices pull it, as the GraphQL mutations do. A koan app's
/// own edits arrive through these endpoints.
pub(super) fn playlist_changed(id: i64) {
    koan_core::playlists::push_to_remote(id);
    crate::clients::changed();
}

/// A playlist by its uid, or by the row id clients from before uids hold.
pub(super) fn playlist_id(db: &Database, raw: Option<&str>) -> Result<i64, SubsonicError> {
    let raw = raw.ok_or_else(|| SubsonicError::missing_param("id"))?;
    queries::resolve_id(&db.conn, queries::UidKind::Playlist, raw)
        .map_err(|e| SubsonicError::internal(e.to_string()))?
        .ok_or_else(|| SubsonicError::not_found("Playlist"))
}

/// `koanPlaylistOrder` — the account's playlists in the order given, by
/// repeated `id`. Its own playlists only: another account's public playlist
/// sits where its owner put it. Ids no longer known are skipped, since a
/// playlist deleted elsewhere should not refuse the rest of a reorder, and the
/// account's playlists the list leaves out keep their places.
pub(super) async fn koan_playlist_order(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let auth = params.auth();
        respond_db_user(&state, &auth, Role::User, |db, user, b| {
            let ids: Vec<i64> = params
                .all("id")
                .filter_map(|raw| playlist_id(db, Some(raw)).ok())
                .collect();
            // Passed on upstream when this server is signed in there as the
            // account, as its other playlist edits are.
            koan_core::playlists::set_account_order(db, user, &ids)
                .map_err(|e| SubsonicError::internal(e.to_string()))?;
            crate::clients::changed();
            Ok(b)
        })
    })
    .await
}

pub(super) async fn get_playlist(
    State(state): State<Arc<AppState>>,
    Query(params): Query<IdParam>,
) -> Response {
    offload_response(move || {
        respond_db_caller(&state, &params.auth, Role::Readonly, |db, caller, b| {
            let user = caller.user_id;
            let id = playlist_id(db, params.id.as_deref())?;
            playlist_for(db, user, id, false)?;
            match queries::smart::refresh_if_due(&db.conn, id) {
                Ok(true) => crate::clients::changed(),
                Ok(false) => {}
                Err(e) => log::warn!("smart playlist {id} not refreshed: {e}"),
            }
            let list = playlist_for(db, user, id, false)?;
            Ok(b.child(playlist_node(db, caller, &list, &state.username)?))
        })
    })
    .await
}

/// `createPlaylist` — new when given a `name`, a wholesale replacement when
/// given a `playlistId`. That second form is the only Subsonic call that can
/// set a playlist's order, which is why koan's own pushes use it too.
pub(super) async fn create_playlist(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let auth = params.auth();

        respond_db_caller(&state, &auth, Role::User, |db, caller, b| {
            let user = caller.user_id;
            // One transaction, so no reader sees the name without the tracks,
            // and no track named goes between being found and being added.
            let id = queries::atomically(&db.conn, || {
                let track_ids = song_ids(db, params.all("songId"))?;
                match params.get("playlistId") {
                    Some(existing) => {
                        let id = playlist_id(db, Some(existing))?;
                        refuse_smart(&playlist_for(db, user, id, true)?)?;
                        if let Some(name) = params.get("name") {
                            queries::rename_playlist(&db.conn, id, name)
                                .map_err(|e| SubsonicError::internal(e.to_string()))?;
                        }
                        queries::set_playlist_tracks(&db.conn, id, &track_ids)
                            .map_err(SubsonicError::playlist_write)?;
                        Ok(id)
                    }
                    None => {
                        let name = params
                            .get("name")
                            .ok_or_else(|| SubsonicError::missing_param("name"))?;
                        let id = queries::create_playlist(&db.conn, user, name, None)
                            .map_err(|e| SubsonicError::internal(e.to_string()))?;
                        queries::add_tracks(&db.conn, id, &track_ids)
                            .map_err(SubsonicError::playlist_write)?;
                        Ok::<_, SubsonicError>(id)
                    }
                }
            })?;

            playlist_changed(id);
            // Since 1.14.0 the response carries the playlist that was created;
            // clients read the id back off it rather than guessing.
            let list = playlist_for(db, user, id, false)?;
            Ok(b.child(playlist_node(db, caller, &list, &state.username)?))
        })
    })
    .await
}

/// `updatePlaylist` — rename, re-comment, and add or remove members by index.
///
/// Removals are applied by descending index so each one does not shift the
/// next; a client sends them against the list as it stood when it asked.
pub(super) async fn update_playlist(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let auth = params.auth();

        respond_db_user(&state, &auth, Role::User, |db, user, b| {
            let id = playlist_id(db, params.get("playlistId").or_else(|| params.get("id")))?;
            let list = playlist_for(db, user, id, true)?;
            if params.get("name").is_some() && list.source_path.is_some() {
                return Err(SubsonicError::new(
                    SubsonicErrorCode::NotAuthorized,
                    "This playlist is named by its file in the library: rename the file",
                ));
            }
            if params.all("songIdToAdd").next().is_some()
                || params.all("songIndexToRemove").next().is_some()
            {
                refuse_smart(&list)?;
            }

            // One transaction: the indexes to remove are against the list as
            // the client last saw it, and another edit landing between the
            // read and the write would shift them.
            queries::atomically(&db.conn, || {
                let added = song_ids(db, params.all("songIdToAdd"))?;
                if let Some(name) = params.get("name") {
                    queries::rename_playlist(&db.conn, id, name)
                        .map_err(|e| SubsonicError::internal(e.to_string()))?;
                }

                let mut doomed: Vec<usize> = params
                    .all("songIndexToRemove")
                    .filter_map(|i| i.parse::<usize>().ok())
                    .collect();
                if !doomed.is_empty() {
                    doomed.sort_unstable();
                    doomed.dedup();
                    let mut ids = queries::playlist_track_ids(&db.conn, id)
                        .map_err(|e| SubsonicError::internal(e.to_string()))?;
                    for index in doomed.into_iter().rev() {
                        if index < ids.len() {
                            ids.remove(index);
                        }
                    }
                    queries::set_playlist_tracks(&db.conn, id, &ids)
                        .map_err(SubsonicError::playlist_write)?;
                }

                if !added.is_empty() {
                    queries::add_tracks(&db.conn, id, &added)
                        .map_err(SubsonicError::playlist_write)?;
                }
                Ok::<_, SubsonicError>(())
            })?;

            playlist_changed(id);
            Ok(b)
        })
    })
    .await
}

pub(super) async fn delete_playlist(
    State(state): State<Arc<AppState>>,
    Query(params): Query<IdParam>,
) -> Response {
    offload_response(move || {
        respond_db_user(&state, &params.auth, Role::User, |db, user, b| {
            let id = playlist_id(db, params.id.as_deref())?;
            // Read before deleting: the delete has to reach the upstream too,
            // or its next sync brings the playlist back.
            let remote_id = playlist_for(db, user, id, true)?.remote_id;
            match queries::delete_playlist(&db.conn, id) {
                Ok(true) => {
                    if let Some(remote_id) = remote_id {
                        koan_core::playlists::delete_on_remote(remote_id);
                    }
                    crate::clients::changed();
                    Ok(b)
                }
                Ok(false) => Err(SubsonicError::not_found("Playlist")),
                Err(e) => Err(SubsonicError::internal(e.to_string())),
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
    async fn test_create_playlist_with_songs() {
        let (state, _dir) = test_state();
        seed_data(&state);

        let db = Database::open(state.pool.path()).unwrap();
        let track_id = queries::all_tracks(&db.conn).unwrap()[0].id;

        // Two `songId` values — the shape every client sends, and the one
        // `serde_urlencoded` turned into a bare HTTP 400.
        let app = build_test_router(state.clone());
        let (status, body) = get_response(
            app,
            &format!(
                "/rest/createPlaylist?{}&name=testmix&songId={}&songId={}",
                auth_query(""),
                track_id,
                track_id
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("status=\"ok\""), "createPlaylist: {}", body);
        // The response carries the playlist itself, per 1.14.0 — which is how
        // the client learns the id it was given.
        assert!(body.contains("name=\"testmix\""), "{}", body);
        let id = playlist_id_in(&body);

        let app = build_test_router(state);
        let (_, body) = get_response(
            app,
            &format!("/rest/getPlaylist?{}&id={id}", auth_query("")),
        )
        .await;
        assert!(body.contains("songCount=\"2\""), "{}", body);
        // Playlist members are `<entry>`, never `<song>`.
        assert_eq!(body.matches("<entry ").count(), 2, "{}", body);
        assert!(!body.contains("<song "));
    }

    /// The `id` off a `<playlist>` element in a response body.
    fn playlist_id_in(body: &str) -> String {
        let start = body
            .find("<playlist id=\"")
            .expect("a playlist in the response")
            + "<playlist id=\"".len();
        let rest = &body[start..];
        rest[..rest.find('"').unwrap()].to_string()
    }

    #[tokio::test]
    async fn the_account_order_is_what_get_playlists_lists() {
        let (state, _dir) = test_state();
        seed_data(&state);
        let mut ids = Vec::new();
        for name in ["one", "two", "three"] {
            let app = build_test_router(state.clone());
            let (_, body) = get_response(
                app,
                &format!("/rest/createPlaylist?{}&name={name}", auth_query("")),
            )
            .await;
            ids.push(playlist_id_in(&body));
        }

        let app = build_test_router(state.clone());
        let (_, body) = get_response(
            app,
            &format!(
                "/rest/koanPlaylistOrder?{}&id={}&id=gone&id={}",
                auth_query(""),
                ids[0],
                ids[2]
            ),
        )
        .await;
        assert!(body.contains("status=\"ok\""), "{body}");

        let app = build_test_router(state.clone());
        let (_, body) = get_response(app, &format!("/rest/getPlaylists?{}", auth_query(""))).await;
        let at = |name: &str| body.find(&format!("name=\"{name}\"")).unwrap();
        // Created newest first: three, two, one. "two" was not named and keeps
        // its place between the two that swapped.
        assert!(at("one") < at("two") && at("two") < at("three"), "{body}");
    }

    #[tokio::test]
    async fn test_playlists_crud() {
        let (state, _dir) = test_state();
        seed_data(&state);

        let app = build_test_router(state.clone());
        let (_, body) = get_response(
            app,
            &format!("/rest/createPlaylist?{}&name=testmix", auth_query("")),
        )
        .await;
        assert!(
            body.contains("status=\"ok\""),
            "createPlaylist failed: {}",
            body
        );
        let id = playlist_id_in(&body);

        // List
        let app = build_test_router(state.clone());
        let (_, body) = get_response(app, &format!("/rest/getPlaylists?{}", auth_query(""))).await;
        assert!(body.contains("testmix"));

        // Get
        let app = build_test_router(state.clone());
        let (_, body) = get_response(
            app,
            &format!("/rest/getPlaylist?{}&id={id}", auth_query("")),
        )
        .await;
        assert!(body.contains("testmix"));

        // Delete
        let app = build_test_router(state.clone());
        let (_, body) = get_response(
            app,
            &format!("/rest/deletePlaylist?{}&id={id}", auth_query("")),
        )
        .await;
        assert!(body.contains("status=\"ok\""));

        // Verify deleted
        let app = build_test_router(state);
        let (_, body) = get_response(
            app,
            &format!("/rest/getPlaylist?{}&id={id}", auth_query("")),
        )
        .await;
        assert!(body.contains("status=\"failed\""));
    }

    /// `updatePlaylist` adds and removes members by index, and a removal must
    /// not shift the index of the one after it.
    #[tokio::test]
    async fn test_update_playlist_adds_and_removes() {
        let (state, _dir) = test_state();
        seed_data(&state);

        let db = Database::open(state.pool.path()).unwrap();
        let a = queries::upsert_track(&db.conn, &track_meta("/music/a.flac", "A", "Test Album", 1))
            .unwrap();
        let b = queries::upsert_track(&db.conn, &track_meta("/music/b.flac", "B", "Test Album", 2))
            .unwrap();
        let c = queries::upsert_track(&db.conn, &track_meta("/music/c.flac", "C", "Test Album", 3))
            .unwrap();
        drop(db);

        let app = build_test_router(state.clone());
        let (_, body) = get_response(
            app,
            &format!(
                "/rest/createPlaylist?{}&name=mix&songId={a}&songId={b}&songId={c}",
                auth_query("")
            ),
        )
        .await;
        let id = playlist_id_in(&body);

        let app = build_test_router(state.clone());
        let (_, body) = get_response(
            app,
            &format!(
                "/rest/updatePlaylist?{}&playlistId={id}&songIndexToRemove=0&songIndexToRemove=1&songIdToAdd={a}",
                auth_query("")
            ),
        )
        .await;
        assert!(body.contains("status=\"ok\""), "updatePlaylist: {}", body);

        let db = Database::open(state.pool.path()).unwrap();
        let row = queries::id_for_uid(&db.conn, queries::UidKind::Playlist, &id)
            .unwrap()
            .unwrap();
        let left = queries::playlist_track_ids(&db.conn, row).unwrap();
        assert_eq!(left, vec![c, a]);
    }

    /// A song id that names no track refuses the write with "not found"
    /// rather than keeping the others and answering ok.
    #[tokio::test]
    async fn playlist_writes_refuse_unknown_songs() {
        let (state, _dir) = test_state();
        seed_data(&state);

        let db = Database::open(state.pool.path()).unwrap();
        let a = queries::upsert_track(&db.conn, &track_meta("/music/a.flac", "A", "Test Album", 1))
            .unwrap();
        let a_uid = queries::uids_for(&db.conn, queries::UidKind::Track, [a]).unwrap()[&a].clone();
        drop(db);

        let app = build_test_router(state.clone());
        let (_, body) = get_response(
            app,
            &format!(
                "/rest/createPlaylist?{}&name=mix&songId={a_uid}",
                auth_query("")
            ),
        )
        .await;
        let id = playlist_id_in(&body);

        let stranger = uuid::Uuid::now_v7().to_string();
        let album = format!("al-{a}");
        for unknown in [
            stranger.as_str(),
            "mf-99999",
            "99999",
            "nonsense",
            album.as_str(),
        ] {
            for edit in [
                format!("createPlaylist?name=other&songId={a_uid}&songId={unknown}"),
                format!("createPlaylist?playlistId={id}&songId={unknown}"),
                format!("updatePlaylist?playlistId={id}&songIdToAdd={a_uid}&songIdToAdd={unknown}"),
            ] {
                let (path, query) = edit.split_once('?').unwrap();
                let app = build_test_router(state.clone());
                let (_, body) =
                    get_response(app, &format!("/rest/{path}?{}&{query}", auth_query(""))).await;
                assert!(body.contains("code=\"70\""), "{edit}: {body}");
                assert!(body.contains(unknown), "the error names the id: {body}");
            }
        }

        let db = Database::open(state.pool.path()).unwrap();
        let row = queries::id_for_uid(&db.conn, queries::UidKind::Playlist, &id)
            .unwrap()
            .unwrap();
        assert_eq!(queries::playlist_track_ids(&db.conn, row).unwrap(), vec![a]);
        drop(db);

        let app = build_test_router(state.clone());
        let (_, body) = get_response(
            app,
            &format!(
                "/rest/createPlaylist?{}&playlistId={id}&songId=",
                auth_query("")
            ),
        )
        .await;
        assert!(
            body.contains("status=\"ok\""),
            "an empty songId empties it: {body}"
        );
        let db = Database::open(state.pool.path()).unwrap();
        assert!(
            queries::playlist_track_ids(&db.conn, row)
                .unwrap()
                .is_empty()
        );
        let lists: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM playlists", [], |r| r.get(0))
            .unwrap();
        assert_eq!(lists, 1, "nothing half-made is left");
    }

    /// A smart playlist is served like any other, marked read-only, and its
    /// contents refuse edits while its name does not.
    #[tokio::test]
    async fn smart_playlists_are_served_read_only() {
        let (state, _dir) = test_state();
        seed_data(&state);

        let db = Database::open(state.pool.path()).unwrap();
        let tracks = queries::all_tracks(&db.conn).unwrap();
        let rules = koan_core::smart::Rules::parse(r#"{"rules":[]}"#).unwrap();
        let row = queries::smart::create_smart_playlist(
            &db.conn,
            queries::LOCAL_USER,
            "Everything",
            None,
            &rules,
        )
        .unwrap();
        let id = queries::get_playlist(&db.conn, row).unwrap().unwrap().uid;
        drop(db);

        let app = build_test_router(state.clone());
        let (_, body) = get_response(app, &format!("/rest/getPlaylists?{}", auth_query(""))).await;
        assert!(body.contains("name=\"Everything\""), "{body}");
        assert!(body.contains("readonly=\"true\""), "{body}");

        let app = build_test_router(state.clone());
        let (_, body) = get_response(
            app,
            &format!("/rest/getPlaylist?{}&id={id}", auth_query("")),
        )
        .await;
        assert_eq!(body.matches("<entry ").count(), tracks.len(), "{body}");

        for edit in [
            format!(
                "updatePlaylist?playlistId={id}&songIdToAdd={}",
                tracks[0].id
            ),
            format!("updatePlaylist?playlistId={id}&songIndexToRemove=0"),
            format!("createPlaylist?playlistId={id}&songId={}", tracks[0].id),
        ] {
            let (path, query) = edit.split_once('?').unwrap();
            let app = build_test_router(state.clone());
            let (_, body) =
                get_response(app, &format!("/rest/{path}?{}&{query}", auth_query(""))).await;
            assert!(body.contains("status=\"failed\""), "{edit}: {body}");
        }

        let app = build_test_router(state.clone());
        let (_, body) = get_response(
            app,
            &format!(
                "/rest/updatePlaylist?{}&playlistId={id}&name=All",
                auth_query("")
            ),
        )
        .await;
        assert!(
            body.contains("status=\"ok\""),
            "a rename is allowed: {body}"
        );

        // One read from a file is named by it.
        let db = Database::open(state.pool.path()).unwrap();
        db.conn
            .execute("UPDATE playlists SET source_path = '/music/All.nsp'", [])
            .unwrap();
        drop(db);
        let app = build_test_router(state.clone());
        let (_, body) = get_response(
            app,
            &format!(
                "/rest/updatePlaylist?{}&playlistId={id}&name=Other",
                auth_query("")
            ),
        )
        .await;
        assert!(body.contains("status=\"failed\""), "{body}");
    }
}
