//! Play history shared between an account's devices: `koanHistory`.

use super::*;

/// Tell the account's linked apps that its play history moved, so each reads
/// what changed (`koanHistory`).
pub(in crate::subsonic) fn history_changed(username: &str) {
    crate::clients::registry().broadcast(
        Some(username),
        koan_core::remote::link::LinkCommand::HistoryChanged,
    );
}

/// The caller's play history after `since` (`play.forgotten`, from the last
/// page; absent for the start), at most `count` plays and `count`
/// forgettings: koan's `koanHistory`. Times are ms since the epoch.
pub(in crate::subsonic) async fn koan_history(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let auth = params.auth();
        respond_db_user(&state, &auth, Role::Readonly, |db, user, b| {
            let since = match params.get("since") {
                Some(raw) => queries::HistoryCursor::parse(raw)
                    .ok_or_else(|| SubsonicError::bad_param("since"))?,
                None => queries::HistoryCursor::default(),
            };
            let count = params
                .get("count")
                .and_then(|c| c.parse::<u32>().ok())
                .unwrap_or(500)
                .clamp(1, 1000);
            let page = queries::history_since(&db.conn, user, since, count)
                .map_err(|e| SubsonicError::internal(e.to_string()))?;
            Ok(b.child(
                XmlNode::new("koanHistory")
                    .attr("cursor", &page.cursor.to_string())
                    .attr_bool("more", page.more)
                    .list(
                        "play",
                        page.plays.iter().map(|p| {
                            XmlNode::new("play")
                                .attr("id", &p.track_uid)
                                .attr_int("seq", p.seq)
                                .attr_int("played", p.played_at * 1000)
                                .attr_opt_int("listenedMs", p.listened_ms)
                        }),
                    )
                    .list(
                        "forgotten",
                        page.forgotten.iter().map(|f| {
                            XmlNode::new("forgotten")
                                .attr_opt("id", f.track_uid.as_deref())
                                .attr_int("played", f.played_at * 1000)
                        }),
                    ),
            ))
        })
    })
    .await
}

/// Forget plays from the caller's history, for every one of the account's
/// devices: each `id` with its `time` (ms, when the play started, as
/// `scrobble` takes it), or with `through` every play up to then. koan's
/// `koanForgetPlays`.
pub(in crate::subsonic) async fn koan_forget_plays(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let auth = params.auth();
        respond_db_caller(&state, &auth, Role::User, |db, caller, b| {
            let failed =
                |e: koan_core::db::connection::DbError| SubsonicError::internal(e.to_string());
            if let Some(through) = params.get("through") {
                let through: i64 = through
                    .parse()
                    .map_err(|_| SubsonicError::bad_param("through"))?;
                queries::forget_shared_plays_through(&db.conn, caller.user_id, through / 1000)
                    .map_err(failed)?;
            } else {
                let ids: Vec<&str> = params.all("id").collect();
                let times: Vec<i64> = params
                    .all("time")
                    .map(|t| t.parse().map_err(|_| SubsonicError::bad_param("time")))
                    .collect::<Result<_, _>>()?;
                if ids.is_empty() {
                    return Err(SubsonicError::missing_param("id"));
                }
                if ids.len() != times.len() {
                    return Err(SubsonicError::missing_param("time"));
                }
                // A track that has left the library took its plays with it.
                let plays: Vec<(i64, i64)> = ids
                    .iter()
                    .zip(&times)
                    .filter_map(|(raw, time)| {
                        resolve_as(db, raw, EntityKind::Song, "id")
                            .ok()
                            .map(|track| (track, time / 1000))
                    })
                    .collect();
                queries::forget_shared_plays(&db.conn, caller.user_id, &plays).map_err(failed)?;
            }
            use koan_core::smart::Field;
            crate::clients::smart_activity(
                db,
                caller.user_id,
                &[Field::PlayCount, Field::LastPlayed],
            );
            history_changed(&caller.username);
            Ok(b)
        })
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subsonic::testing::*;

    /// Plays scrobbled come back from `koanHistory` after the cursor; a play
    /// forgotten goes from the history and comes back as forgotten, and the
    /// next page from the cursor holds only what changed since.
    #[tokio::test]
    async fn history_pages_plays_and_forgettings_after_a_cursor() {
        let (state, _dir) = test_state();
        let shelves = seed_shelves(&state);
        let db = Database::open(state.pool.path()).unwrap();
        let (a, b) = (shelves[0], shelves[1]);
        let (a_uid, b_uid) = (
            uid_of(&state, queries::UidKind::Track, a),
            uid_of(&state, queries::UidKind::Track, b),
        );
        let get = |path: String| {
            let state = state.clone();
            async move { json_of(build_test_router(state), &path).await }
        };

        get(format!(
            "/rest/scrobble?{}&id={a_uid}&time=1000000&id={b_uid}&time=2000000",
            auth_query("f=json")
        ))
        .await;
        let v = get(format!("/rest/koanHistory?{}", auth_query("f=json"))).await;
        let page = &v["koanHistory"];
        let plays: Vec<(String, i64)> = page["play"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                (
                    p["id"].as_str().unwrap().to_owned(),
                    p["played"].as_i64().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            plays,
            [(a_uid.clone(), 1_000_000), (b_uid.clone(), 2_000_000)]
        );
        assert_eq!(page["more"], false);
        let cursor = page["cursor"].as_str().unwrap().to_owned();

        // Forgotten a second off: a play is named by when it started, and the
        // device that recorded it read its clock a moment apart.
        let v = get(format!(
            "/rest/koanForgetPlays?{}&id={a_uid}&time=1001000",
            auth_query("f=json")
        ))
        .await;
        assert_eq!(v["status"], "ok", "{v}");
        assert_eq!(
            queries::play_count(&db.conn, koan_core::db::queries::LOCAL_USER, a).unwrap(),
            0
        );
        let v = get(format!(
            "/rest/koanHistory?{}&since={cursor}",
            auth_query("f=json")
        ))
        .await;
        let page = &v["koanHistory"];
        assert!(page["play"].as_array().is_none_or(|p| p.is_empty()), "{v}");
        assert_eq!(page["forgotten"][0]["id"], a_uid.as_str());
        assert_eq!(page["forgotten"][0]["played"], 1_001_000);

        // Forgetting a play nobody has records nothing.
        let cursor = page["cursor"].as_str().unwrap().to_owned();
        get(format!(
            "/rest/koanForgetPlays?{}&id={a_uid}&time=9000000",
            auth_query("f=json")
        ))
        .await;
        let v = get(format!(
            "/rest/koanHistory?{}&since={cursor}",
            auth_query("f=json")
        ))
        .await;
        assert!(
            v["koanHistory"]["forgotten"]
                .as_array()
                .is_none_or(|f| f.is_empty()),
            "{v}"
        );

        // `through` forgets everything up to then, as one entry with no track.
        get(format!(
            "/rest/koanForgetPlays?{}&through=5000000",
            auth_query("f=json")
        ))
        .await;
        assert_eq!(
            queries::play_count(&db.conn, koan_core::db::queries::LOCAL_USER, b).unwrap(),
            0
        );
        let v = get(format!(
            "/rest/koanHistory?{}&since={cursor}",
            auth_query("f=json")
        ))
        .await;
        let forgotten = &v["koanHistory"]["forgotten"][0];
        assert!(forgotten.get("id").is_none_or(|id| id.is_null()), "{v}");
        assert_eq!(forgotten["played"], 5_000_000);
    }
}
