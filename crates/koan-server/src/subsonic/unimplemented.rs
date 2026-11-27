//! The answer to an endpoint koan does not implement.

use super::*;

/// Anything under `/rest/` with no handler.
///
/// Registered as a wildcard route rather than a router fallback: this router is
/// merged into the GraphQL app, and a fallback there would swallow every 404 in
/// the process. A body-less 404 reads to a client as a broken server, and
/// several abort the connection test on one.
pub(super) async fn unsupported_endpoint(
    UrlPath(path): UrlPath<String>,
    RawQuery(raw): RawQuery,
) -> Response {
    let params = RawParams::parse(raw.as_deref());
    let endpoint = path.trim_end_matches(".view");
    SubsonicResponse::error(
        params.auth().wants_json(),
        &SubsonicError::unsupported(endpoint),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subsonic::testing::*;

    #[tokio::test]
    async fn test_unknown_endpoint_is_subsonic_error_70() {
        let (state, _dir) = test_state();
        let app = build_test_router(state.clone());
        let (status, body) =
            get_response(app, &format!("/rest/getPodcasts?{}", auth_query(""))).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("status=\"failed\""));
        assert!(body.contains("code=\"70\""));
        assert!(body.contains("getPodcasts"));

        let app = build_test_router(state);
        let (_, body) = get_response(
            app,
            &format!(
                "/rest/getInternetRadioStations.view?{}",
                auth_query("f=json")
            ),
        )
        .await;
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["subsonic-response"]["error"]["code"], 70);
    }
}
