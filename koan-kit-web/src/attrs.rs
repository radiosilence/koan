//! The props every component takes, whatever it draws: an id, layout classes, data and other attributes, and a
//! Datastar attribute the kit does not write itself.
//!
//! Names are `&'static str`, so only the program's own text names an attribute, and each is checked as it is given:
//! a `data-*` key that would make a Datastar expression of its value is refused, since an expression is code and the
//! value may have come from anyone. Values are escaped as they are written.

use std::fmt;

use askama::filters::{Escaper, HtmlSafe};

use crate::arg::Text;

/// Datastar's attribute plugins in the release `askama_datastar` vendors, and the attributes its core reads. A `data-*`
/// key naming one would hand its value to Datastar as an expression or a signal.
const DATASTAR: &[&str] = &[
    "attr",
    "bind",
    "class",
    "computed",
    "effect",
    "ignore",
    "ignore-morph",
    "indicator",
    "init",
    "json-signals",
    "on",
    "on-intersect",
    "on-interval",
    "on-signal-patch",
    "preserve-attr",
    "ref",
    "show",
    "signals",
    "style",
    "text",
];

/// An attribute: `data-{key}`, or a name of its own.
#[derive(Clone, Debug)]
enum Name {
    Data(&'static str),
    Plain(&'static str),
}

/// The common props of a component, written onto its outermost element.
#[derive(Clone, Debug, Default)]
pub(crate) struct Attrs<'a> {
    id: Text<'a>,
    classes: Vec<&'static str>,
    attributes: Vec<(Name, Text<'a>)>,
    datastar: Vec<(&'static str, &'static str)>,
}

impl<'a> Attrs<'a> {
    /// The id, or empty.
    pub(crate) fn id(&self) -> &str {
        self.id.as_str()
    }

    pub(crate) fn set_id(&mut self, id: Text<'a>) {
        self.id = id;
    }

    pub(crate) fn add_class(&mut self, class: &'static str) {
        assert!(
            !class.contains(['"', '<', '>', '&']),
            "a class is a list of class names; {class:?} holds markup"
        );
        self.classes.push(class);
    }

    pub(crate) fn add_data(&mut self, key: &'static str, value: Text<'a>) {
        assert!(
            !key.is_empty()
                && key
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
            "data-{key}: a data key is lowercase letters, digits and hyphens"
        );
        let plugin = DATASTAR
            .iter()
            .any(|name| key == *name || key.starts_with(&format!("{name}-")));
        assert!(
            !plugin,
            "data-{key} is a Datastar attribute, whose value is an expression: give it with .datastar(), whose \
             expression is the program's own, and pass a value it reads in a data attribute of another name"
        );
        self.attributes.push((Name::Data(key), value));
    }

    pub(crate) fn add_attr(&mut self, name: &'static str, value: Text<'a>) {
        assert!(
            !name.is_empty()
                && name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                && name.as_bytes()[0].is_ascii_lowercase(),
            "{name:?} is not an attribute name"
        );
        assert!(
            !name.starts_with("on"),
            "{name}: an event handler attribute runs script, which the CSP refuses; use .datastar(\"on:…\", …)"
        );
        assert!(
            !name.starts_with("data-"),
            "{name}: give a data attribute with .data() or .datastar()"
        );
        assert!(
            !matches!(name, "id" | "class" | "style"),
            "{name}: give an id with .id() and classes with .class(); a style attribute is refused by the CSP"
        );
        self.attributes.push((Name::Plain(name), value));
    }

    pub(crate) fn add_datastar(&mut self, name: &'static str, expression: &'static str) {
        assert!(
            !name.is_empty()
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b':' | b'_' | b'.')),
            "data-{name} is not a Datastar attribute name"
        );
        self.datastar.push((name, expression));
    }

    /// The classes, each after a space, to follow the component's own in its `class` attribute.
    pub(crate) fn classes(&self) -> Classes<'_> {
        Classes(&self.classes)
    }
}

/// The id and every attribute but `class`, each after a space.
impl fmt::Display for Attrs<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(id) = self.id.present() {
            attribute(f, "id", id)?;
        }
        for (name, value) in &self.attributes {
            match name {
                Name::Data(key) => {
                    f.write_str(" data-")?;
                    f.write_str(key)?;
                    f.write_str("=\"")?;
                }
                Name::Plain(name) => {
                    f.write_str(" ")?;
                    f.write_str(name)?;
                    f.write_str("=\"")?;
                }
            }
            escape(f, value.as_str())?;
            f.write_str("\"")?;
        }
        for (name, expression) in &self.datastar {
            f.write_str(" data-")?;
            f.write_str(name)?;
            f.write_str("=\"")?;
            escape(f, expression)?;
            f.write_str("\"")?;
        }
        Ok(())
    }
}

impl HtmlSafe for Attrs<'_> {}

/// Classes an app added for layout, each after a space.
pub(crate) struct Classes<'v>(&'v [&'static str]);

impl Classes<'_> {
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Display for Classes<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for class in self.0 {
            f.write_str(" ")?;
            f.write_str(class)?;
        }
        Ok(())
    }
}

impl HtmlSafe for Classes<'_> {}

/// ` name="value"`, the value escaped.
pub(crate) fn attribute(f: &mut fmt::Formatter<'_>, name: &str, value: &str) -> fmt::Result {
    f.write_str(" ")?;
    f.write_str(name)?;
    f.write_str("=\"")?;
    escape(f, value)?;
    f.write_str("\"")
}

/// Text as HTML writes it: `&`, `<`, `>`, `"` and `'` escaped, as askama escapes a template's values.
pub(crate) fn escape(f: &mut fmt::Formatter<'_>, text: &str) -> fmt::Result {
    askama::filters::Html.write_escaped_str(f, text)
}

/// The common props, rendering and conversions of a component, for the type named.
///
/// Every component draws through `Display` (askama's, or its own), keeps its common props in a field named `attrs`,
/// and has one lifetime, that of the text it borrows.
macro_rules! component {
    ($component:ident) => {
        impl<'a> $component<'a> {
            /// The outermost element's id: what a patch finds the component by, and what a label or a
            /// `commandfor` names.
            pub fn id(mut self, id: impl Into<$crate::arg::Text<'a>>) -> Self {
                self.attrs.set_id(id.into());
                self
            }

            /// Classes from the app's own stylesheet for where the component sits: a column span, a width, a
            /// margin. They follow the kit's on its outermost element. The class string sits in the app's
            /// template or code, where its Tailwind finds it and kit-check reads it.
            pub fn class(mut self, class: &'static str) -> Self {
                self.attrs.add_class(class);
                self
            }

            /// A `data-{key}` attribute, its value escaped: something for a Datastar expression to read with
            /// `el.dataset`, or for the page's own script. A key naming a Datastar attribute is refused, since its
            /// value would run as an expression; that goes through [`datastar`](Self::datastar).
            ///
            /// # Panics
            ///
            /// When `key` is not lowercase letters, digits and hyphens, or names a Datastar attribute.
            pub fn data(
                mut self,
                key: &'static str,
                value: impl Into<$crate::arg::Text<'a>>,
            ) -> Self {
                self.attrs.add_data(key, value.into());
                self
            }

            /// Another attribute, its value escaped: `title`, `lang`, `aria-*`, `hidden`. An id, a class, a
            /// style, a `data-*` attribute and an `on*` handler each have their own way in, or none.
            ///
            /// # Panics
            ///
            /// When `name` is not an attribute name, or is one of those.
            pub fn attr(
                mut self,
                name: &'static str,
                value: impl Into<$crate::arg::Text<'a>>,
            ) -> Self {
                self.attrs.add_attr(name, value.into());
                self
            }

            /// A Datastar attribute the kit does not write itself: `data-{name}="{expression}"`, such as
            /// `.datastar("on-intersect", "$page++")`. Both are `&'static str`, so by convention only the
            /// program's own text reaches an expression; a value it needs goes in a [`data`](Self::data)
            /// attribute, read with `el.dataset`.
            ///
            /// # Panics
            ///
            /// When `name` holds anything an attribute name cannot.
            pub fn datastar(mut self, name: &'static str, expression: &'static str) -> Self {
                self.attrs.add_datastar(name, expression);
                self
            }

            /// Kept as it is when a patch morphs the region around it: something the page's own script or the
            /// person changed, such as an open disclosure in a streamed list.
            pub fn ignore_morph(mut self) -> Self {
                self.attrs.add_datastar("ignore-morph", "");
                self
            }

            /// The component as a patch that replaces its element whole rather than morphing it: for an answer whose
            /// target carries `data-ignore-morph`, or an ancestor does, which a morph would leave alone.
            pub fn patch_replace(&self) -> askama_datastar::PatchElements {
                self.html()
                    .patch()
                    .mode(askama_datastar::ElementPatchMode::Replace)
            }

            /// The component's markup.
            pub fn html(&self) -> askama_datastar::Html {
                askama_datastar::Html::trusted(self.to_string())
            }

            /// The component as a Datastar patch, which morphs it into the page by its id.
            pub fn patch(&self) -> askama_datastar::PatchElements {
                self.html().patch()
            }
        }

        impl askama::filters::HtmlSafe for $component<'_> {}

        impl From<$component<'_>> for askama_datastar::Html {
            fn from(component: $component<'_>) -> Self {
                component.html()
            }
        }

        impl From<$component<'_>> for $crate::arg::Markup {
            fn from(component: $component<'_>) -> Self {
                $crate::arg::Markup::kit(component.to_string())
            }
        }

        impl From<&$component<'_>> for $crate::arg::Markup {
            fn from(component: &$component<'_>) -> Self {
                $crate::arg::Markup::kit(component.to_string())
            }
        }

        impl From<&&$component<'_>> for $crate::arg::Markup {
            fn from(component: &&$component<'_>) -> Self {
                $crate::arg::Markup::kit(component.to_string())
            }
        }

        impl<'a> From<$component<'_>> for $crate::arg::Content<'a> {
            fn from(component: $component<'_>) -> Self {
                Self::Markup(component.into())
            }
        }

        impl<'a> From<&$component<'_>> for $crate::arg::Content<'a> {
            fn from(component: &$component<'_>) -> Self {
                Self::Markup(component.into())
            }
        }
    };
}

pub(crate) use component;

#[cfg(test)]
mod tests {
    use super::*;

    fn written(attrs: &Attrs<'_>) -> String {
        attrs.to_string()
    }

    #[test]
    fn values_are_escaped_and_names_are_the_programs() {
        let mut attrs = Attrs::default();
        attrs.set_id("row-1".into());
        attrs.add_data("url", "/x?a=1&b=\"2\"".into());
        attrs.add_attr("title", "<b>".into());
        attrs.add_datastar("on:click", "@post(el.dataset.url)");
        assert_eq!(
            written(&attrs),
            " id=\"row-1\" data-url=\"/x?a=1&#38;b=&#34;2&#34;\" title=\"&#60;b&#62;\" \
             data-on:click=\"@post(el.dataset.url)\""
        );
    }

    #[test]
    #[should_panic(expected = "is a Datastar attribute")]
    fn a_data_key_naming_a_datastar_attribute_is_refused() {
        Attrs::default().add_data("on-click", "alert(1)".into());
    }

    #[test]
    #[should_panic(expected = "is a Datastar attribute")]
    fn a_data_key_naming_a_signal_binding_is_refused() {
        Attrs::default().add_data("bind", "name".into());
    }

    #[test]
    #[should_panic(expected = "event handler")]
    fn an_event_handler_attribute_is_refused() {
        Attrs::default().add_attr("onclick", "alert(1)".into());
    }

    #[test]
    #[should_panic(expected = "style")]
    fn a_style_attribute_is_refused() {
        Attrs::default().add_attr("style", "color: red".into());
    }
}
