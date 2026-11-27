//! The node tree a response is written from: elements named by [`Tag`],
//! attributes with their JSON types, rendered as XML or as JSON.

use super::*;

/// An attribute value with its wire type preserved.
///
/// XML has only text, so every variant renders identically there. JSON is
/// typed, and the OpenSubsonic schema says `duration`/`track`/`bitRate`/`year`
/// are ints, `size` is a long and `isDir` is a boolean. Clients with generated
/// deserialisers abort a library sync on the first song when those arrive
/// quoted.
#[derive(Clone)]
pub(in crate::subsonic) enum AttrValue {
    Str(String),
    Int(i64),
    Bool(bool),
}

impl AttrValue {
    pub(in crate::subsonic) fn to_xml_text(&self) -> String {
        match self {
            AttrValue::Str(s) => s.clone(),
            AttrValue::Int(n) => n.to_string(),
            AttrValue::Bool(b) => b.to_string(),
        }
    }

    pub(in crate::subsonic) fn to_json(&self) -> serde_json::Value {
        match self {
            AttrValue::Str(s) => serde_json::Value::String(s.clone()),
            AttrValue::Int(n) => serde_json::Value::Number((*n).into()),
            AttrValue::Bool(b) => serde_json::Value::Bool(*b),
        }
    }
}

#[derive(Clone)]
pub(in crate::subsonic) struct XmlNode {
    pub(in crate::subsonic) tag: Tag,
    pub(in crate::subsonic) attrs: Vec<(String, AttrValue)>,
    /// Element text. The XSD carries a few values this way
    /// (`<genre songCount="6">Noise</genre>`); the JSON mapping spells the same
    /// thing as a `value` member.
    pub(in crate::subsonic) text: Option<String>,
    /// A bare value, for arrays of primitives: `<versions>1</versions>` in XML,
    /// the element of `"versions": [1]` in JSON.
    pub(in crate::subsonic) scalar: Option<AttrValue>,
    pub(in crate::subsonic) children: Vec<XmlNode>,
    /// Child tags that are arrays in JSON whatever their count, empty
    /// included. OpenSubsonic requires a supported list field to be present
    /// as `[]` rather than absent, and a lone member would otherwise collapse
    /// into an object.
    pub(in crate::subsonic) array_tags: Vec<Tag>,
}

impl XmlNode {
    pub(in crate::subsonic) fn new(tag: Tag) -> Self {
        Self {
            tag,
            attrs: Vec::new(),
            text: None,
            scalar: None,
            children: Vec::new(),
            array_tags: Vec::new(),
        }
    }

    pub(in crate::subsonic) fn scalar(tag: Tag, value: AttrValue) -> Self {
        Self {
            scalar: Some(value),
            ..Self::new(tag)
        }
    }

    pub(in crate::subsonic) fn attr(mut self, key: &str, value: &str) -> Self {
        self.attrs.push((key.into(), AttrValue::Str(value.into())));
        self
    }

    pub(in crate::subsonic) fn attr_int(mut self, key: &str, value: i64) -> Self {
        self.attrs.push((key.into(), AttrValue::Int(value)));
        self
    }

    pub(in crate::subsonic) fn attr_bool(mut self, key: &str, value: bool) -> Self {
        self.attrs.push((key.into(), AttrValue::Bool(value)));
        self
    }

    pub(in crate::subsonic) fn attr_opt(self, key: &str, value: Option<&str>) -> Self {
        match value {
            Some(v) => self.attr(key, v),
            None => self,
        }
    }

    pub(in crate::subsonic) fn attr_opt_int(self, key: &str, value: Option<i64>) -> Self {
        match value {
            Some(v) => self.attr_int(key, v),
            None => self,
        }
    }

    pub(in crate::subsonic) fn text(mut self, value: &str) -> Self {
        self.text = Some(value.into());
        self
    }

    pub(in crate::subsonic) fn child(mut self, node: XmlNode) -> Self {
        self.children.push(node);
        self
    }

    pub(in crate::subsonic) fn array_of(mut self, child_tag: Tag) -> Self {
        self.array_tags.push(child_tag);
        self
    }

    /// Children under `tag`, always an array in JSON.
    pub(in crate::subsonic) fn list(
        self,
        tag: Tag,
        nodes: impl IntoIterator<Item = XmlNode>,
    ) -> Self {
        nodes
            .into_iter()
            .fold(self.array_of(tag), |node, child| node.child(child))
    }

    pub(in crate::subsonic) fn to_xml(&self, indent: usize) -> String {
        let pad = "  ".repeat(indent);
        let mut s = format!("<{}", self.tag.as_str());
        for (k, v) in &self.attrs {
            s.push_str(&format!(" {}=\"{}\"", k, xml_escape(&v.to_xml_text())));
        }
        if self.children.is_empty() {
            let text = self
                .text
                .clone()
                .or_else(|| self.scalar.as_ref().map(AttrValue::to_xml_text));
            return match text {
                Some(t) => format!("{}{}>{}</{}>", pad, s, xml_escape(&t), self.tag.as_str()),
                None => format!("{}{}/>", pad, s),
            };
        }
        s.push('>');
        let mut out = format!("{}{}\n", pad, s);
        for child in &self.children {
            out.push_str(&child.to_xml(indent + 1));
            out.push('\n');
        }
        out.push_str(&format!("{}</{}>", pad, self.tag.as_str()));
        out
    }

    pub(in crate::subsonic) fn to_json_value(&self) -> serde_json::Value {
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
                .entry(child.tag.as_str())
                .or_default()
                .push(child.to_json_value());
        }
        for (tag, mut values) in groups {
            let value = if values.len() == 1 && !self.array_tags.iter().any(|t| t.as_str() == tag) {
                values.pop().unwrap()
            } else {
                serde_json::Value::Array(values)
            };
            obj.insert(tag.into(), value);
        }
        serde_json::Value::Object(obj)
    }
}

pub(in crate::subsonic) fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_xml_escape() {
        assert_eq!(xml_escape("A&B"), "A&amp;B");
        assert_eq!(xml_escape("a<b>c"), "a&lt;b&gt;c");
        assert_eq!(xml_escape(r#"say "hi""#), "say &quot;hi&quot;");
    }
}
