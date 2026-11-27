//! The response envelope and the node tree it is written from, as XML or
//! JSON.

use super::*;

mod node;
mod tag;

pub(super) use node::*;
pub(super) use tag::*;

pub(super) struct SubsonicResponse;

impl SubsonicResponse {
    pub(super) fn ok(json: bool) -> XmlBuilder {
        XmlBuilder {
            json,
            status: "ok",
            root: XmlNode::new(Tag::SubsonicResponse),
        }
    }

    pub(super) fn error(json: bool, err: &SubsonicError) -> Response {
        let error = XmlNode::new(Tag::Error)
            .attr_int("code", err.code as i64)
            .attr("message", &err.message)
            .attr_opt("helpUrl", err.help_url);
        let mut response = XmlBuilder {
            json,
            status: "failed",
            root: XmlNode::new(Tag::SubsonicResponse).child(error),
        }
        .build();
        if matches!(err.code, SubsonicErrorCode::WrongAuth) {
            response.extensions_mut().insert(AuthFailed);
        }
        response
    }
}

/// A Subsonic "not authorized" error carrying `message`, for refusals made
/// before a request reaches its endpoint.
pub(crate) fn not_authorized(json: bool, message: &str) -> Response {
    SubsonicResponse::error(
        json,
        &SubsonicError::new(SubsonicErrorCode::NotAuthorized, message),
    )
}

/// A response document: the `subsonic-response` envelope and what hangs off it.
pub(super) struct XmlBuilder {
    pub(super) json: bool,
    pub(super) status: &'static str,
    pub(super) root: XmlNode,
}

impl XmlBuilder {
    pub(super) fn child(mut self, node: XmlNode) -> Self {
        self.root = self.root.child(node);
        self
    }

    pub(super) fn list(mut self, tag: Tag, nodes: impl IntoIterator<Item = XmlNode>) -> Self {
        self.root = self.root.list(tag, nodes);
        self
    }

    /// Render with the envelope every response carries, success or failure:
    /// the Subsonic `status` and `version`, and OpenSubsonic's `type`,
    /// `serverVersion` and `openSubsonic`.
    pub(super) fn build(self) -> Response {
        let mut root = self.root;
        root.attrs.splice(
            0..0,
            [
                ("status".into(), AttrValue::Str(self.status.into())),
                (
                    "version".into(),
                    AttrValue::Str(SUBSONIC_API_VERSION.into()),
                ),
                ("type".into(), AttrValue::Str(SERVER_TYPE.into())),
                (
                    "serverVersion".into(),
                    AttrValue::Str(SERVER_VERSION.into()),
                ),
                ("openSubsonic".into(), AttrValue::Bool(true)),
            ],
        );
        if self.json {
            let wrapper = serde_json::json!({ "subsonic-response": root.to_json_value() });
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
                serde_json::to_string(&wrapper).unwrap(),
            )
                .into_response()
        } else {
            root.attrs
                .insert(0, ("xmlns".into(), AttrValue::Str(SUBSONIC_XMLNS.into())));
            let xml = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n{}",
                root.to_xml(0)
            );
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "application/xml; charset=utf-8")],
                xml,
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subsonic::testing::*;

    #[tokio::test]
    async fn test_json_response_format() {
        let (state, _dir) = test_state();
        seed_data(&state);

        let db = Database::open(state.pool.path()).unwrap();
        let albums = queries::all_albums(&db.conn).unwrap();

        let app = build_test_router(state);
        let (_, body) = get_response(
            app,
            &format!(
                "/rest/getAlbum?{}&id={}",
                auth_query("f=json"),
                albums[0].id
            ),
        )
        .await;

        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["subsonic-response"]["status"], "ok");
        assert!(parsed["subsonic-response"]["album"].is_object());
    }

    #[tokio::test]
    async fn test_envelope_on_success_and_error() {
        let (state, _dir) = test_state();
        let ok = json_of(
            build_test_router(state.clone()),
            &format!("/rest/ping?{}", auth_query("f=json")),
        )
        .await;
        assert_envelope(&ok, "ok");
        let failed = json_of(
            build_test_router(state.clone()),
            "/rest/ping?u=mate&p=wrong&f=json",
        )
        .await;
        assert_envelope(&failed, "failed");
        assert_eq!(failed["error"]["code"], 40);

        let expected = format!(
            "type=\"koan\" serverVersion=\"{}\" openSubsonic=\"true\"",
            env!("CARGO_PKG_VERSION")
        );
        for uri in [
            format!("/rest/ping?{}", auth_query("")),
            "/rest/ping?u=mate&p=wrong".into(),
            format!("/rest/nope?{}", auth_query("")),
        ] {
            let (_, body) = get_response(build_test_router(state.clone()), &uri).await;
            assert!(body.contains(&expected), "{uri}: {body}");
        }
    }
}
