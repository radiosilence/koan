//! The response envelope and the node tree it is written from, as XML or
//! JSON.

use super::*;

pub(super) struct SubsonicResponse;

impl SubsonicResponse {
    pub(super) fn ok(json: bool) -> XmlBuilder {
        XmlBuilder {
            json,
            status: "ok",
            root: XmlNode::new("subsonic-response"),
        }
    }

    pub(super) fn error(json: bool, err: &SubsonicError) -> Response {
        let error = XmlNode::new("error")
            .attr_int("code", err.code as i64)
            .attr("message", &err.message)
            .attr_opt("helpUrl", err.help_url);
        let mut response = XmlBuilder {
            json,
            status: "failed",
            root: XmlNode::new("subsonic-response").child(error),
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

/// An attribute value with its wire type preserved.
///
/// XML has only text, so every variant renders identically there. JSON is
/// typed, and the OpenSubsonic schema says `duration`/`track`/`bitRate`/`year`
/// are ints, `size` is a long and `isDir` is a boolean. Clients with generated
/// deserialisers abort a library sync on the first song when those arrive
/// quoted.
#[derive(Clone)]
pub(super) enum AttrValue {
    Str(String),
    Int(i64),
    Bool(bool),
}

impl AttrValue {
    pub(super) fn to_xml_text(&self) -> String {
        match self {
            AttrValue::Str(s) => s.clone(),
            AttrValue::Int(n) => n.to_string(),
            AttrValue::Bool(b) => b.to_string(),
        }
    }

    pub(super) fn to_json(&self) -> serde_json::Value {
        match self {
            AttrValue::Str(s) => serde_json::Value::String(s.clone()),
            AttrValue::Int(n) => serde_json::Value::Number((*n).into()),
            AttrValue::Bool(b) => serde_json::Value::Bool(*b),
        }
    }
}

#[derive(Clone)]
pub(super) struct XmlNode {
    pub(super) tag: String,
    pub(super) attrs: Vec<(String, AttrValue)>,
    /// Element text. The XSD carries a few values this way
    /// (`<genre songCount="6">Noise</genre>`); the JSON mapping spells the same
    /// thing as a `value` member.
    pub(super) text: Option<String>,
    /// A bare value, for arrays of primitives: `<versions>1</versions>` in XML,
    /// the element of `"versions": [1]` in JSON.
    pub(super) scalar: Option<AttrValue>,
    pub(super) children: Vec<XmlNode>,
    /// Child tags that are arrays in JSON whatever their count, empty
    /// included. OpenSubsonic requires a supported list field to be present
    /// as `[]` rather than absent, and a lone member would otherwise collapse
    /// into an object.
    pub(super) array_tags: Vec<String>,
}

impl XmlNode {
    pub(super) fn new(tag: &str) -> Self {
        Self {
            tag: tag.into(),
            attrs: Vec::new(),
            text: None,
            scalar: None,
            children: Vec::new(),
            array_tags: Vec::new(),
        }
    }

    pub(super) fn scalar(tag: &str, value: AttrValue) -> Self {
        Self {
            scalar: Some(value),
            ..Self::new(tag)
        }
    }

    pub(super) fn attr(mut self, key: &str, value: &str) -> Self {
        self.attrs.push((key.into(), AttrValue::Str(value.into())));
        self
    }

    pub(super) fn attr_int(mut self, key: &str, value: i64) -> Self {
        self.attrs.push((key.into(), AttrValue::Int(value)));
        self
    }

    pub(super) fn attr_bool(mut self, key: &str, value: bool) -> Self {
        self.attrs.push((key.into(), AttrValue::Bool(value)));
        self
    }

    pub(super) fn attr_opt(self, key: &str, value: Option<&str>) -> Self {
        match value {
            Some(v) => self.attr(key, v),
            None => self,
        }
    }

    pub(super) fn attr_opt_int(self, key: &str, value: Option<i64>) -> Self {
        match value {
            Some(v) => self.attr_int(key, v),
            None => self,
        }
    }

    pub(super) fn text(mut self, value: &str) -> Self {
        self.text = Some(value.into());
        self
    }

    pub(super) fn child(mut self, node: XmlNode) -> Self {
        self.children.push(node);
        self
    }

    pub(super) fn array_of(mut self, child_tag: &str) -> Self {
        self.array_tags.push(child_tag.into());
        self
    }

    /// Children under `tag`, always an array in JSON.
    pub(super) fn list(self, tag: &str, nodes: impl IntoIterator<Item = XmlNode>) -> Self {
        nodes
            .into_iter()
            .fold(self.array_of(tag), |node, child| node.child(child))
    }

    pub(super) fn to_xml(&self, indent: usize) -> String {
        let pad = "  ".repeat(indent);
        let mut s = format!("<{}", self.tag);
        for (k, v) in &self.attrs {
            s.push_str(&format!(" {}=\"{}\"", k, xml_escape(&v.to_xml_text())));
        }
        if self.children.is_empty() {
            let text = self
                .text
                .clone()
                .or_else(|| self.scalar.as_ref().map(AttrValue::to_xml_text));
            return match text {
                Some(t) => format!("{}{}>{}</{}>", pad, s, xml_escape(&t), self.tag),
                None => format!("{}{}/>", pad, s),
            };
        }
        s.push('>');
        let mut out = format!("{}{}\n", pad, s);
        for child in &self.children {
            out.push_str(&child.to_xml(indent + 1));
            out.push('\n');
        }
        out.push_str(&format!("{}</{}>", pad, self.tag));
        out
    }

    pub(super) fn to_json_value(&self) -> serde_json::Value {
        if let Some(value) = &self.scalar {
            return value.to_json();
        }
        let mut obj = serde_json::Map::new();
        for (k, v) in &self.attrs {
            obj.insert(k.clone(), v.to_json());
        }
        if let Some(text) = &self.text {
            obj.insert("value".into(), serde_json::Value::String(text.clone()));
        }
        let mut groups: BTreeMap<&str, Vec<serde_json::Value>> = self
            .array_tags
            .iter()
            .map(|tag| (tag.as_str(), Vec::new()))
            .collect();
        for child in &self.children {
            groups
                .entry(&child.tag)
                .or_default()
                .push(child.to_json_value());
        }
        for (tag, mut values) in groups {
            let value = if values.len() == 1 && !self.array_tags.iter().any(|t| t == tag) {
                values.pop().unwrap()
            } else {
                serde_json::Value::Array(values)
            };
            obj.insert(tag.into(), value);
        }
        serde_json::Value::Object(obj)
    }
}

impl XmlBuilder {
    pub(super) fn child(mut self, node: XmlNode) -> Self {
        self.root = self.root.child(node);
        self
    }

    pub(super) fn list(mut self, tag: &str, nodes: impl IntoIterator<Item = XmlNode>) -> Self {
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

pub(super) fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subsonic::testing::*;

    #[test]
    fn test_xml_escape() {
        assert_eq!(xml_escape("A&B"), "A&amp;B");
        assert_eq!(xml_escape("a<b>c"), "a&lt;b&gt;c");
        assert_eq!(xml_escape(r#"say "hi""#), "say &quot;hi&quot;");
    }

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
