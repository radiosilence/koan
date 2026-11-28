//! A list of rows to tick and act on together: a [`Selection`], its [`SelectionCount`], and the [`SelectionBar`] of
//! actions that sticks to the foot of the window while anything is ticked.

use askama::Template;

use crate::arg::{Amount, Markup, Text};
use crate::attrs::{Attrs, component};
use crate::button::Button;
use crate::choice::Check;
use crate::slot::Wrap;

/// What a selection does as it starts: keep its counts, its bar and its validity in step with its boxes, now, on every
/// change, and whenever rows arrive or leave, as a stream appends them.
///
/// The boxes are the `<input>`s sent under the selection's `data-name`, told apart by comparing names rather than by a
/// selector, so no name has to be escaped into one. Ticking "all" stands for `data-total` rows, when more match than
/// are drawn. A count writes `data-one` or `data-many`, `{}` replaced by the count, as text. While none is ticked, the
/// bars are hidden and the first box says `data-none`, which a request sending the form reports instead of sending.
const COUNT: &str = concat!(
    "(el._kitsel = () => (b => (n => (",
    "el.querySelectorAll('[data-kit-count]').forEach(c => (w => c.textContent != w && (c.textContent = w))",
    "((n == 1 ? c.dataset.one : c.dataset.many).replace('{}', n))),",
    " el.querySelectorAll('[data-kit-selected]').forEach(s => s.hidden = !n),",
    " b[0]?.setCustomValidity(n || !el.dataset.none ? '' : el.dataset.none)))",
    "(el.querySelector('input[name=all]')?.checked ? +(el.dataset.total || b.length) : b.filter(c => c.checked).length))",
    "([...el.querySelectorAll('input')].filter(c => c.name == el.dataset.name)),",
    " new MutationObserver(() => el._kitsel()).observe(el, {childList: true, subtree: true}), el._kitsel())",
);

/// What a selection does when a box changes: "all" ticks or unticks every row, a row unticked unticks "all", and the
/// counts follow.
const CHANGE: &str = concat!(
    "(t => t.name == 'all' ? el.querySelectorAll('input').forEach(c => c.name == el.dataset.name && (c.checked = t.checked))",
    " : t.name == el.dataset.name && !t.checked && (a => a && (a.checked = false))(el.querySelector('input[name=all]')),",
    " el._kitsel())(evt.target)",
);

/// Rows to tick and act on together: `kit::Selection::new("albums", "path")` round a list whose rows each hold a box
/// sent as `path`.
///
/// It is a form, so an action that sends it ([`Button::sends`]) sends the rows ticked. Its rows are the markup of a
/// `{% filter kit::wrap(selection) %}` block: the boxes, a [`select_all`](Self::select_all) box if it has one, and any
/// [`SelectionCount`] and [`SelectionBar`], which it keeps up to date as boxes change and as rows arrive. With
/// [`none`](Self::none), a request sending it with nothing ticked says so rather than sending.
#[must_use = "a selection draws nothing until it is rendered"]
#[derive(Clone, Debug, Template)]
#[template(path = "selection.html")]
pub struct Selection<'a> {
    name: Text<'a>,
    total: Option<Amount>,
    none: Text<'a>,
    rows: Markup,
    attrs: Attrs<'a>,
}

impl<'a> Selection<'a> {
    /// The form `id`, whose rows' boxes are sent under `name`.
    pub fn new(id: impl Into<Text<'a>>, name: impl Into<Text<'a>>) -> Self {
        let mut attrs = Attrs::default();
        attrs.set_id(id.into());
        Self {
            name: name.into(),
            total: None,
            none: Text::default(),
            rows: Markup::default(),
            attrs,
        }
    }

    /// How many rows "all" stands for, when more match than are drawn: the count says this many while "all" is
    /// ticked, and the form sends `all=1` for the handler to act on every match.
    pub fn total(mut self, total: impl Into<Amount>) -> Self {
        self.total = Some(total.into());
        self
    }

    /// What a request sending it with nothing ticked says, in place of sending: "Tick an album to apply this to."
    pub fn none(mut self, message: impl Into<Text<'a>>) -> Self {
        self.none = message.into();
        self
    }

    /// The box that ticks every row, saying `label`: sent as `all=1`, and unticked when a row is.
    pub fn select_all(label: impl Into<Text<'a>>) -> Check<'a> {
        Check::new("all", label).value("1")
    }
}

impl Wrap for Selection<'_> {
    fn wrap(mut self, content: Markup) -> Self {
        self.rows = content;
        self
    }
}

component!(Selection);

/// How many rows of its [`Selection`] are ticked, in words: `kit::SelectionCount::new("{} album", "{} albums")`, `{}`
/// standing for the count. Empty until the selection starts.
#[must_use = "a count draws nothing until it is rendered"]
#[derive(Clone, Debug, Template)]
#[template(path = "selection_count.html")]
pub struct SelectionCount<'a> {
    one: Text<'a>,
    many: Text<'a>,
    attrs: Attrs<'a>,
}

impl<'a> SelectionCount<'a> {
    /// The words for one row ticked and for any other number, `{}` standing for the count.
    pub fn new(one: impl Into<Text<'a>>, many: impl Into<Text<'a>>) -> Self {
        Self {
            one: one.into(),
            many: many.into(),
            attrs: Attrs::default(),
        }
    }
}

component!(SelectionCount);

/// The actions on what its [`Selection`] has ticked, in a bar stuck to the foot of the window and clear of a phone's
/// home indicator, shown only while anything is ticked: `kit::SelectionBar::new(count).action(refile)`.
///
/// The count leads; the actions follow, from [`action`](Self::action) or the markup of a
/// `{% filter kit::wrap(bar) %}` block. It sits inside the selection, so its actions send the form.
#[must_use = "a selection bar draws nothing until it is rendered"]
#[derive(Clone, Debug, Template)]
#[template(path = "selection_bar.html")]
pub struct SelectionBar<'a> {
    count: SelectionCount<'a>,
    actions: Vec<Button<'a>>,
    markup: Markup,
    attrs: Attrs<'a>,
}

impl<'a> SelectionBar<'a> {
    /// A bar leading with `count`.
    pub fn new(count: SelectionCount<'a>) -> Self {
        Self {
            count,
            actions: Vec::new(),
            markup: Markup::default(),
            attrs: Attrs::default(),
        }
    }

    /// An action after those already given.
    pub fn action(mut self, button: Button<'a>) -> Self {
        self.actions.push(button);
        self
    }
}

impl Wrap for SelectionBar<'_> {
    fn wrap(mut self, content: Markup) -> Self {
        self.markup = content;
        self
    }
}

component!(SelectionBar);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_selection_is_a_form_counting_its_own_boxes() {
        let html = Selection::new("albums", "path")
            .total(240)
            .none("Tick an album.")
            .to_string();
        assert!(
            html.starts_with("<form data-name=\"path\" data-total=\"240\" data-none=\"Tick an album.\" id=\"albums\""),
            "{html}"
        );
        assert!(html.contains("data-on:change=\""), "{html}");
        assert!(
            html.contains("new MutationObserver"),
            "rows that arrive are counted: {html}"
        );
        assert!(
            !COUNT.contains("[name=") || COUNT.contains("[name=all]"),
            "the rows' name is compared, never written into a selector"
        );
    }

    #[test]
    fn the_bar_shows_only_while_anything_is_ticked() {
        let bar = SelectionBar::new(SelectionCount::new("{} album", "{} albums"))
            .action(Button::bordered("re-file"))
            .to_string();
        assert!(
            bar.starts_with("<div class=\"selectionbar\" hidden data-kit-selected>"),
            "{bar}"
        );
        assert!(
            bar.contains(
                "<span data-kit-count data-one=\"{} album\" data-many=\"{} albums\"></span>"
            ),
            "{bar}"
        );
        assert!(bar.contains("re-file"), "{bar}");
    }

    #[test]
    fn select_all_is_sent_as_all() {
        let all = Selection::select_all("select all 240").to_string();
        assert!(all.contains("name=\"all\""), "{all}");
        assert!(all.contains("value=\"1\""), "{all}");
    }
}
