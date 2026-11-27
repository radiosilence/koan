//! Pairing: approving a device that is waiting to sign in.

use super::*;

/// A pairing as an approver sees it: the device's name, the address it asked
/// from, and whether that address is on a private network.
pub(in crate::subsonic) fn pair_node(info: &crate::pair::PairInfo) -> XmlNode {
    let node = XmlNode::new(Tag::Pair)
        .attr("device", &info.device)
        .attr("from", &info.from.to_string())
        .attr_bool("local", info.local());
    match &info.device_id {
        Some(id) => node.attr("deviceId", id),
        None => node,
    }
}

/// Who may look a pairing up, approve it or decline it: a caller who could
/// sign it in. Approving mints an API key for the device, which an app
/// password, one client's credential, may not hand out, and the shared secret
/// is no account's. Any role: a device is signed in to the caller's own
/// account, with no more than the caller can do.
pub(in crate::subsonic) fn may_pair(caller: &Caller) -> Result<(), SubsonicError> {
    caller.may_manage_credentials()?;
    if caller.user_id == queries::LOCAL_USER {
        return Err(SubsonicError::new(
            SubsonicErrorCode::Generic,
            "sign in with an account to sign a device in as it",
        ));
    }
    Ok(())
}

pub(in crate::subsonic) fn pair_error(e: crate::pair::PairError) -> SubsonicError {
    match e {
        crate::pair::PairError::NotFound => SubsonicError::not_found("Pairing"),
        crate::pair::PairError::Limited => SubsonicError::new(
            SubsonicErrorCode::Generic,
            "Too many pairing codes tried that were not waiting; try again in a minute",
        ),
        crate::pair::PairError::Internal(e) => SubsonicError::internal(e),
    }
}

/// The device waiting on `pair`, an id or a code, and where it asked from: for
/// an app to ask whether to sign it in.
pub(in crate::subsonic) async fn koan_pair_info(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        respond(&state, &params.auth(), |caller, b| {
            let pair = params
                .get("pair")
                .ok_or_else(|| SubsonicError::missing_param("pair"))?;
            may_pair(caller)?;
            let info = crate::pair::pairings()
                .info(pair, caller.user_id)
                .map_err(pair_error)?;
            Ok(b.child(pair_node(&info)))
        })
    })
    .await
}

/// Sign the device waiting on `pair` in as the caller, with an API key of its
/// own, or with `decline=true` turn it away.
pub(in crate::subsonic) async fn koan_pair_approve(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        respond_db_caller(&state, &params.auth(), Role::Readonly, |db, caller, b| {
            let pair = params
                .get("pair")
                .ok_or_else(|| SubsonicError::missing_param("pair"))?;
            let decline = params.get("decline") == Some("true");
            may_pair(caller)?;
            let info = crate::pair::pairings()
                .settle(&db.conn, pair, caller.user_id, &caller.username, decline)
                .map_err(pair_error)?;
            Ok(b.child(pair_node(&info)))
        })
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subsonic::testing::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn pairing_is_refused_to_web_pages() {
        let (state, _dir) = test_state();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, build_test_router(state)).await });
        let url = format!("ws://{addr}/rest/koanPair?name=tv");
        tokio::task::spawn_blocking(move || {
            use tungstenite::client::IntoClientRequest as _;
            let mut from_page = url.as_str().into_client_request().unwrap();
            from_page
                .headers_mut()
                .insert(header::ORIGIN, "https://evil.example".parse().unwrap());
            match tungstenite::connect(from_page) {
                Err(tungstenite::Error::Http(r)) => assert_eq!(r.status(), StatusCode::FORBIDDEN),
                other => panic!("a page was let in: {:?}", other.map(|_| ())),
            }
            // As the apps connect: tungstenite sends no Origin.
            let (mut socket, _) = tungstenite::connect(url.as_str()).unwrap();
            let first = socket.read().unwrap();
            let message: koan_core::remote::pair::PairMessage =
                serde_json::from_str(first.to_text().unwrap()).unwrap();
            assert!(
                matches!(
                    message,
                    koan_core::remote::pair::PairMessage::Pending { .. }
                ),
                "{message:?}"
            );
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn a_waiting_device_is_paired_with_the_approvers_account() {
        let (state, _dir) = test_state();
        let call = |path: String| {
            let state = state.clone();
            async move { get_response(build_test_router(state), &path).await.1 }
        };
        let mate = "u=mate&p=hunter22&v=1.16.1&c=test&f=json";
        let opened = crate::pair::pairings()
            .open("Den TV", Some("0192-den"), "198.51.100.7".parse().unwrap())
            .unwrap();
        let (id, code) = (
            opened.id.clone(),
            opened.code.replace('-', "").to_lowercase(),
        );
        let device = crate::pair::device(opened, true);

        let body = call(format!("/rest/koanPairInfo?pair={code}")).await;
        assert!(body.contains("code=\"10\""), "{body}");
        let body = call(format!("/rest/koanPairInfo?pair={code}&{mate}")).await;
        // Compared as values: the order of a JSON object's keys is serde's to
        // choose, and differs with the features a build unifies.
        let pair: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            pair["subsonic-response"]["pair"],
            serde_json::json!({
                "device": "Den TV",
                "deviceId": "0192-den",
                "from": "198.51.100.7",
                "local": false,
            }),
            "{body}"
        );

        let body = call(format!("/rest/koanPairApprove?pair={id}&{mate}")).await;
        assert!(body.contains("\"device\":\"Den TV\""), "{body}");
        assert!(matches!(
            device.join().unwrap(),
            koan_core::remote::pair::PairMessage::Approved { .. }
        ));
        let body = call(format!("/rest/koanPairApprove?pair={id}&{mate}")).await;
        assert!(body.contains("\"code\":70"), "{body}");
        let body = call(format!("/rest/koanPairInfo?pair=ABCD-EFGH&{mate}")).await;
        assert!(body.contains("\"code\":70"), "{body}");
    }
}
