//! EQ profiles kept for an account on every device: `koanDspProfiles`.

use super::*;

/// EQ profiles are an account's, and only its password or an API key may
/// read or change them: not the shared secret, which is no account's, and
/// not an app password, which is handed to one other app.
pub(in crate::subsonic) fn dsp_account(caller: &Caller) -> Result<i64, SubsonicError> {
    match caller.via {
        Via::ApiKey | Via::Password => Ok(caller.user_id),
        Via::AppPassword | Via::SharedSecret => Err(SubsonicError::new(
            SubsonicErrorCode::NotAuthorized,
            "EQ is an account's: sign in with its password or an API key",
        )),
    }
}

pub(in crate::subsonic) fn dsp_failed(e: impl std::fmt::Display) -> SubsonicError {
    SubsonicError::internal(e.to_string())
}

/// Tell the account's linked apps that its EQ profiles moved, so each reads
/// what changed (`koanDspProfiles`).
pub(in crate::subsonic) fn dsp_profiles_changed(username: &str) {
    crate::clients::registry().broadcast(
        Some(username),
        koan_core::remote::link::LinkCommand::DspProfilesChanged,
    );
}

/// An edit's time as the server takes it: never later than the server's own
/// clock, so a device whose clock runs ahead cannot make its edits outrank
/// those a correct clock makes after it.
pub(in crate::subsonic) fn dsp_edited_at(params: &RawParams) -> Result<i64, SubsonicError> {
    let at: i64 = params
        .get("editedAt")
        .ok_or_else(|| SubsonicError::missing_param("editedAt"))?
        .parse()
        .map_err(|_| SubsonicError::new(SubsonicErrorCode::Generic, "editedAt is not a time"))?;
    Ok(at.min(chrono::Utc::now().timestamp_millis()))
}

pub(in crate::subsonic) fn dsp_uid(params: &RawParams) -> Result<String, SubsonicError> {
    let uid = params
        .get("uid")
        .ok_or_else(|| SubsonicError::missing_param("uid"))?;
    uuid::Uuid::parse_str(uid)
        .map(|u| u.to_string())
        .map_err(|_| SubsonicError::new(SubsonicErrorCode::Generic, "uid is not a UUID"))
}

/// The files the account's profiles name now, by hash, with their sizes;
/// `except` left out, and `with` added, for a profile being replaced.
pub(in crate::subsonic) fn dsp_named_files(
    conn: &rusqlite::Connection,
    user: i64,
    except: Option<&str>,
    with: Option<&koan_core::remote::dsp_sync::SyncDoc>,
) -> Result<HashMap<String, u64>, koan_core::db::connection::DbError> {
    use koan_core::remote::dsp_sync::SyncDoc;
    let mut named = HashMap::new();
    for (uid, json) in queries::dsp::live_docs(conn, user)? {
        if Some(uid.as_str()) == except {
            continue;
        }
        // Every stored document was checked on the way in.
        if let Ok(doc) = SyncDoc::parse(&json) {
            named.extend(doc.files.into_iter().map(|f| (f.sha256, f.size)));
        }
    }
    if let Some(doc) = with {
        named.extend(doc.files.iter().map(|f| (f.sha256.clone(), f.size)));
    }
    Ok(named)
}

/// Drop the files no profile names any more, a profile deleted in the last
/// thirty days included, and forget the documents of older deletions.
pub(in crate::subsonic) fn dsp_collect(
    conn: &rusqlite::Connection,
    user: i64,
) -> Result<(), koan_core::db::connection::DbError> {
    use koan_core::remote::dsp_sync::SyncDoc;
    let since = chrono::Utc::now().timestamp_millis() - queries::dsp::DELETED_KEPT_MS;
    queries::dsp::expire_deleted(conn, user, since)?;
    let named = dsp_named_files(conn, user, None, None)?;
    let mut keep: std::collections::HashSet<String> = named.into_keys().collect();
    for (_, _, json) in queries::dsp::deleted_docs(conn, user, since)? {
        if let Ok(doc) = SyncDoc::parse(&json) {
            keep.extend(doc.files.into_iter().map(|f| f.sha256));
        }
    }
    queries::dsp::keep_files(conn, user, &keep)?;
    Ok(())
}

pub(in crate::subsonic) fn dsp_saved_node(
    saved: queries::dsp::Saved,
    missing: &[String],
) -> XmlNode {
    XmlNode::new(Tag::KoanDspSaved)
        .attr_int("rev", saved.rev)
        .attr_bool("stored", saved.stored)
        .list(
            Tag::Missing,
            missing
                .iter()
                .map(|sha| XmlNode::new(Tag::Missing).attr("sha256", sha)),
        )
}

/// The caller's EQ profiles changed after revision `since` (0 for all),
/// deletions included, and the outputs it turned AutoEQ down for.
pub(in crate::subsonic) async fn koan_dsp_profiles(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let auth = params.auth();
        respond_db_caller(&state, &auth, Role::Readonly, |db, caller, b| {
            let user = dsp_account(caller)?;
            let since = params
                .get("since")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let (changed, cursor) =
                queries::dsp::changes(&db.conn, user, since).map_err(dsp_failed)?;
            let dismissed = queries::dsp::dismissed(&db.conn, user).map_err(dsp_failed)?;
            Ok(b.child(
                XmlNode::new(Tag::KoanDspProfiles)
                    .attr_int("cursor", cursor)
                    .list(
                        Tag::Profile,
                        changed.iter().map(|p| {
                            XmlNode::new(Tag::Profile)
                                .attr("uid", &p.uid)
                                .attr_int("rev", p.rev)
                                .attr_int("editedAt", p.edited_at)
                                .attr_opt("doc", p.doc.as_deref())
                        }),
                    )
                    .list(
                        Tag::Dismissed,
                        dismissed
                            .iter()
                            .map(|o| XmlNode::new(Tag::Dismissed).attr("output", o)),
                    ),
            ))
        })
    })
    .await
}

/// Keep profile `uid` as `doc`, edited at `editedAt` (ms), unless the copy
/// here was edited later. Answers with the files the server still needs.
/// A form POST only: a profile does not fit in a URL.
pub(in crate::subsonic) async fn koan_dsp_profile_save(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    use koan_core::remote::dsp_sync::{MAX_ACCOUNT, SyncDoc};
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let auth = params.auth();
        let mut username = None;
        let response = respond_db_caller(&state, &auth, Role::Readonly, |db, caller, b| {
            let user = dsp_account(caller)?;
            let uid = dsp_uid(&params)?;
            let edited_at = dsp_edited_at(&params)?;
            let json = params
                .get("doc")
                .ok_or_else(|| SubsonicError::missing_param("doc"))?;
            let doc = SyncDoc::parse(json)
                .map_err(|e| SubsonicError::new(SubsonicErrorCode::Generic, e))?;
            // The caps are checked inside the write transaction: two saves at
            // once each see the other's.
            let saved = queries::atomically(&db.conn, || {
                let (kept, known) =
                    queries::dsp::count(&db.conn, user, &uid).map_err(dsp_failed)?;
                if !known && kept >= koan_core::remote::dsp_sync::MAX_PROFILES {
                    return Err(SubsonicError::new(
                        SubsonicErrorCode::Generic,
                        "The account keeps as many EQs as it may",
                    ));
                }
                let named =
                    dsp_named_files(&db.conn, user, Some(&uid), Some(&doc)).map_err(dsp_failed)?;
                if named.values().sum::<u64>() > MAX_ACCOUNT {
                    return Err(SubsonicError::new(
                        SubsonicErrorCode::Generic,
                        format!(
                            "Too large to keep: the account's EQ files would pass {} MB",
                            MAX_ACCOUNT >> 20
                        ),
                    ));
                }
                let saved = queries::dsp::save(&db.conn, user, &uid, edited_at, Some(&doc.json()))
                    .map_err(dsp_failed)?;
                if saved.stored {
                    dsp_collect(&db.conn, user).map_err(dsp_failed)?;
                }
                Ok(saved)
            })?;
            let held = queries::dsp::files(&db.conn, user).map_err(dsp_failed)?;
            let missing: Vec<String> = if saved.stored {
                let mut m: Vec<String> = doc
                    .files
                    .iter()
                    .filter(|f| !held.contains_key(&f.sha256))
                    .map(|f| f.sha256.clone())
                    .collect();
                m.sort();
                m.dedup();
                m
            } else {
                Vec::new()
            };
            if saved.stored {
                username = Some(caller.username.clone());
            }
            Ok(b.child(dsp_saved_node(saved, &missing)))
        });
        if let Some(username) = username {
            dsp_profiles_changed(&username);
        }
        response
    })
    .await
}

/// Delete profile `uid` on every device, unless the copy here was edited
/// after `editedAt`.
pub(in crate::subsonic) async fn koan_dsp_profile_delete(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let auth = params.auth();
        let mut username = None;
        let response = respond_db_caller(&state, &auth, Role::Readonly, |db, caller, b| {
            let user = dsp_account(caller)?;
            let uid = dsp_uid(&params)?;
            let edited_at = dsp_edited_at(&params)?;
            let saved = queries::atomically(&db.conn, || {
                let saved = queries::dsp::save(&db.conn, user, &uid, edited_at, None)?;
                if saved.stored {
                    dsp_collect(&db.conn, user)?;
                }
                Ok::<_, koan_core::db::connection::DbError>(saved)
            })
            .map_err(dsp_failed)?;
            if saved.stored {
                username = Some(caller.username.clone());
            }
            Ok(b.child(dsp_saved_node(saved, &[])))
        });
        if let Some(username) = username {
            dsp_profiles_changed(&username);
        }
        response
    })
    .await
}

/// The caller's EQ profiles deleted in the last thirty days, newest first:
/// each one's uid, name, when the server recorded the deletion and when it
/// will forget it (ms).
pub(in crate::subsonic) async fn koan_dsp_deleted(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    use koan_core::remote::dsp_sync::SyncDoc;
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        respond_db_caller(&state, &params.auth(), Role::Readonly, |db, caller, b| {
            let user = dsp_account(caller)?;
            let since = chrono::Utc::now().timestamp_millis() - queries::dsp::DELETED_KEPT_MS;
            let deleted = queries::dsp::deleted_docs(&db.conn, user, since).map_err(dsp_failed)?;
            Ok(b.child(XmlNode::new(Tag::KoanDspDeleted).list(
                Tag::Profile,
                deleted.iter().filter_map(|(uid, at, json)| {
                    let doc = SyncDoc::parse(json).ok()?;
                    Some(
                        XmlNode::new(Tag::Profile)
                            .attr("uid", uid)
                            .attr("name", &doc.profile.name)
                            .attr_int("deletedAt", *at)
                            .attr_int("expiresAt", at + queries::dsp::DELETED_KEPT_MS),
                    )
                }),
            )))
        })
    })
    .await
}

/// Save deleted profile `uid` again from the copy the server kept, as an
/// edit made now, so every device adopts it.
pub(in crate::subsonic) async fn koan_dsp_restore(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    use koan_core::remote::dsp_sync::{MAX_ACCOUNT, SyncDoc};
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let auth = params.auth();
        let mut username = None;
        let response = respond_db_caller(&state, &auth, Role::Readonly, |db, caller, b| {
            let user = dsp_account(caller)?;
            let uid = dsp_uid(&params)?;
            let now = chrono::Utc::now().timestamp_millis();
            let saved = queries::atomically(&db.conn, || {
                let (deleted_at, json) = queries::dsp::deleted_doc(
                    &db.conn,
                    user,
                    &uid,
                    now - queries::dsp::DELETED_KEPT_MS,
                )
                .map_err(dsp_failed)?
                .ok_or_else(|| SubsonicError::not_found("Deleted EQ"))?;
                let doc = SyncDoc::parse(&json).map_err(dsp_failed)?;
                // A file never uploaded before the deletion would leave every
                // device unable to take the profile, and its sync stuck there.
                let held = queries::dsp::files(&db.conn, user).map_err(dsp_failed)?;
                if doc.files.iter().any(|f| !held.contains_key(&f.sha256)) {
                    return Err(SubsonicError::new(
                        SubsonicErrorCode::Generic,
                        "Its files never reached the server, so it cannot be restored",
                    ));
                }
                let named =
                    dsp_named_files(&db.conn, user, Some(&uid), Some(&doc)).map_err(dsp_failed)?;
                if named.values().sum::<u64>() > MAX_ACCOUNT {
                    return Err(SubsonicError::new(
                        SubsonicErrorCode::Generic,
                        format!(
                            "Too large to restore: the account's EQ files would pass {} MB",
                            MAX_ACCOUNT >> 20
                        ),
                    ));
                }
                // Later than the deletion, whatever clock the deleting device
                // kept.
                let edited_at = now.max(deleted_at + 1);
                let saved = queries::dsp::save(&db.conn, user, &uid, edited_at, Some(&json))
                    .map_err(dsp_failed)?;
                dsp_collect(&db.conn, user).map_err(dsp_failed)?;
                Ok(saved)
            })?;
            username = Some(caller.username.clone());
            Ok(b.child(dsp_saved_node(saved, &[])))
        });
        if let Some(username) = username {
            dsp_profiles_changed(&username);
        }
        response
    })
    .await
}

/// Turn the AutoEQ suggestion for `output` down on every device.
pub(in crate::subsonic) async fn koan_dsp_dismiss(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let auth = params.auth();
        let mut username = None;
        let response = respond_db_caller(&state, &auth, Role::Readonly, |db, caller, b| {
            let user = dsp_account(caller)?;
            let output = params
                .get("output")
                .filter(|o| !o.trim().is_empty() && o.len() <= 256)
                .ok_or_else(|| SubsonicError::missing_param("output"))?;
            let held = queries::dsp::dismissed_count(&db.conn, user).map_err(dsp_failed)?;
            if held >= koan_core::remote::dsp_sync::MAX_DISMISSED {
                return Ok(b);
            }
            if queries::dsp::dismiss(&db.conn, user, output).map_err(dsp_failed)? {
                username = Some(caller.username.clone());
            }
            Ok(b)
        });
        if let Some(username) = username {
            dsp_profiles_changed(&username);
        }
        response
    })
    .await
}

/// A file one of the caller's profiles names, by `sha256`.
pub(in crate::subsonic) async fn koan_dsp_file(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let auth = params.auth();
        let json = auth.wants_json();
        let found = validate_auth(&auth, &state)
            .and_then(|caller| dsp_account(&caller))
            .and_then(|user| {
                let sha = params
                    .get("sha256")
                    .filter(|s| koan_core::remote::dsp_sync::is_sha256(s))
                    .ok_or_else(|| SubsonicError::missing_param("sha256"))?;
                let db = state.open_db()?;
                queries::dsp::file(&db.conn, user, sha)
                    .map_err(dsp_failed)?
                    .ok_or_else(|| SubsonicError::not_found("File"))
            });
        match found {
            Ok(bytes) => {
                ([(header::CONTENT_TYPE, "application/octet-stream")], bytes).into_response()
            }
            Err(e) => SubsonicResponse::error(json, &e),
        }
    })
    .await
}

/// Keep a file one of the caller's profiles names and the server lacks: the
/// raw bytes, checked against `sha256` and the size the profile gave. The
/// caller is known before any of the body is read.
pub(in crate::subsonic) async fn koan_dsp_file_upload(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
    body: axum::body::Body,
) -> Response {
    use koan_core::remote::dsp_sync::{MAX_FILE, is_sha256, sha256_hex};
    let asked = {
        let state = state.clone();
        let raw = raw.clone();
        tokio::task::spawn_blocking(move || {
            let params = RawParams::parse(raw.as_deref());
            let auth = params.auth();
            let json = auth.wants_json();
            let caller = validate_auth(&auth, &state)
                .and_then(|caller| Ok((dsp_account(&caller)?, caller.username)));
            (json, caller)
        })
        .await
    };
    let Ok((json, caller)) = asked else {
        return SubsonicResponse::error(false, &SubsonicError::generic("upload failed"));
    };
    let (user, username) = match caller {
        Ok(c) => c,
        Err(e) => return SubsonicResponse::error(json, &e),
    };
    let Ok(bytes) = axum::body::to_bytes(body, MAX_FILE as usize).await else {
        return SubsonicResponse::error(
            json,
            &SubsonicError::new(
                SubsonicErrorCode::Generic,
                format!("A file may be at most {} MB", MAX_FILE >> 20),
            ),
        );
    };
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let stored = (|| {
            let sha = params
                .get("sha256")
                .filter(|s| is_sha256(s))
                .ok_or_else(|| SubsonicError::missing_param("sha256"))?;
            if sha256_hex(&bytes) != sha {
                return Err(SubsonicError::new(
                    SubsonicErrorCode::Generic,
                    "The file does not match its SHA-256",
                ));
            }
            let db = state.open_db()?;
            // Named and stored in one transaction: a deletion's collection
            // cannot land between, leaving a file no profile names.
            queries::atomically(&db.conn, || {
                let named = dsp_named_files(&db.conn, user, None, None).map_err(dsp_failed)?;
                if named.get(sha) != Some(&(bytes.len() as u64)) {
                    return Err(SubsonicError::new(
                        SubsonicErrorCode::Generic,
                        "No profile of the account names this file",
                    ));
                }
                queries::dsp::store_file(&db.conn, user, sha, &bytes).map_err(dsp_failed)
            })
        })();
        match stored {
            Ok(()) => {
                dsp_profiles_changed(&username);
                SubsonicResponse::ok(json).build()
            }
            Err(e) => SubsonicResponse::error(json, &e),
        }
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subsonic::testing::*;
    use axum::http::Request;

    /// Send `body` as a profile's file, as `koan_dsp_upload` does.
    async fn upload_dsp(app: axum::Router, query: &str, body: Vec<u8>) -> String {
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/rest/koanDspFile?{query}"))
                    .header(header::CONTENT_TYPE, "application/octet-stream")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    fn dsp_doc(name: &str, file: &[u8]) -> koan_core::remote::dsp_sync::SyncDoc {
        use koan_core::remote::dsp_sync::{SyncDoc, SyncFile, sha256_hex};
        SyncDoc {
            profile: koan_core::config::DspProfile {
                name: name.into(),
                impulses: vec!["48000.wav".into()],
                ..Default::default()
            },
            files: vec![SyncFile {
                name: "48000.wav".into(),
                sha256: sha256_hex(file),
                size: file.len() as u64,
            }],
            unknown: Default::default(),
        }
    }

    fn form_value(s: &str) -> String {
        form_urlencoded::byte_serialize(s.as_bytes()).collect()
    }

    const UID: &str = "0199b5a2-6c1e-7cc3-9d2a-3f5b1e0c4d21";

    /// Profiles are an account's: the shared secret and an app password get
    /// none of them, the account's password and its API keys get all.
    #[tokio::test]
    async fn dsp_profiles_are_the_accounts_own() {
        let (state, _dir) = test_state();
        let app_password = {
            let db = Database::open(state.pool.path()).unwrap();
            queries::app_passwords::create_app_password(
                &db.conn,
                state.app_key.as_ref().unwrap(),
                1,
                "arpeggi",
            )
            .unwrap()
            .1
        };
        let key = api_key(&state, "mate");
        let app = build_test_router(state);
        let refused = [
            auth_query(""),
            format!("u=mate&p={app_password}&v=1.16.1&c=test"),
        ];
        for creds in &refused {
            let (_, body) =
                get_response(app.clone(), &format!("/rest/koanDspProfiles?{creds}")).await;
            assert!(body.contains("code=\"50\""), "{creds}: {body}");
            let (_, body) = get_response(
                app.clone(),
                &format!("/rest/koanDspFile?{creds}&sha256={}", "a".repeat(64)),
            )
            .await;
            assert!(body.contains("code=\"50\""), "{creds}: {body}");
            let body = upload_dsp(
                app.clone(),
                &format!("{creds}&sha256={}", "a".repeat(64)),
                vec![1],
            )
            .await;
            assert!(body.contains("code=\"50\""), "{creds}: {body}");
        }
        for creds in [
            MATE.to_owned(),
            format!("apiKey={key}&v=1.16.1&c=test&f=json"),
        ] {
            let (_, body) =
                get_response(app.clone(), &format!("/rest/koanDspProfiles?{creds}")).await;
            assert!(body.contains("\"cursor\":0"), "{creds}: {body}");
        }
        let (status, _) = get_response(
            app,
            &format!("/rest/koanDspProfileSave?{MATE}&uid={UID}&editedAt=1"),
        )
        .await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    }

    /// A deleted profile is listed and restored for its own account only,
    /// and comes back as a new edit every device adopts; one past the thirty
    /// days is neither.
    #[tokio::test]
    async fn a_deleted_dsp_profile_is_listed_and_restored_by_its_account() {
        let (state, _dir) = test_state();
        let owner = format!("apiKey={}&v=1.16.1&c=test&f=json", api_key(&state, "owner"));
        let path = state.pool.path().to_owned();
        let app = build_test_router(state);
        let wav = b"RIFF and the rest";
        let doc = dsp_doc("Room", wav);
        let call = |path: String, form: String| {
            let app = app.clone();
            async move { json(&post_form(app, &path, &form).await) }
        };
        let deleted = |creds: String| {
            let app = app.clone();
            async move {
                json(
                    &get_response(app, &format!("/rest/koanDspDeleted?{creds}"))
                        .await
                        .1,
                )["subsonic-response"]["koanDspDeleted"]["profile"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
            }
        };
        let restore = |creds: String| {
            call(
                format!("/rest/koanDspRestore?{creds}"),
                format!("uid={UID}"),
            )
        };

        call(
            format!("/rest/koanDspProfileSave?{MATE}"),
            format!("uid={UID}&editedAt=1000&doc={}", form_value(&doc.json())),
        )
        .await;
        assert!(deleted(MATE.into()).await.is_empty(), "nothing deleted yet");

        // One whose file never arrived is not brought back.
        call(
            format!("/rest/koanDspProfileDelete?{MATE}"),
            format!("uid={UID}&editedAt=1500"),
        )
        .await;
        let v = restore(MATE.into()).await;
        assert!(
            v["subsonic-response"]["error"]["message"]
                .as_str()
                .unwrap()
                .contains("never reached the server"),
            "{v}"
        );
        call(
            format!("/rest/koanDspProfileSave?{MATE}"),
            format!("uid={UID}&editedAt=1600&doc={}", form_value(&doc.json())),
        )
        .await;
        let body = upload_dsp(
            app.clone(),
            &format!("{MATE}&sha256={}", doc.files[0].sha256),
            wav.to_vec(),
        )
        .await;
        assert!(body.contains("\"status\":\"ok\""), "{body}");

        // Deleted by a device whose clock runs far ahead.
        let before = chrono::Utc::now().timestamp_millis();
        let ahead = chrono::Utc::now().timestamp_millis() + 50_000;
        call(
            format!("/rest/koanDspProfileDelete?{MATE}"),
            format!("uid={UID}&editedAt={ahead}"),
        )
        .await;

        let listed = deleted(MATE.into()).await;
        assert_eq!(listed.len(), 1, "{listed:?}");
        assert_eq!(listed[0]["uid"], UID);
        assert_eq!(listed[0]["name"], "Room");
        assert_eq!(
            listed[0]["expiresAt"].as_i64().unwrap() - listed[0]["deletedAt"].as_i64().unwrap(),
            queries::dsp::DELETED_KEPT_MS
        );

        // Another account sees none of it and cannot restore it.
        assert!(deleted(owner.clone()).await.is_empty());
        let v = restore(owner).await;
        assert_eq!(v["subsonic-response"]["error"]["code"], 70, "{v}");

        let v = restore(MATE.into()).await;
        assert_eq!(
            v["subsonic-response"]["koanDspSaved"]["stored"], true,
            "{v}"
        );
        let (_, body) = get_response(app.clone(), &format!("/rest/koanDspProfiles?{MATE}")).await;
        let profile = &json(&body)["subsonic-response"]["koanDspProfiles"]["profile"][0];
        assert_eq!(profile["doc"], doc.json().as_str(), "{body}");
        assert!(profile["editedAt"].as_i64().unwrap() > before, "{body}");
        assert!(deleted(MATE.into()).await.is_empty(), "restored");
        let v = restore(MATE.into()).await;
        assert_eq!(
            v["subsonic-response"]["error"]["code"], 70,
            "not deleted: {v}"
        );

        // Past the thirty days: not listed, not restorable.
        call(
            format!("/rest/koanDspProfileDelete?{MATE}"),
            format!("uid={UID}&editedAt={}", ahead + 100_000),
        )
        .await;
        Database::open(&path)
            .unwrap()
            .conn
            .execute(
                "UPDATE dsp_profiles SET deleted_at = ?1",
                [chrono::Utc::now().timestamp_millis() - queries::dsp::DELETED_KEPT_MS - 1],
            )
            .unwrap();
        assert!(deleted(MATE.into()).await.is_empty());
        let v = restore(MATE.into()).await;
        assert_eq!(v["subsonic-response"]["error"]["code"], 70, "expired: {v}");
    }

    /// An edit stamped ahead of the server's clock counts as made now, so an
    /// edit a correct clock makes after it still wins.
    #[tokio::test]
    async fn a_dsp_edit_stamped_ahead_does_not_outrank_a_later_correct_one() {
        let (state, _dir) = test_state();
        let app = build_test_router(state);
        let wav = b"RIFF and the rest";
        let doc = dsp_doc("Room", wav);
        let stored = |at: i64| {
            let app = app.clone();
            let form = format!("uid={UID}&editedAt={at}&doc={}", form_value(&doc.json()));
            async move {
                json(&post_form(app, &format!("/rest/koanDspProfileSave?{MATE}"), &form).await)
                    ["subsonic-response"]["koanDspSaved"]["stored"]
                    .clone()
            }
        };
        let now = chrono::Utc::now().timestamp_millis();
        assert_eq!(stored(now + 50_000).await, true, "stamped ahead");
        let later = chrono::Utc::now().timestamp_millis();
        assert_eq!(stored(later).await, true, "a correct clock, later");
    }

    /// A file is taken only as the bytes a profile of the account names, and
    /// is read back only by that account. A deleted profile's files stay with
    /// the copy the server keeps of it (`queries::dsp::DELETED_KEPT_MS`).
    #[tokio::test]
    async fn a_dsp_file_is_kept_only_while_a_profile_names_it() {
        let (state, _dir) = test_state();
        let owner = api_key(&state, "owner");
        let app = build_test_router(state);
        let wav = b"RIFF and the rest".to_vec();
        let doc = dsp_doc("Room", &wav);
        let sha = doc.files[0].sha256.clone();

        let body = post_form(
            app.clone(),
            &format!("/rest/koanDspProfileSave?{MATE}"),
            &format!("uid={UID}&editedAt=1000&doc={}", form_value(&doc.json())),
        )
        .await;
        let v = json(&body);
        let saved = &v["subsonic-response"]["koanDspSaved"];
        assert_eq!(saved["stored"], true, "{body}");
        assert_eq!(saved["missing"][0]["sha256"], sha.as_str(), "{body}");

        let wrong = upload_dsp(
            app.clone(),
            &format!("{MATE}&sha256={sha}"),
            b"something else".to_vec(),
        )
        .await;
        assert!(wrong.contains("does not match"), "{wrong}");
        let stray = b"not named".to_vec();
        let stray_sha = koan_core::remote::dsp_sync::sha256_hex(&stray);
        let body = upload_dsp(app.clone(), &format!("{MATE}&sha256={stray_sha}"), stray).await;
        assert!(body.contains("No profile of the account names"), "{body}");
        let body = upload_dsp(app.clone(), &format!("{MATE}&sha256={sha}"), wav.clone()).await;
        assert!(body.contains("\"status\":\"ok\""), "{body}");

        let (_, back) = get_response(
            app.clone(),
            &format!("/rest/koanDspFile?{MATE}&sha256={sha}"),
        )
        .await;
        assert_eq!(back.as_bytes(), &wav[..]);
        let (_, theirs) = get_response(
            app.clone(),
            &format!("/rest/koanDspFile?apiKey={owner}&v=1.16.1&c=test&sha256={sha}"),
        )
        .await;
        assert!(
            theirs.contains("code=\"70\""),
            "another account's: {theirs}"
        );

        // An older edit is not kept; a later deletion is, and keeps the file.
        let body = post_form(
            app.clone(),
            &format!("/rest/koanDspProfileDelete?{MATE}"),
            &format!("uid={UID}&editedAt=500"),
        )
        .await;
        assert_eq!(
            json(&body)["subsonic-response"]["koanDspSaved"]["stored"],
            false,
            "{body}"
        );
        post_form(
            app.clone(),
            &format!("/rest/koanDspProfileDelete?{MATE}"),
            &format!("uid={UID}&editedAt=2000"),
        )
        .await;
        let (_, kept) = get_response(
            app.clone(),
            &format!("/rest/koanDspFile?{MATE}&sha256={sha}"),
        )
        .await;
        assert_eq!(kept.as_bytes(), &wav[..], "kept with the deleted copy");
        let (_, body) = get_response(app, &format!("/rest/koanDspProfiles?{MATE}")).await;
        let v = json(&body);
        let profile = &v["subsonic-response"]["koanDspProfiles"]["profile"][0];
        assert_eq!(profile["uid"], UID);
        assert!(profile.get("doc").is_none(), "deleted: {body}");
    }

    /// Past the caps, nothing is kept: a file over 32 MB in a profile, or
    /// sent as one.
    #[tokio::test]
    async fn dsp_files_past_the_cap_are_refused() {
        use koan_core::remote::dsp_sync::MAX_FILE;
        let (state, _dir) = test_state();
        let app = build_test_router(state);
        let mut doc = dsp_doc("Room", b"x");
        doc.files[0].size = MAX_FILE + 1;
        let body = post_form(
            app.clone(),
            &format!("/rest/koanDspProfileSave?{MATE}"),
            &format!("uid={UID}&editedAt=1&doc={}", form_value(&doc.json())),
        )
        .await;
        assert!(body.contains("larger than the 32 MB"), "{body}");
        let body = upload_dsp(
            app,
            &format!("{MATE}&sha256={}", "a".repeat(64)),
            vec![0; MAX_FILE as usize + 1],
        )
        .await;
        assert!(body.contains("at most 32 MB"), "{body}");
    }

    /// An account holds at most `MAX_PROFILES`, checked in the transaction
    /// that saves: a profile it already has may still change.
    #[tokio::test]
    async fn an_account_keeps_a_bounded_number_of_profiles() {
        use koan_core::remote::dsp_sync::MAX_PROFILES;
        let (state, _dir) = test_state();
        {
            let db = Database::open(state.pool.path()).unwrap();
            for i in 0..MAX_PROFILES {
                queries::dsp::save(&db.conn, 1, &format!("uid-{i}"), 1, None).unwrap();
            }
        }
        let app = build_test_router(state);
        let doc = koan_core::remote::dsp_sync::SyncDoc {
            profile: koan_core::config::DspProfile {
                name: "One more".into(),
                ..Default::default()
            },
            files: vec![],
            unknown: Default::default(),
        };
        let body = post_form(
            app,
            &format!("/rest/koanDspProfileSave?{MATE}"),
            &format!("uid={UID}&editedAt=1&doc={}", form_value(&doc.json())),
        )
        .await;
        assert!(body.contains("as many EQs as it may"), "{body}");
    }
}
