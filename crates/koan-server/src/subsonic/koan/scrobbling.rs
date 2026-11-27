//! An account's scrobbling services: `koanScrobbling`.

use super::*;

/// The caller's scrobbling, as `koanScrobbling` answers it: one `service` per
/// connected service. The token never leaves the server.
pub(in crate::subsonic) fn scrobbling_node(
    services: &[koan_core::db::queries::scrobbling::ScrobbleService],
) -> XmlNode {
    XmlNode::new("koanScrobbling").list(
        "service",
        services.iter().map(|s| {
            XmlNode::new("service")
                .attr("name", &s.service)
                .attr("account", &s.account_name)
                .attr_int("connected", s.connected_at * 1000)
                .attr_int("pending", s.pending)
                .attr_opt("error", s.error.as_deref())
        }),
    )
}

/// Scrobbling is an account's: the shared secret has no plays of its own to
/// send anywhere.
pub(in crate::subsonic) fn scrobbling_account(caller: &Caller) -> Result<i64, SubsonicError> {
    if caller.via == Via::SharedSecret {
        return Err(SubsonicError::new(
            SubsonicErrorCode::NotAuthorized,
            "Scrobbling belongs to accounts; sign in as one",
        ));
    }
    Ok(caller.user_id)
}

/// Where the caller's plays are forwarded: koan's `koanScrobbling`.
pub(in crate::subsonic) async fn koan_scrobbling(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let auth = params.auth();
        respond_db_caller(&state, &auth, Role::Readonly, |db, caller, b| {
            let user = scrobbling_account(caller)?;
            let services = koan_core::db::queries::scrobbling::services(&db.conn, user)
                .map_err(|e| SubsonicError::internal(e.to_string()))?;
            Ok(b.child(scrobbling_node(&services)))
        })
    })
    .await
}

/// Connect the caller's ListenBrainz account with `token`, checked with
/// ListenBrainz first, as the web UI's Scrobbling page does. Answers as
/// `koanScrobbling`. The token is read from a form POST's body only, never
/// the query string, which proxies log.
pub(in crate::subsonic) async fn koan_scrobbling_connect(
    State(state): State<Arc<AppState>>,
    form: Option<axum::Extension<FormBody>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let auth = params.auth();
        // No connection is held while ListenBrainz answers.
        respond(&state, &auth, |caller, b| {
            let user = scrobbling_account(caller)?;
            let form = RawParams::parse(form.as_ref().map(|f| &*f.0.0));
            let pasted = form
                .get("token")
                .ok_or_else(|| SubsonicError::missing_param("token"))?;
            // Each `ConnectError` is written for the person who pasted the token.
            let refused =
                |e: koan_core::scrobbling::ConnectError| SubsonicError::generic(e.to_string());
            let (token, name) =
                koan_core::scrobbling::check_listenbrainz_token(pasted).map_err(refused)?;
            let db = state.open_db()?;
            koan_core::scrobbling::connect_listenbrainz(&db.conn, user, &token, &name)
                .map_err(refused)?;
            let services = koan_core::db::queries::scrobbling::services(&db.conn, user)
                .map_err(|e| SubsonicError::internal(e.to_string()))?;
            Ok(b.child(scrobbling_node(&services)))
        })
    })
    .await
}

/// Stop forwarding the caller's plays to `service` (`listenbrainz`, the
/// default, or `lastfm`); what was waiting is dropped. Answers as
/// `koanScrobbling`.
pub(in crate::subsonic) async fn koan_scrobbling_disconnect(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let auth = params.auth();
        respond_db_caller(&state, &auth, Role::Readonly, |db, caller, b| {
            use koan_core::db::queries::scrobbling;
            let user = scrobbling_account(caller)?;
            let failed = |e: rusqlite::Error| SubsonicError::internal(e.to_string());
            let service = match params.get("service") {
                None | Some(scrobbling::LISTENBRAINZ) => scrobbling::LISTENBRAINZ,
                Some(scrobbling::LASTFM) => scrobbling::LASTFM,
                Some(_) => return Err(SubsonicError::bad_param("service")),
            };
            scrobbling::disconnect(&db.conn, user, service).map_err(failed)?;
            let services = scrobbling::services(&db.conn, user).map_err(failed)?;
            Ok(b.child(scrobbling_node(&services)))
        })
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subsonic::testing::*;

    /// ListenBrainz, as far as checking a token goes: `good-token` belongs to
    /// `lbuser`, anything else is refused. One for the whole process, since
    /// where ListenBrainz is reached is.
    fn fake_listenbrainz() {
        use std::io::{BufRead, BufReader, Write};
        static STARTED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        STARTED.get_or_init(|| {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            koan_core::scrobbling::use_api(&format!("http://{}", listener.local_addr().unwrap()));
            std::thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut good = false;
                    let mut line = String::new();
                    while reader.read_line(&mut line).is_ok_and(|n| n > 0) && line != "\r\n" {
                        good |= line.eq_ignore_ascii_case("authorization: Token good-token\r\n");
                        line.clear();
                    }
                    let (status, body) = if good {
                        ("200 OK", r#"{"code":200,"valid":true,"user_name":"lbuser"}"#)
                    } else {
                        ("401 Unauthorized", r#"{"code":401,"valid":false}"#)
                    };
                    let _ = write!(
                        &stream,
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                }
            });
        });
    }

    #[tokio::test]
    async fn scrobbling_refuses_a_token_listenbrainz_refuses() {
        fake_listenbrainz();
        let (state, _dir) = test_state();
        let app = build_test_router(state);
        let body = post_form(
            app.clone(),
            &format!("/rest/koanScrobblingConnect?{MATE}"),
            "token=wrong",
        )
        .await;
        assert!(body.contains("does not accept this token"), "{body}");
        let (_, body) = get_response(app, &format!("/rest/koanScrobbling?{MATE}")).await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let services = &v["subsonic-response"]["koanScrobbling"]["service"];
        assert!(services.as_array().is_none_or(|s| s.is_empty()), "{body}");
    }

    #[tokio::test]
    async fn scrobbling_connects_and_disconnects_without_giving_the_token_back() {
        fake_listenbrainz();
        let (state, _dir) = test_state();
        let app = build_test_router(state);
        let body = post_form(
            app.clone(),
            &format!("/rest/koanScrobblingConnect?{MATE}"),
            "token=%20good-token%20",
        )
        .await;
        assert!(!body.contains("good-token"), "{body}");
        let (_, body) = get_response(app.clone(), &format!("/rest/koanScrobbling?{MATE}")).await;
        assert!(!body.contains("good-token"), "{body}");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let service = &v["subsonic-response"]["koanScrobbling"]["service"][0];
        assert_eq!(service["name"], "listenbrainz", "{body}");
        assert_eq!(service["account"], "lbuser", "{body}");
        assert_eq!(service["pending"], 0, "{body}");

        let (_, body) = get_response(
            app.clone(),
            &format!("/rest/koanScrobblingDisconnect?{MATE}"),
        )
        .await;
        assert!(body.contains("\"status\":\"ok\""), "{body}");
        let (_, body) = get_response(app, &format!("/rest/koanScrobbling?{MATE}")).await;
        assert!(!body.contains("lbuser"), "{body}");
    }

    #[tokio::test]
    async fn scrobbling_disconnects_lastfm_by_name_and_leaves_listenbrainz() {
        let (state, _dir) = test_state();
        let db = state.open_db().unwrap();
        let mate: i64 = db
            .conn
            .query_row("SELECT id FROM users WHERE username = 'mate'", [], |r| {
                r.get(0)
            })
            .unwrap();
        use koan_core::db::queries::scrobbling::{LASTFM, LISTENBRAINZ, connect};
        connect(&db.conn, mate, LISTENBRAINZ, "lb-token", "lbuser", 0).unwrap();
        connect(&db.conn, mate, LASTFM, "fm-session", "fmuser", 0).unwrap();
        drop(db);
        let app = build_test_router(state);

        let (_, body) = get_response(app.clone(), &format!("/rest/koanScrobbling?{MATE}")).await;
        assert!(
            !body.contains("fm-session") && !body.contains("lb-token"),
            "{body}"
        );
        assert!(body.contains("fmuser") && body.contains("lbuser"), "{body}");

        let (_, body) = get_response(
            app.clone(),
            &format!("/rest/koanScrobblingDisconnect?{MATE}&service=lastfm"),
        )
        .await;
        assert!(
            !body.contains("fmuser") && body.contains("lbuser"),
            "{body}"
        );
        let (_, body) = get_response(
            app,
            &format!("/rest/koanScrobblingDisconnect?{MATE}&service=nowhere"),
        )
        .await;
        assert!(body.contains("\"failed\""), "{body}");
    }

    #[tokio::test]
    async fn scrobbling_is_an_accounts_not_the_shared_secrets() {
        let (state, _dir) = test_state();
        let app = build_test_router(state);
        let secret = auth_query("");
        let (_, body) = get_response(app.clone(), &format!("/rest/koanScrobbling?{secret}")).await;
        assert!(body.contains("code=\"50\""), "{body}");
        let body = post_form(
            app.clone(),
            &format!("/rest/koanScrobblingConnect?{secret}"),
            "token=good-token",
        )
        .await;
        assert!(body.contains("code=\"50\""), "{body}");
        let (_, body) =
            get_response(app, &format!("/rest/koanScrobblingDisconnect?{secret}")).await;
        assert!(body.contains("code=\"50\""), "{body}");
    }

    #[tokio::test]
    async fn a_scrobbling_token_is_taken_only_from_a_form_body() {
        fake_listenbrainz();
        let (state, _dir) = test_state();
        let base = serve(state).await;
        let http = reqwest::Client::new();
        let connect = format!("{base}/rest/koanScrobblingConnect");
        let connected = || async {
            let status = http
                .get(format!("{base}/rest/koanScrobbling?{MATE}"))
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap();
            status.contains("lbuser")
        };

        let get = http
            .get(format!("{connect}?{MATE}&token=good-token"))
            .send()
            .await
            .unwrap();
        assert_eq!(get.status(), StatusCode::METHOD_NOT_ALLOWED);

        // A token in the query is not read, even on a form POST.
        let body = post_text(
            &http,
            &format!("{connect}?{MATE}&token=good-token"),
            "c=test",
        )
        .await;
        assert!(body.contains("'token' is missing"), "{body}");
        assert!(!body.contains("lbuser"), "{body}");
        assert!(!connected().await);

        let body = post_text(&http, &format!("{connect}?{MATE}"), "token=good-token").await;
        assert!(body.contains("lbuser"), "{body}");
        assert!(connected().await);
    }
}
