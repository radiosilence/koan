//! The server and its accounts as clients see them: ping, licence,
//! extensions, folders, the user, scan status and genres.

use super::*;

pub(super) async fn ping(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    offload_response(move || {
        respond(&state, &params, |_, mut b| {
            // The address clients know this server by: see `server.url`.
            if let Some(url) = &state.server_url() {
                b.root = b.root.attr("serverUrl", url);
            }
            Ok(b)
        })
    })
    .await
}

pub(super) async fn get_license(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    offload_response(move || {
        respond(&state, &params, |_, b| {
            Ok(b.child(
                XmlNode::new("license")
                    .attr_bool("valid", true)
                    .attr("email", "koan@localhost"),
            ))
        })
    })
    .await
}

pub(super) async fn get_music_folders(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    offload_response(move || {
        respond(&state, &params, |_, b| {
            Ok(b.child(
                XmlNode::new("musicFolders").list(
                    "musicFolder",
                    [XmlNode::new("musicFolder")
                        .attr_int("id", 1)
                        .attr("name", "Music")],
                ),
            ))
        })
    })
    .await
}

/// Clients call this during setup to decide which features to offer. It
/// reports the caller's own roles, whatever `username` asks about.
pub(super) async fn get_user(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    offload_response(move || {
        respond(&state, &params, |caller, b| {
            let role = caller.role;
            let writes = role.has_permission(Role::User);
            Ok(b.child(
                XmlNode::new("user")
                    .attr("username", &caller.username)
                    .attr_bool("scrobblingEnabled", writes)
                    .attr_bool("adminRole", role == Role::Admin)
                    .attr_bool("settingsRole", false)
                    .attr_bool("downloadRole", true)
                    .attr_bool("uploadRole", false)
                    .attr_bool("playlistRole", writes)
                    .attr_bool("coverArtRole", true)
                    .attr_bool("commentRole", false)
                    .attr_bool("podcastRole", false)
                    .attr_bool("streamRole", true)
                    .attr_bool("jukeboxRole", false)
                    .attr_bool("shareRole", writes)
                    .attr_bool("videoConversionRole", false),
            ))
        })
    })
    .await
}

/// Answered without authentication, as OpenSubsonic requires: a client asks
/// before it knows which sign-in methods it may use.
pub(super) async fn get_open_subsonic_extensions(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    // `timeOffset` seeks a transcode, so it is offered only where one can run.
    let transcode: &[(&str, &[i64])] = if state.transcoder.is_some() {
        &[("transcodeOffset", &[1])]
    } else {
        &[]
    };
    // An app's own server mounts no MCP.
    let mcp: &[(&str, &[i64])] = if state.server_url().is_some() && state.host_key.is_none() {
        &[(koan_core::remote::profile::MCP, &[1])]
    } else {
        &[]
    };
    let app_passwords: &[(&str, &[i64])] = if state.app_key.is_some() {
        &[(koan_core::remote::profile::APP_PASSWORDS, &[1])]
    } else {
        &[]
    };
    let push: &[(&str, &[i64])] = if state.push {
        &[(koan_core::remote::profile::PUSH, &[1])]
    } else {
        &[]
    };
    SubsonicResponse::ok(params.wants_json())
        .list(
            "openSubsonicExtensions",
            EXTENSIONS
                .iter()
                .chain(transcode)
                .chain(mcp)
                .chain(app_passwords)
                .chain(push)
                .map(|(name, versions)| {
                    XmlNode::new("openSubsonicExtensions")
                        .attr("name", name)
                        .list(
                            "versions",
                            versions
                                .iter()
                                .map(|v| XmlNode::scalar("versions", AttrValue::Int(*v))),
                        )
                }),
        )
        .build()
}

/// Who the credentials belong to. Meant for API keys, which carry no username.
pub(super) async fn token_info(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    offload_response(move || {
        respond(&state, &params, |caller, b| {
            Ok(b.child(XmlNode::new("tokenInfo").attr("username", &caller.username)))
        })
    })
    .await
}

/// Scans are driven by `koan scan`, never by a client, so this only ever
/// reports the library size. Clients poll it after a connection test.
pub(super) async fn get_scan_status(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    offload_response(move || {
        respond_db(&state, &params, |db, b| {
            // Clients poll this; the track count is all it reports.
            let count: i64 = db
                .conn
                .query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))
                .map_err(|e| SubsonicError::internal(e.to_string()))?;
            Ok(b.child(
                XmlNode::new("scanStatus")
                    .attr_bool("scanning", false)
                    .attr_int("count", count),
            ))
        })
    })
    .await
}

pub(super) async fn get_genres(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    offload_response(move || {
        respond_db(&state, &params, |db, b| {
            let mut stmt = db
                .conn
                .prepare(
                    "SELECT genre, COUNT(*), COUNT(DISTINCT album_id)
                 FROM tracks WHERE genre IS NOT NULL AND genre != ''
                 GROUP BY genre ORDER BY genre",
                )
                .map_err(|e| SubsonicError::internal(e.to_string()))?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                })
                .map_err(|e| SubsonicError::internal(e.to_string()))?;

            let mut genres_node = XmlNode::new("genres").array_of("genre");
            for row in rows {
                let (name, song_count, album_count) =
                    row.map_err(|e| SubsonicError::internal(e.to_string()))?;
                genres_node = genres_node.child(
                    XmlNode::new("genre")
                        .attr_int("songCount", song_count)
                        .attr_int("albumCount", album_count)
                        // The XSD carries the name as element text; `value` is the
                        // JSON spelling of the same thing.
                        .text(&name),
                );
            }

            Ok(b.child(genres_node))
        })
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subsonic::testing::*;

    #[tokio::test]
    async fn test_ping_ok() {
        let (state, _dir) = test_state();
        let app = build_test_router(state);
        let (status, body) = get_response(app, &format!("/rest/ping?{}", auth_query(""))).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("status=\"ok\""));
    }

    #[tokio::test]
    async fn test_ping_json() {
        let (state, _dir) = test_state();
        let app = build_test_router(state);
        let (status, body) =
            get_response(app, &format!("/rest/ping?{}", auth_query("f=json"))).await;
        assert_eq!(status, StatusCode::OK);
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["subsonic-response"]["status"], "ok");
    }

    /// A server that cannot seal app passwords does not offer them.
    #[tokio::test]
    async fn app_passwords_are_offered_only_with_a_sealing_key() {
        let (state, _dir) = test_state();
        let mut inner = Arc::try_unwrap(state).ok().unwrap();
        inner.app_key = None;
        let state = Arc::new(inner);
        let body = get_response(
            build_test_router(state.clone()),
            "/rest/getOpenSubsonicExtensions?f=json",
        )
        .await
        .1;
        assert!(!body.contains("koanAppPasswords"), "{body}");
        let body = post_form(
            build_test_router(state),
            "/rest/koanAppPasswords",
            "u=mate&p=hunter22&v=1.16.1&c=test&f=json",
        )
        .await;
        assert!(body.contains("\"code\":70"), "{body}");
    }

    /// Assistants are offered only where the server knows the address it is
    /// reached at: MCP's sign-in goes through it.
    #[tokio::test]
    async fn assistants_are_offered_with_a_public_address() {
        let extensions = |state: Arc<AppState>| async move {
            get_response(
                build_test_router(state),
                "/rest/getOpenSubsonicExtensions?f=json",
            )
            .await
            .1
        };
        let mcp = |state: Arc<AppState>, auth: &str| {
            let path = format!("/rest/koanMcp?{auth}&v=1.16.1&c=test&f=json");
            async move {
                let body = get_response(build_test_router(state), &path).await.1;
                serde_json::from_str::<serde_json::Value>(&body).unwrap()["subsonic-response"]
                    .clone()
            }
        };
        let mate = "u=mate&p=hunter22";

        let (state, _dir) = test_state();
        assert!(!extensions(state.clone()).await.contains("koanMcp"));
        let v = mcp(state.clone(), mate).await;
        assert_eq!(v["error"]["code"], 70, "{v}");

        let (state, _dir) = test_state();
        let mut inner = Arc::try_unwrap(state).ok().unwrap();
        inner.server_url = crate::ServerUrl::Fixed(Some("https://koan.example.com".into()));
        let state = Arc::new(inner);
        assert!(extensions(state.clone()).await.contains("koanMcp"));
        let v = mcp(state.clone(), mate).await;
        assert_eq!(v["mcp"]["url"], "https://koan.example.com/mcp", "{v}");
        assert_eq!(
            v["mcp"]["connect"], "https://koan.example.com/connect",
            "{v}"
        );
        let v = mcp(state, "u=mate&p=wrong").await;
        assert_eq!(v["error"]["code"], 40, "signed in only: {v}");
    }

    /// Notifications are offered only by a server that pushes.
    #[tokio::test]
    async fn notifications_are_offered_only_where_push_is_on() {
        let extensions = |state: Arc<AppState>| async move {
            get_response(
                build_test_router(state),
                "/rest/getOpenSubsonicExtensions?f=json",
            )
            .await
            .1
        };
        let (state, _dir) = test_state();
        assert!(!extensions(state).await.contains("koanPush"));

        let (state, _dir) = test_state();
        let mut inner = Arc::try_unwrap(state).ok().unwrap();
        inner.push = true;
        assert!(extensions(Arc::new(inner)).await.contains("koanPush"));
    }

    #[tokio::test]
    async fn test_get_license() {
        let (state, _dir) = test_state();
        let app = build_test_router(state);
        let (_, body) = get_response(app, &format!("/rest/getLicense?{}", auth_query(""))).await;
        assert!(body.contains("license"));
        assert!(body.contains("valid=\"true\""));
    }

    #[tokio::test]
    async fn test_get_music_folders() {
        let (state, _dir) = test_state();
        let app = build_test_router(state);
        let (_, body) =
            get_response(app, &format!("/rest/getMusicFolders?{}", auth_query(""))).await;
        assert!(body.contains("musicFolder"));
        assert!(body.contains("Music"));
    }

    #[tokio::test]
    async fn test_get_user_and_scan_status() {
        let (state, _dir) = test_state();
        seed_data(&state);

        let app = build_test_router(state.clone());
        let (_, body) = get_response(app, &format!("/rest/getUser?{}", auth_query(""))).await;
        assert!(body.contains("username=\"testuser\""));
        assert!(body.contains("streamRole=\"true\""));

        let app = build_test_router(state);
        let (_, body) = get_response(app, &format!("/rest/getScanStatus?{}", auth_query(""))).await;
        assert!(body.contains("scanning=\"false\""));
        assert!(body.contains("count=\"1\""));
    }

    #[tokio::test]
    async fn test_get_genres_xml_carries_name_as_text() {
        let (state, _dir) = test_state();
        seed_data(&state);

        let app = build_test_router(state.clone());
        let (_, body) = get_response(app, &format!("/rest/getGenres?{}", auth_query(""))).await;
        assert!(
            body.contains(">Rock</genre>"),
            "genre name must be element text: {}",
            body
        );

        // JSON spells the same value as a `value` member.
        let app = build_test_router(state);
        let (_, body) =
            get_response(app, &format!("/rest/getGenres?{}", auth_query("f=json"))).await;
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        let genre = &parsed["subsonic-response"]["genres"]["genre"][0];
        assert_eq!(genre["value"], "Rock");
        assert_eq!(genre["songCount"], serde_json::json!(1));
    }

    /// `ping` gives clients the address to know the server by, once it has one.
    #[tokio::test]
    async fn ping_gives_the_server_url() {
        let (state, _dir) = test_state();
        let ping = format!("/rest/ping?{}", auth_query("f=json"));
        let r = json_of(build_test_router(state), &ping).await;
        assert!(r.get("serverUrl").is_none(), "{r}");

        let (state, _dir) = test_state();
        let mut inner = Arc::try_unwrap(state).ok().unwrap();
        inner.server_url = crate::ServerUrl::Fixed(Some("https://koan.example.com".into()));
        let r = json_of(build_test_router(Arc::new(inner)), &ping).await;
        assert_eq!(r["serverUrl"], "https://koan.example.com", "{r}");
    }

    #[tokio::test]
    async fn test_extensions_need_no_auth() {
        let (state, _dir) = test_state();
        let r = json_of(
            build_test_router(state.clone()),
            "/rest/getOpenSubsonicExtensions?f=json",
        )
        .await;
        assert_envelope(&r, "ok");
        let listed: Vec<(String, Vec<i64>)> = r["openSubsonicExtensions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                (
                    e["name"].as_str().unwrap().to_owned(),
                    serde_json::from_value(e["versions"].clone()).unwrap(),
                )
            })
            .collect();
        // The test server can seal app passwords, so it offers them too.
        let expected: Vec<(String, Vec<i64>)> = EXTENSIONS
            .iter()
            .chain(&[(koan_core::remote::profile::APP_PASSWORDS, &[1][..])])
            .map(|(n, v)| (n.to_string(), v.to_vec()))
            .collect();
        assert_eq!(listed, expected);

        let (_, xml) = get_response(
            build_test_router(state),
            "/rest/getOpenSubsonicExtensions.view?u=nobody&p=wrong",
        )
        .await;
        assert!(xml.contains("status=\"ok\""), "{xml}");
        assert!(
            xml.contains("<openSubsonicExtensions name=\"formPost\">"),
            "{xml}"
        );
        assert!(xml.contains("<versions>1</versions>"), "{xml}");
    }
}
