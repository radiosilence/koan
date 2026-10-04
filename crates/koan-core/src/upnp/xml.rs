//! A small element tree over quick-xml, for the documents a renderer sends.
//!
//! Every UPnP document is a few kilobytes, and each is read once. A tree keyed
//! by local name reads them without caring which prefix a renderer bound each
//! namespace to, which is where renderers disagree most.

use quick_xml::events::Event;

#[derive(Debug, Default, Clone)]
pub struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Element>,
    pub text: String,
}

impl Element {
    /// The first child named `name`.
    pub fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.name == name)
    }

    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> {
        self.children.iter().filter(move |c| c.name == name)
    }

    /// The text of the first child named `name`, trimmed.
    pub fn child_text(&self, name: &str) -> Option<&str> {
        self.child(name).map(|c| c.text.trim())
    }

    /// Follow a path of child names.
    pub fn path(&self, names: &[&str]) -> Option<&Element> {
        names.iter().try_fold(self, |el, name| el.child(name))
    }

    /// The first element anywhere below this one named `name`, depth first.
    pub fn find(&self, name: &str) -> Option<&Element> {
        self.children.iter().find_map(|c| {
            if c.name == name {
                Some(c)
            } else {
                c.find(name)
            }
        })
    }

    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

/// Parse a document into its root element.
pub fn parse(doc: &str) -> Result<Element, String> {
    let mut reader = quick_xml::Reader::from_str(doc);
    let mut stack: Vec<Element> = vec![Element::default()];
    loop {
        match reader.read_event().map_err(|e| e.to_string())? {
            Event::Start(start) => stack.push(element(&start)?),
            Event::Empty(start) => {
                let el = element(&start)?;
                push_child(&mut stack, el);
            }
            Event::End(_) => {
                if stack.len() < 2 {
                    return Err("unbalanced end tag".into());
                }
                let el = stack.pop().expect("checked above");
                push_child(&mut stack, el);
            }
            Event::Text(text) => {
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&text.xml10_content());
                }
            }
            Event::CData(data) => {
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&data.xml10_content());
                }
            }
            Event::GeneralRef(entity) => {
                let resolved = match entity.resolve_char_ref() {
                    Ok(Some(c)) => c.to_string(),
                    _ => quick_xml::escape::resolve_predefined_entity(&entity)
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("&{};", &*entity)),
                };
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&resolved);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    let mut root = stack.into_iter().next().ok_or("empty document")?;
    root.children.pop().ok_or_else(|| "no root element".into())
}

fn push_child(stack: &mut [Element], el: Element) {
    if let Some(parent) = stack.last_mut() {
        parent.children.push(el);
    }
}

fn element(start: &quick_xml::events::BytesStart) -> Result<Element, String> {
    let name = start.local_name().as_ref().to_string();
    let mut attrs = Vec::new();
    for attr in start.attributes().flatten() {
        let key = attr.key.local_name().as_ref().to_string();
        let value = attr
            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
            .map_err(|e| e.to_string())?
            .into_owned();
        attrs.push((key, value));
    }
    Ok(Element {
        name,
        attrs,
        ..Default::default()
    })
}

/// Escape text for an element body or an attribute value.
pub fn escape(s: &str) -> String {
    quick_xml::escape::escape(s).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_names_ignore_prefixes_and_entities_resolve() {
        let el = parse(
            r#"<?xml version="1.0"?><s:Envelope xmlns:s="x"><s:Body><u:R xmlns:u="y"><A>a &amp; b &#233;</A><B v="1&lt;2"/></u:R></s:Body></s:Envelope>"#,
        )
        .unwrap();
        assert_eq!(el.name, "Envelope");
        let r = el.path(&["Body", "R"]).unwrap();
        assert_eq!(r.child_text("A"), Some("a & b é"));
        assert_eq!(r.child("B").unwrap().attr("v"), Some("1<2"));
        assert!(el.find("B").is_some());
    }
}
