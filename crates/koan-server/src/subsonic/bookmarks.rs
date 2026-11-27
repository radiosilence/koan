//! Bookmarks and the saved play queue: an account's places in tracks, kept
//! for its other clients.

use super::*;

/// `getBookmarks`: the caller's places in tracks, most recently changed first,
/// each with the track as its `entry`.
pub(super) async fn get_bookmarks(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    offload_response(move || {
        respond_db_caller(&state, &params, Role::Readonly, |db, caller, b| {
            let internal = |e: rusqlite::Error| SubsonicError::internal(e.to_string());
            let marks = queries::bookmarks(&db.conn, caller.user_id).map_err(internal)?;
            let ids: Vec<i64> = marks.iter().map(|m| m.track_id).collect();
            let tracks: HashMap<i64, queries::TrackRow> = queries::tracks_by_ids(&db.conn, &ids)
                .map_err(|e| SubsonicError::internal(e.to_string()))?
                .into_iter()
                .map(|t| (t.id, t))
                .collect();
            let extras = song_extras(db, caller, tracks.values())?;
            Ok(b.child(XmlNode::new(Tag::Bookmarks).list(
                Tag::Bookmark,
                marks.iter().filter_map(|m| {
                    let track = tracks.get(&m.track_id)?;
                    Some(
                        XmlNode::new(Tag::Bookmark)
                            .attr_int("position", m.position_ms)
                            .attr("username", &caller.username)
                            .attr_opt("comment", m.comment.as_deref())
                            .attr("created", &iso(m.created_at))
                            .attr("changed", &iso(m.changed_at))
                            .child(track_node(track, Tag::Entry, &extras)),
                    )
                }),
            )))
        })
    })
    .await
}

/// A note on a place in a track. Longer is refused rather than cut, so a
/// client never reads back something other than what it saved.
pub(super) const MAX_BOOKMARK_COMMENT: usize = 1024;

/// The most songs a saved play queue holds. Longer is refused rather than cut,
/// so a client never reads back something other than what it saved; each song
/// is a lookup and a row written in one transaction.
pub(super) const MAX_PLAY_QUEUE: usize = 5000;

/// `createBookmark`: save where the caller is in the song `id` names, as
/// `position` milliseconds and an optional `comment`. One per song; a second
/// replaces the first.
pub(super) async fn create_bookmark(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let auth = params.auth();
        respond_db_user(&state, &auth, Role::User, |db, user, b| {
            let track_id = require_id(db, params.get("id"), EntityKind::Song)?;
            let position = params
                .get("position")
                .ok_or_else(|| SubsonicError::missing_param("position"))?
                .parse::<i64>()
                .ok()
                .filter(|p| *p >= 0)
                .ok_or_else(|| SubsonicError::bad_param("position"))?;
            let comment = params.get("comment");
            if comment.is_some_and(|c| c.chars().count() > MAX_BOOKMARK_COMMENT) {
                return Err(SubsonicError::bad_param("comment"));
            }
            queries::get_track_row(&db.conn, track_id)
                .map_err(|e| SubsonicError::internal(e.to_string()))?
                .ok_or_else(|| SubsonicError::not_found("Song"))?;
            queries::save_bookmark(&db.conn, user, track_id, position, comment)
                .map_err(|e| SubsonicError::internal(e.to_string()))?;
            Ok(b)
        })
    })
    .await
}

/// `deleteBookmark`: forget the caller's place in the song `id` names. One
/// that was never saved is already forgotten, as Navidrome answers it.
pub(super) async fn delete_bookmark(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let auth = params.auth();
        respond_db_user(&state, &auth, Role::User, |db, user, b| {
            let track_id = require_id(db, params.get("id"), EntityKind::Song)?;
            queries::delete_bookmark(&db.conn, user, track_id)
                .map_err(|e| SubsonicError::internal(e.to_string()))?;
            Ok(b)
        })
    })
    .await
}

/// A saved play queue belongs to an account; the shared secret is no
/// account's, and every client signed in with it would share the one queue.
pub(super) fn queue_owner(caller: &Caller) -> Result<i64, SubsonicError> {
    if caller.via == Via::SharedSecret {
        return Err(SubsonicError::new(
            SubsonicErrorCode::NotAuthorized,
            "sign in with an account to save a play queue",
        ));
    }
    Ok(caller.user_id)
}

/// `getPlayQueue` and `getPlayQueueByIndex`: the queue a client saved for the
/// caller, as far as its tracks are still in the library. Nothing saved
/// answers with no `playQueue`, as Navidrome does. `by_index` names the
/// current entry by its place, so a track queued twice is unambiguous.
pub(super) fn play_queue_response(
    state: &AppState,
    params: &RawParams,
    by_index: bool,
) -> Response {
    respond_db_caller(state, &params.auth(), Role::Readonly, |db, caller, b| {
        let user = queue_owner(caller)?;
        let internal = |e: rusqlite::Error| SubsonicError::internal(e.to_string());
        let Some(queue) = queries::play_queue(&db.conn, user).map_err(internal)? else {
            return Ok(b);
        };
        let rows: HashMap<i64, queries::TrackRow> =
            queries::tracks_by_ids(&db.conn, &queue.track_ids)
                .map_err(|e| SubsonicError::internal(e.to_string()))?
                .into_iter()
                .map(|t| (t.id, t))
                .collect();
        let extras = song_extras(db, caller, rows.values())?;
        let tag = if by_index {
            Tag::PlayQueueByIndex
        } else {
            Tag::PlayQueue
        };
        let mut node = XmlNode::new(tag)
            .attr_int("position", queue.position_ms)
            .attr("username", &caller.username)
            .attr("changed", &iso(queue.changed_at))
            .attr("changedBy", &queue.changed_by);
        node = if by_index {
            node.attr_int("currentIndex", queue.current as i64)
        } else {
            node.attr(
                "current",
                &extras.uids.track(queue.track_ids[queue.current]),
            )
        };
        Ok(b.child(
            node.list(
                Tag::Entry,
                queue
                    .track_ids
                    .iter()
                    .filter_map(|id| rows.get(id))
                    .map(|track| track_node(track, Tag::Entry, &extras)),
            ),
        ))
    })
}

pub(super) async fn get_play_queue(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || play_queue_response(&state, &RawParams::parse(raw.as_deref()), false))
        .await
}

pub(super) async fn get_play_queue_by_index(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || play_queue_response(&state, &RawParams::parse(raw.as_deref()), true))
        .await
}

/// `savePlayQueue` and `savePlayQueueByIndex`: replace the caller's saved
/// queue with the songs `id` names, in order, `position` milliseconds into the
/// current one. The current one is `current`, a song id (its first place in
/// the queue), or with `by_index` `currentIndex`, its place. No `id` clears
/// the queue. A song the library does not have is left out, and the current
/// place follows the songs that stay.
pub(super) fn save_play_queue_response(
    state: &AppState,
    params: &RawParams,
    by_index: bool,
) -> Response {
    respond_db_caller(state, &params.auth(), Role::User, |db, caller, b| {
        let user = queue_owner(caller)?;
        if params.all("id").count() > MAX_PLAY_QUEUE {
            return Err(SubsonicError::new(
                SubsonicErrorCode::MissingParameter,
                format!("a play queue holds at most {MAX_PLAY_QUEUE} songs"),
            ));
        }
        let asked: Vec<Option<i64>> = params
            .all("id")
            .map(|raw| resolve_as(db, raw, EntityKind::Song, "id").ok())
            .collect();
        let known: std::collections::HashSet<i64> = queries::tracks_by_ids(
            &db.conn,
            &asked.iter().flatten().copied().collect::<Vec<_>>(),
        )
        .map_err(|e| SubsonicError::internal(e.to_string()))?
        .into_iter()
        .map(|t| t.id)
        .collect();
        // Each kept song with its place in the queue as asked.
        let kept: Vec<(usize, i64)> = asked
            .iter()
            .enumerate()
            .filter_map(|(at, id)| id.filter(|id| known.contains(id)).map(|id| (at, id)))
            .collect();
        let current = if by_index {
            match params.get("currentIndex").filter(|raw| !raw.is_empty()) {
                Some(raw) => {
                    let at = raw
                        .parse::<usize>()
                        .map_err(|_| SubsonicError::bad_param("currentIndex"))?;
                    if at >= asked.len() && !asked.is_empty() {
                        return Err(SubsonicError::bad_param("currentIndex"));
                    }
                    // The first kept song from the one named on: a song left
                    // out leaves the place to the next that stays.
                    kept.iter().position(|(asked_at, _)| *asked_at >= at)
                }
                None if asked.is_empty() => None,
                None => return Err(SubsonicError::missing_param("currentIndex")),
            }
        } else {
            params
                .get("current")
                .and_then(|raw| resolve_as(db, raw, EntityKind::Song, "current").ok())
                .and_then(|current| kept.iter().position(|(_, id)| *id == current))
        };
        // An empty position is the start, as OpenSubsonic says.
        let position = match params.get("position").filter(|raw| !raw.is_empty()) {
            Some(raw) => raw
                .parse::<i64>()
                .ok()
                .filter(|p| *p >= 0)
                .ok_or_else(|| SubsonicError::bad_param("position"))?,
            None => 0,
        };
        let ids: Vec<i64> = kept.into_iter().map(|(_, id)| id).collect();
        queries::save_play_queue(
            &db.conn,
            user,
            &ids,
            current,
            position,
            params.get("c").unwrap_or_default(),
        )
        .map_err(|e| SubsonicError::internal(e.to_string()))?;
        Ok(b)
    })
}

pub(super) async fn save_play_queue(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        save_play_queue_response(&state, &RawParams::parse(raw.as_deref()), false)
    })
    .await
}

pub(super) async fn save_play_queue_by_index(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        save_play_queue_response(&state, &RawParams::parse(raw.as_deref()), true)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subsonic::testing::*;

    #[tokio::test]
    async fn a_saved_play_queue_reads_back_by_id_and_by_index() {
        let (state, _dir) = test_state();
        let [ra, rb, rc] = seed_shelves(&state)[..] else {
            unreachable!()
        };
        let [a, b, c] = [ra, rb, rc].map(|t| uid_of(&state, queries::UidKind::Track, t));
        let call = |as_who: &str, path: String| {
            let state = state.clone();
            let auth = format!("{as_who}&v=1.16.1&c=Feishin&f=json");
            async move { json_of(build_test_router(state), &format!("/rest/{path}&{auth}")).await }
        };
        let owner = "u=owner&p=sesame";
        let ids = |list: &serde_json::Value| -> Vec<String> {
            list.as_array()
                .unwrap()
                .iter()
                .map(|e| e["id"].as_str().unwrap().to_owned())
                .collect()
        };

        // By index: the same track twice, the second current.
        let v = call(
            owner,
            format!("savePlayQueueByIndex.view?id={a}&id={b}&id={a}&currentIndex=2&position=1500"),
        )
        .await;
        assert_eq!(v["status"], "ok", "{v}");
        let v = call(owner, "getPlayQueueByIndex?".into()).await;
        let q = &v["playQueueByIndex"];
        assert_eq!(
            ids(&q["entry"]),
            [a.as_str(), b.as_str(), a.as_str()],
            "{v}"
        );
        assert_eq!(q["currentIndex"], 2, "{v}");
        assert_eq!(q["position"], 1500);
        assert_eq!(q["username"], "owner");
        assert_eq!(q["changedBy"], "Feishin");
        assert!(
            q["changed"].as_str().is_some_and(|s| s.ends_with('Z')),
            "{v}"
        );
        let v = call(owner, "getPlayQueue?".into()).await;
        assert_eq!(v["playQueue"]["current"], a, "{v}");

        // By id: an unknown song is left out, and the current one follows.
        let v = call(
            owner,
            format!("savePlayQueue?id={a}&id=999999&id={c}&current={c}&position=7"),
        )
        .await;
        assert_eq!(v["status"], "ok", "{v}");
        let v = call(owner, "getPlayQueueByIndex?".into()).await;
        assert_eq!(
            ids(&v["playQueueByIndex"]["entry"]),
            [a.as_str(), c.as_str()],
            "{v}"
        );
        assert_eq!(v["playQueueByIndex"]["currentIndex"], 1, "{v}");

        // A song gone from the library since is dropped on reading.
        {
            let db = Database::open(state.pool.path()).unwrap();
            db.conn
                .execute("DELETE FROM tracks WHERE id = ?1", [rc])
                .unwrap();
        }
        let v = call(owner, "getPlayQueue?".into()).await;
        assert_eq!(ids(&v["playQueue"]["entry"]), [a.as_str()], "{v}");
        assert_eq!(
            v["playQueue"]["current"], a,
            "a current entry while there are any: {v}"
        );
        let v = call(owner, "getPlayQueueByIndex?".into()).await;
        assert_eq!(v["playQueueByIndex"]["currentIndex"], 0, "{v}");

        // By index the current place is required with songs; an empty
        // position is the start.
        let v = call(owner, format!("savePlayQueueByIndex?id={a}")).await;
        assert_eq!(v["error"]["code"], 10, "{v}");
        let v = call(
            owner,
            format!("savePlayQueueByIndex?id={a}&id={b}&currentIndex=1&position="),
        )
        .await;
        assert_eq!(v["status"], "ok", "{v}");
        let v = call(owner, "getPlayQueueByIndex?".into()).await;
        assert_eq!(
            (
                v["playQueueByIndex"]["currentIndex"].clone(),
                v["playQueueByIndex"]["position"].clone()
            ),
            (1.into(), 0.into()),
            "{v}"
        );

        // Past the limit, refused whole. Asked of the handler directly: a
        // request this long is past what a URI holds, and a form is turned
        // into one before any endpoint sees it.
        let query = format!(
            "u=owner&p=sesame&v=1.16.1&c=x&f=json&{}",
            std::iter::repeat_n(format!("id={a}"), MAX_PLAY_QUEUE + 1)
                .collect::<Vec<_>>()
                .join("&")
        );
        let resp = save_play_queue_response(&state, &RawParams::parse(Some(&query)), false);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains("\"code\":10"), "{body}");
        let v = call(owner, "getPlayQueueByIndex?".into()).await;
        assert_eq!(
            v["playQueueByIndex"]["entry"].as_array().unwrap().len(),
            2,
            "unchanged: {v}"
        );

        // Another account has none; the shared secret is refused.
        let v = call("u=mate&p=hunter22", "getPlayQueue?".into()).await;
        assert_eq!(v["status"], "ok", "{v}");
        assert!(v.get("playQueue").is_none(), "{v}");
        let v = json_of(
            build_test_router(state.clone()),
            &format!("/rest/getPlayQueue?{}", auth_query("f=json")),
        )
        .await;
        assert_eq!(v["error"]["code"], 50, "{v}");

        // No songs clears it.
        call(owner, "savePlayQueue?".into()).await;
        let v = call(owner, "getPlayQueue?".into()).await;
        assert!(v.get("playQueue").is_none(), "{v}");

        // XML carries the same.
        call(owner, format!("savePlayQueue?id={b}&current={b}")).await;
        let body = get_response(
            build_test_router(state.clone()),
            "/rest/getPlayQueue?u=owner&p=sesame&v=1.16.1&c=x",
        )
        .await
        .1;
        assert!(body.contains(&format!(" current=\"{b}\"")), "{body}");
        assert_eq!(body.matches("<entry ").count(), 1, "{body}");

        let v = json_of(
            build_test_router(state.clone()),
            "/rest/getOpenSubsonicExtensions?f=json",
        )
        .await;
        assert!(v.to_string().contains("indexBasedQueue"), "{v}");
    }

    #[tokio::test]
    async fn bookmarks_save_replace_list_and_delete() {
        let (state, _dir) = test_state();
        let [alpha, beta, _] = seed_shelves(&state)[..] else {
            unreachable!()
        };
        let call = |path: String| {
            let state = state.clone();
            async move {
                json_of(
                    build_test_router(state),
                    &format!("/rest/{path}&{}", auth_query("f=json")),
                )
                .await
            }
        };
        let (alpha, beta) = (
            uid_of(&state, queries::UidKind::Track, alpha),
            uid_of(&state, queries::UidKind::Track, beta),
        );

        call(format!(
            "createBookmark?id={alpha}&position=1000&comment=intro"
        ))
        .await;
        call(format!("createBookmark?id={beta}&position=5000")).await;
        call(format!("createBookmark?id={alpha}&position=2500")).await;
        let v = call("getBookmarks?".into()).await;
        let marks = v["bookmarks"]["bookmark"].as_array().unwrap();
        assert_eq!(marks.len(), 2, "{v}");
        let mark = marks.iter().find(|m| m["entry"]["id"] == alpha).unwrap();
        assert_eq!(
            mark["position"], 2500,
            "a second bookmark replaces the first"
        );
        assert!(mark["comment"].is_null());
        assert_eq!(mark["username"], "testuser");
        assert!(mark["created"].is_string() && mark["changed"].is_string());

        let v = call(format!("createBookmark?id={alpha}")).await;
        assert_eq!(v["error"]["code"], 10, "position is required");

        call(format!("deleteBookmark?id={alpha}")).await;
        let v = call("getBookmarks?".into()).await;
        assert_eq!(v["bookmarks"]["bookmark"].as_array().unwrap().len(), 1);
        let v = call(format!("deleteBookmark?id={alpha}")).await;
        assert_eq!(v["status"], "ok", "deleting twice is not an error");

        let long = "x".repeat(MAX_BOOKMARK_COMMENT + 1);
        let v = call(format!(
            "createBookmark?id={beta}&position=1&comment={long}"
        ))
        .await;
        assert_eq!(v["error"]["code"], 10, "an overlong comment is refused");
        let v = call("getBookmarks?".into()).await;
        assert_eq!(v["bookmarks"]["bookmark"][0]["position"], 5000, "{v}");
    }
}
