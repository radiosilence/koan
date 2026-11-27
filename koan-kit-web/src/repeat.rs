//! A field of any number of one value: [`RepeatField`].

use askama::Template;

use crate::arg::Text;
use crate::attrs::{Attrs, component};
use crate::button::Button;
use crate::field::Input;
use crate::icon;

/// Rows of one input, each sent under the input's name, which the kit adds, removes and fills from a paste:
/// `kit::RepeatField::new("magnet links", kit::Input::url("magnet"), "add another", "remove")`.
///
/// Each row is the input given, holding one of the [`values`](Self::values) or empty, named by the label for a screen
/// reader, with a button to remove it while there are others. The add button puts an empty row after the last and
/// focuses it. A paste into a row holding several values (one per line, or per word with
/// [`split_words`](Self::split_words)) fills that row and as many after it as it takes, rather than running them
/// together in one field; with [`paste`](Self::paste) each row has a button that reads the clipboard into it, which
/// on iOS puts Safari's Paste bubble against the field it fills. Give the input no id, since every row repeats it.
#[must_use = "a field draws nothing until it is rendered"]
#[derive(Clone, Debug, Template)]
#[template(path = "repeat.html")]
pub struct RepeatField<'a> {
    label: Text<'a>,
    input: Input<'a>,
    values: Vec<Text<'a>>,
    add: Text<'a>,
    remove: Text<'a>,
    paste: Text<'a>,
    words: bool,
    hint: Text<'a>,
    attrs: Attrs<'a>,
}

impl<'a> RepeatField<'a> {
    /// Rows of `input`, labelled `label`, with an add button saying `add` and remove buttons named `remove`.
    pub fn new(
        label: impl Into<Text<'a>>,
        input: Input<'a>,
        add: impl Into<Text<'a>>,
        remove: impl Into<Text<'a>>,
    ) -> Self {
        Self {
            label: label.into(),
            input,
            values: Vec::new(),
            add: add.into(),
            remove: remove.into(),
            paste: Text::default(),
            words: false,
            hint: Text::default(),
            attrs: Attrs::default(),
        }
    }

    /// A row for each of `values`, as the page opens: what was sent and refused. With none, one empty row.
    pub fn values<T: Into<Text<'a>>>(mut self, values: impl IntoIterator<Item = T>) -> Self {
        self.values = values.into_iter().map(Into::into).collect();
        self
    }

    /// A button on each row, named `label`, that puts the clipboard into it.
    pub fn paste(mut self, label: impl Into<Text<'a>>) -> Self {
        self.paste = label.into();
        self
    }

    /// A paste splits on any space rather than on line breaks, for values that hold none, such as links.
    pub fn split_words(mut self) -> Self {
        self.words = true;
        self
    }

    /// A note under the rows, which describes them.
    pub fn hint(mut self, hint: impl Into<Text<'a>>) -> Self {
        self.hint = hint.into();
        self
    }

    /// The ids its label and note are built on: its own id, or else the input's name.
    fn base(&self) -> &str {
        if self.attrs.id().is_empty() {
            self.input.field_name()
        } else {
            self.attrs.id()
        }
    }

    /// Each row's input.
    fn rows(&self) -> Vec<Input<'a>> {
        let row = self.input.clone().label(self.label.clone());
        if self.values.is_empty() {
            vec![row]
        } else {
            self.values
                .iter()
                .map(|value| row.clone().value(value.clone()))
                .collect()
        }
    }

    fn paste_button(&self) -> Option<Button<'a>> {
        self.paste.present().map(|_| {
            Button::bordered(self.paste.clone())
                .icon_only(icon::PASTE)
                .data("repeat", "paste")
        })
    }

    fn remove_button(&self) -> Button<'a> {
        Button::quiet(self.remove.clone())
            .icon_only(icon::CLOSE)
            .data("repeat", "remove")
    }

    fn add_button(&self) -> Button<'a> {
        Button::quiet(self.add.clone())
            .icon(icon::PLUS)
            .data("repeat", "add")
    }
}

component!(RepeatField);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_row_is_the_input_named_by_the_label() {
        let html = RepeatField::new(
            "magnet links",
            Input::url("magnet"),
            "add another",
            "remove",
        )
        .values(["magnet:?a", "magnet:?b"])
        .paste("paste")
        .split_words()
        .hint("One per row.")
        .to_string();
        assert_eq!(html.matches("data-repeat-row>").count(), 2, "{html}");
        assert!(
            html.contains("value=\"magnet:?a\" name=\"magnet\""),
            "{html}"
        );
        assert_eq!(
            html.matches("aria-label=\"magnet links\"").count(),
            2,
            "{html}"
        );
        assert!(
            html.contains(
                "role=\"group\" aria-labelledby=\"magnet-label\" aria-describedby=\"magnet-note\""
            ),
            "{html}"
        );
        assert!(html.contains("data-split=\"words\""), "{html}");
        assert!(html.contains("data-repeat=\"paste\""), "{html}");
        assert!(
            !html.contains("magnet:?a\" data-on"),
            "a value reaches no expression: {html}"
        );
    }

    #[test]
    fn with_no_values_there_is_one_empty_row() {
        let html = RepeatField::new("tags", Input::text("tag"), "add", "remove").to_string();
        assert_eq!(html.matches("data-repeat-row>").count(), 1, "{html}");
        assert!(!html.contains("data-repeat=\"paste\""), "{html}");
        assert!(html.contains("data-split=\"lines\""), "{html}");
    }
}
