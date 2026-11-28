//! A list of rows to tick and act on together: a [`Selection`], its [`SelectionCount`], and the [`SelectionBar`] of
//! actions that sticks to the foot of the window while anything is ticked.

use askama::Template;

use crate::arg::{Amount, Markup, Text};
use crate::attrs::{Attrs, component};
use crate::button::Button;
use crate::choice::Check;
use crate::slot::Wrap;

/// What a selection does as it starts: keep its counts, its bars, its conditional actions and its validity in step
/// with its own boxes, now, on every change, whenever rows holding a box arrive or leave, and after every patch
/// Datastar applies, since a morph can reset a box without adding or removing one.
///
/// Its own boxes are its form's `elements` sent under its `data-name`: never those of a selection nested inside it,
/// and including those outside that name it with `form=`. Names are compared rather than written into a selector, so
/// none has to be escaped. Its counts, bars and conditional actions are those whose nearest selection it is. Ticking
/// "all" stands for `data-total` rows, when more match than are drawn. A count writes `data-one` or `data-many`, `{}`
/// replaced by the count, as text. A conditional action (`data-kit-when`) shows while a ticked box's row carries its
/// tag in `data-kit-tags`, or while "all" is ticked and `data-all-tags` holds it. While none is ticked, the bars are
/// hidden and the first box says `data-none`, which a request sending the form reports instead of sending. Patches and
/// new rows recount at most once a frame.
const COUNT: &str = concat!(
    "(el._kitsel = () => (e => (b => (a => (t => (n => (has => (",
    "el.querySelectorAll('[data-kit-count], [data-kit-selected], [data-kit-when]').forEach(c => c.closest('[data-kit-selection]') == el",
    " && (c.dataset.kitWhen ? c.hidden = !(n && has(c.dataset.kitWhen))",
    " : 'one' in c.dataset ? (w => c.textContent != w && (c.textContent = w))((n == 1 ? c.dataset.one : c.dataset.many).replace('{}', n))",
    " : c.hidden = !n)),",
    " b[0]?.setCustomValidity(n || !el.dataset.none ? '' : el.dataset.none)))",
    "(g => (a?.checked ? b : t).some(c => (c.closest('[data-kit-tags]')?.dataset.kitTags || '').split(' ').includes(g))",
    " || !!a?.checked && (el.dataset.allTags || '').split(' ').includes(g)))",
    "(a?.checked ? +(el.dataset.total || b.length) : t.length))",
    "(b.filter(c => c.checked)))",
    "(e.find(c => c.name == 'all')))",
    "(e.filter(c => c.name == el.dataset.name)))",
    "([...el.elements]),",
    " el._kitsoon = () => el._kitq || (el._kitq = requestAnimationFrame(() => (el._kitq = 0, el._kitsel()))),",
    " new MutationObserver(ms => ms.some(m => [...m.addedNodes, ...m.removedNodes].some(x => x.nodeType == 1",
    " && (x.matches('input') || !!x.querySelector('input')))) && el._kitsoon()).observe(el, {childList: true, subtree: true}),",
    " el._kitsel())",
);

/// What a selection does when one of its own boxes changes: "all" ticks or unticks every row, a row unticked unticks
/// "all", and the counts follow. A change in a selection nested inside it is that one's.
const CHANGE: &str = concat!(
    "(t => t.form == el && (t.name == 'all' ? [...el.elements].forEach(c => c.name == el.dataset.name && (c.checked = t.checked))",
    " : t.name == el.dataset.name && !t.checked && [...el.elements].forEach(c => c.name == 'all' && (c.checked = false)),",
    " el._kitsel()))(evt.target)",
);

/// What a selection does after Datastar applies a patch anywhere on the page: recount, once the patch has landed.
const PATCHED: &str = "evt.detail.type == 'datastar-patch-elements' && el._kitsoon?.()";

/// Rows to tick and act on together: `kit::Selection::new("albums", "path")` round a list whose rows each hold a box
/// sent as `path`.
///
/// It is a form, so an action that sends it ([`Button::sends`]) sends the rows ticked. Its rows are the markup of a
/// `{% filter kit::wrap(selection) %}` block: the boxes, a [`select_all`](Self::select_all) box if it has one, and any
/// [`SelectionCount`] and [`SelectionBar`], which it keeps up to date as boxes change, as rows arrive and as patches
/// morph them. With [`none`](Self::none), a request sending it with nothing ticked says so rather than sending.
///
/// It counts and acts on its own boxes only. A selection inside another, such as a preview's own list drawn into a
/// row, is counted by itself; since an HTML parser drops a form inside a form, the inner one is drawn into the outer
/// by a patch, or stands outside it with its boxes naming it through `form=`.
///
/// A row's box can carry tags, such as `kit::Check::new("path", title).data("kit-tags", "spare")`, or the row round it
/// can, for an action that applies to only some rows: [`SelectionBar::action_when`].
#[must_use = "a selection draws nothing until it is rendered"]
#[derive(Clone, Debug, Template)]
#[template(path = "selection.html")]
pub struct Selection<'a> {
    name: Text<'a>,
    total: Option<Amount>,
    none: Text<'a>,
    all_tags: Vec<Text<'a>>,
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
            all_tags: Vec::new(),
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

    /// Some of every match carries `tag`, including rows not drawn, so an action for it shows while "all" is ticked:
    /// the handler knows, the page holds only the rows drawn.
    pub fn all_have(mut self, tag: impl Into<Text<'a>>) -> Self {
        self.all_tags.push(tag.into());
        self
    }

    fn all_tags(&self) -> String {
        self.all_tags
            .iter()
            .map(Text::as_str)
            .collect::<Vec<_>>()
            .join(" ")
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
    actions: Vec<(Option<Text<'a>>, Button<'a>)>,
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
        self.actions.push((None, button));
        self
    }

    /// An action shown only while a ticked row carries `tag`, or while "all" is ticked and the selection says
    /// [`all_have`](Selection::all_have) it: "bin spares", for the copies among the albums ticked.
    pub fn action_when(mut self, tag: impl Into<Text<'a>>, button: Button<'a>) -> Self {
        self.actions.push((Some(tag.into()), button));
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
            .all_have("spare")
            .to_string();
        assert!(
            html.starts_with("<form data-kit-selection data-name=\"path\" data-total=\"240\" data-none=\"Tick an album.\" data-all-tags=\"spare\" id=\"albums\""),
            "{html}"
        );
        assert!(html.contains("data-on:change=\""), "{html}");
        assert!(
            COUNT.contains("[...el.elements]") && !COUNT.contains("el.querySelectorAll('input')"),
            "its own boxes are its form's elements, never a nested selection's"
        );
        assert!(
            COUNT.contains("c.closest('[data-kit-selection]') == el"),
            "a nested selection's counts and bars are its own"
        );
        assert!(
            CHANGE.starts_with("(t => t.form == el &&"),
            "a nested selection's change is not this one's"
        );
        assert!(
            !COUNT.contains("[name="),
            "the rows' name is compared, never written into a selector"
        );
    }

    #[test]
    fn a_selection_recounts_after_patches_at_most_once_a_frame() {
        let html = Selection::new("albums", "path").to_string();
        assert!(
            html.contains("data-on:datastar-fetch__document=\"evt.detail.type == &#39;datastar-patch-elements&#39; &#38;&#38; el._kitsoon?.()\""),
            "a morph that resets a box adds no node, so a patch recounts: {html}"
        );
        assert!(
            COUNT.contains("requestAnimationFrame"),
            "recounts are folded into one a frame"
        );
        assert!(
            COUNT.contains("x.matches('input') || !!x.querySelector('input')"),
            "only nodes holding a box can change the count"
        );
    }

    #[test]
    fn an_action_for_some_rows_waits_for_one_of_them() {
        let bar = SelectionBar::new(SelectionCount::new("{} album", "{} albums"))
            .action(Button::bordered("re-file"))
            .action_when("spare", Button::bordered("bin spares"))
            .to_string();
        assert!(
            bar.contains("<span data-kit-when=\"spare\" hidden><button"),
            "{bar}"
        );
        assert!(
            COUNT.contains("dataset.kitTags"),
            "tags are read from the box's row"
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
