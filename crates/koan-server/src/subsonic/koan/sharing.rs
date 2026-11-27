//! Sharing: links this server serves itself at `/share/{id}`.

use super::*;

pub(in crate::subsonic) fn iso(secs: i64) -> String {
    chrono::DateTime::from_timestamp(secs, 0)
        .unwrap_or_default()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

pub(in crate::subsonic) fn share_node(
    db: &Database,
    caller: &Caller,
    share: &queries::shares::ShareRow,
    base: &str,
    username: &str,
) -> Result<XmlNode, SubsonicError> {
    let rows = queries::tracks_by_ids(&db.conn, &share.track_ids)
        .map_err(|e| SubsonicError::internal(e.to_string()))?;
    let mut node = XmlNode::new("share")
        .array_of("entry")
        .attr("id", &share.id)
        .attr("url", &koan_core::helpers::share_url(base, &share.id))
        .attr("username", username)
        .attr("created", &iso(share.created_at))
        .attr_int("visitCount", share.visits)
        .attr_opt("description", share.description.as_deref());
    if let Some(e) = share.expires_at {
        node = node.attr("expires", &iso(e));
    }
    if let Some(v) = share.last_visited {
        node = node.attr("lastVisited", &iso(v));
    }
    let extras = song_extras(db, caller, &rows)?;
    for id in &share.track_ids {
        if let Some(t) = rows.iter().find(|t| t.id == *id) {
            node = node.child(track_node(t, "entry", &extras));
        }
    }
    Ok(node)
}

/// Where share links point; sharing is refused without it rather than
/// handing out an address that may not be reachable.
pub(in crate::subsonic) fn share_base(state: &AppState) -> Result<String, SubsonicError> {
    state.server_url().ok_or_else(|| {
        SubsonicError::generic("this server has no public address to share links on")
    })
}

/// `expires` is milliseconds since the epoch; 0 or absent never expires.
pub(in crate::subsonic) fn expires_param(params: &RawParams) -> Option<i64> {
    params
        .get("expires")
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|ms| *ms > 0)
        .map(|ms| ms / 1000)
}

/// What a `createShare` asks for. One song shares its album cued to it, an
/// album the album, an artist their albums; several ids stay a track list,
/// albums among them expanded in place.
pub(in crate::subsonic) fn share_target(
    db: &Database,
    ids: &[(Option<EntityKind>, i64)],
) -> Result<koan_core::helpers::ShareTarget, SubsonicError> {
    use koan_core::helpers::ShareTarget;
    Ok(match ids {
        [] => return Err(SubsonicError::missing_param("id")),
        [(Some(EntityKind::Artist), id)] => ShareTarget::Artist(*id),
        [(Some(EntityKind::Album), id)] => ShareTarget::Album {
            album_id: *id,
            start_track_id: None,
        },
        _ => {
            let mut track_ids = Vec::new();
            for (kind, id) in ids {
                match kind {
                    Some(EntityKind::Album) => track_ids.extend(
                        queries::tracks_for_album(&db.conn, *id)
                            .map_err(|e| SubsonicError::internal(e.to_string()))?
                            .into_iter()
                            .map(|t| t.id),
                    ),
                    Some(EntityKind::Song) | None => track_ids.push(*id),
                    Some(EntityKind::Artist) => return Err(SubsonicError::bad_param("id")),
                }
            }
            ShareTarget::Tracks(track_ids)
        }
    })
}

/// `createShare`, as a slice of the library: see `share_target`.
pub(in crate::subsonic) async fn create_share(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let auth = params.auth();
        respond_db_caller(&state, &auth, Role::User, |db, caller, b| {
            let user = caller.user_id;
            let base = share_base(&state)?;
            let ids: Vec<_> = params
                .all("id")
                .map(|raw| resolve_entity(db, raw))
                .collect::<Result<_, _>>()?;
            let target = share_target(db, &ids)?;
            let (slice, track_ids) =
                koan_core::helpers::resolve_share(&db.conn, &target).map_err(|e| match e {
                    koan_core::helpers::ShareError::NothingToShare => {
                        SubsonicError::not_found("Song")
                    }
                    e => SubsonicError::internal(e.to_string()),
                })?;
            let now = chrono::Utc::now().timestamp();
            let share = queries::shares::create_share(
                &db.conn,
                user,
                slice,
                &track_ids,
                params.get("description"),
                now,
                expires_param(&params),
            )
            .map_err(|e| SubsonicError::internal(e.to_string()))?;
            Ok(
                b.child(XmlNode::new("shares").array_of("share").child(share_node(
                    db,
                    caller,
                    &share,
                    &base,
                    &state.username,
                )?)),
            )
        })
    })
    .await
}

pub(in crate::subsonic) async fn get_shares(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    offload_response(move || {
        respond_db_caller(&state, &params, Role::Readonly, |db, caller, b| {
            let base = share_base(&state)?;
            let mut node = XmlNode::new("shares").array_of("share");
            for share in queries::shares::list_shares(&db.conn, caller.share_owner())
                .map_err(|e| SubsonicError::internal(e.to_string()))?
            {
                node = node.child(share_node(db, caller, &share, &base, &state.username)?);
            }
            Ok(b.child(node))
        })
    })
    .await
}

pub(in crate::subsonic) async fn update_share(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let auth = params.auth();
        respond_db_caller(&state, &auth, Role::User, |db, caller, b| {
            let id = params
                .get("id")
                .ok_or_else(|| SubsonicError::missing_param("id"))?;
            let found = queries::shares::update_share(
                &db.conn,
                caller.share_owner(),
                id,
                params.get("description"),
                expires_param(&params),
            )
            .map_err(|e| SubsonicError::internal(e.to_string()))?;
            if found {
                Ok(b)
            } else {
                Err(SubsonicError::not_found("Share"))
            }
        })
    })
    .await
}

pub(in crate::subsonic) async fn delete_share(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let auth = params.auth();
        respond_db_caller(&state, &auth, Role::User, |db, caller, b| {
            let id = params
                .get("id")
                .ok_or_else(|| SubsonicError::missing_param("id"))?;
            let found = queries::shares::delete_share(&db.conn, caller.share_owner(), id)
                .map_err(|e| SubsonicError::internal(e.to_string()))?;
            if found {
                Ok(b)
            } else {
                Err(SubsonicError::not_found("Share"))
            }
        })
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subsonic::testing::*;

    #[test]
    fn create_share_picks_the_slice_from_what_was_picked() {
        use koan_core::helpers::ShareTarget;
        let (state, _dir) = test_state();
        let db = Database::open(state.pool.path()).unwrap();
        let a = queries::upsert_track(&db.conn, &track_meta("/m/a.flac", "A", "One", 1)).unwrap();
        let b = queries::upsert_track(&db.conn, &track_meta("/m/b.flac", "B", "One", 2)).unwrap();
        let album = queries::tracks_by_ids(&db.conn, &[a]).unwrap()[0]
            .album_id
            .unwrap();
        let target = |raw: &[&str]| {
            let ids: Vec<_> = raw.iter().map(|r| parse_entity_id(r).unwrap()).collect();
            share_target(&db, &ids).ok()
        };
        assert_eq!(
            target(&[&format!("mf-{a}")]),
            Some(ShareTarget::Tracks(vec![a]))
        );
        assert_eq!(
            target(&[&format!("al-{album}")]),
            Some(ShareTarget::Album {
                album_id: album,
                start_track_id: None
            })
        );
        assert_eq!(target(&["ar-7"]), Some(ShareTarget::Artist(7)));
        assert_eq!(
            target(&[&format!("al-{album}"), &b.to_string()]),
            Some(ShareTarget::Tracks(vec![a, b, b]))
        );
        assert_eq!(target(&["ar-7", "ar-8"]), None);
        assert_eq!(target(&[]), None);
    }
}
