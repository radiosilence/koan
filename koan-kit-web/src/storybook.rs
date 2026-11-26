//! Every component in every state, as the storybook draws it.
//!
//! The storybook at ui.koan.rocks is this module's [`page`], written into `storybook/index.html` by
//! `cargo run -p koan-kit-web --example storybook --features storybook`, and a test fails when that file is stale.
//! Each entry shows the call that drew it, taken from the code itself by `stringify!`, so the reference cannot drift
//! from what it shows. The crate's class test renders every entry and fails on a class `kit.css` has no rule for.
//!
//! The page is static there, with no Datastar, since a static site ships none (§8): it shows the markup and every
//! state. `--serve` runs the same page with Datastar and stub handlers, so a busy button, a question, a preview and a
//! live search can be tried.

use askama::Template;
use askama_datastar::Html;

use crate as kit;
use crate::Wrap as _;

/// One thing drawn, and the call that drew it.
pub struct Entry {
    /// The call, as code.
    pub code: &'static str,
    /// What it drew.
    pub html: Html,
}

/// A part of the storybook: a heading, what it shows, and its entries.
pub struct Section {
    /// The heading, which is also its anchor.
    pub title: &'static str,
    /// One line on what it shows.
    pub intro: &'static str,
    /// What it draws.
    pub entries: Vec<Entry>,
}

impl Section {
    /// Its anchor: the title, hyphenated.
    #[must_use]
    pub fn anchor(&self) -> String {
        self.title.replace(' ', "-")
    }
}

macro_rules! entry {
    ($component:expr) => {
        Entry {
            code: stringify!($component),
            html: Html::from($component),
        }
    };
}

/// The storybook's form, as an app writes it in a template.
const FORM_CODE: &str = r#"{% let form = kit::Form::post("/stub/slow") %}
<form {{ form.attrs() }}>
  {{ kit::Field::text("name", "name").required().hint("What the server calls itself.") }}
  {{ kit::Toggle::new("public", "listed publicly") }}
  {{ form.submit(kit::Button::primary("save")) }}
</form>"#;

/// The storybook's live search, as an app writes it in a template.
const SEARCH_CODE: &str = r#"{% let search = kit::Form::get("/stub/search").live() %}
<form {{ search.attrs() }}>
  {{ kit::Input::search("q").label("search").placeholder("polar bear") }}
</form>
<ul id="story-results" class="rows"></ul>"#;

/// A sheet's body, as a filter block gives it.
const WRAP_CODE: &str = r#"{% filter kit::wrap(kit::Sheet::new("story-wrapped", "rename").close("close")) %}
  <p>The sheet holds whatever the block renders.</p>
{% endfilter %}"#;

/// A form with several actions, as an app writes it in a template.
const ACTIONS_CODE: &str = r#"{% let form = kit::Form::post("/stub/slow").id("story-actions") %}
<form {{ form.attrs() }} class="controls">
  {{ kit::Input::text("tag").label("tag").placeholder("tag").required() }}
  {{ kit::Button::bordered("add").post("/stub/slow").sends("story-actions").busy_key("story-actions") }}
  {{ kit::Button::bordered("remove").post("/stub/slow?remove").sends("story-actions").busy_key("story-actions") }}
</form>"#;

/// A form that asks first, as an app writes it in a template.
const ASKING_FORM_CODE: &str = r#"{% let form = kit::Form::post("/stub/slow").id("story-ban") %}
<form {{ form.attrs() }} class="controls">
  {{ kit::Input::text("username").label("username to ban").placeholder("username").required() }}
  {{ kit::Button::primary("ban").bad().post("/stub/slow").sends("story-ban").confirm("ban this user?", "ban") }}
</form>"#;

/// A compact row, as an app writes it.
const COMPACT_ROW_CODE: &str = r#"<div class="compact controls">
  {{ kit::Input::text("split").label("separator").placeholder("split on") }}
  {{ kit::Combo::new(kit::Select::new("sort").options(["newest", "oldest"]).label("sort"), kit::Button::bordered("sort")) }}
  {{ kit::Button::bordered("credit artists") }}
  {{ kit::Button::quiet("pin").icon_only(kit::icon::PIN) }}
</div>"#;

/// A field beside a bordered button at the default size.
const CONTROLS_CODE: &str = r#"<div class="controls">
  {{ kit::Input::text("room").label("room").placeholder("room name") }}
  {{ kit::Button::bordered("join") }}
</div>"#;

/// Rows a template builds with a loop.
const PAIRS_CODE: &str = r#"{% filter kit::wrap(kit::Pairs::new().pair("last 30 days", "42 views")) %}
  <dt>linked from</dt>
  <dd>{% for (host, n) in referrers %}{% if !loop.first %} · {% endif %}{{ host }} {{ n }}{% endfor %}</dd>
{% endfilter %}"#;

/// A summary written as markup.
const SUMMARY_CODE: &str = r#"{% filter kit::wrap(kit::Disclosure::new("")) %}
  <summary><span>3. Held on the Tips of Fingers</span> <small>4:12</small></summary>
  <p>Polar Bear · FLAC 16/44.1</p>
{% endfilter %}"#;

/// Every section, in the order the page shows them.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "a list of every component in every state, read top to bottom"
)]
pub fn sections() -> Vec<Section> {
    let form = kit::Form::post("/stub/slow");
    let form_html = Html::trusted(format!(
        "<form{}>{}{}{}</form>",
        form.attrs(),
        kit::Field::text("name", "name")
            .required()
            .hint("What the server calls itself."),
        kit::Toggle::new("public", "listed publicly"),
        form.submit(kit::Button::primary("save")),
    ));
    let search = kit::Form::get("/stub/search").live();
    let search_html = Html::trusted(format!(
        "<form{}>{}</form><ul id=\"story-results\" class=\"rows\"></ul>",
        search.attrs(),
        kit::Input::search("q")
            .label("search")
            .placeholder("polar bear"),
    ));
    let actions = kit::Form::post("/stub/slow").id("story-actions");
    let actions_html = Html::trusted(format!(
        "<form{} class=\"controls\">{}{}{}</form>",
        actions.attrs(),
        kit::Input::text("tag")
            .label("tag")
            .placeholder("tag")
            .required(),
        kit::Button::bordered("add")
            .post("/stub/slow")
            .sends("story-actions")
            .busy_key("story-actions"),
        kit::Button::bordered("remove")
            .post("/stub/slow?remove")
            .sends("story-actions")
            .busy_key("story-actions"),
    ));
    let ban = kit::Form::post("/stub/slow").id("story-ban");
    let asking_form = Html::trusted(format!(
        "<form{} class=\"controls\">{}{}</form>",
        ban.attrs(),
        kit::Input::text("username")
            .label("username to ban")
            .placeholder("username")
            .required(),
        kit::Button::primary("ban")
            .bad()
            .post("/stub/slow")
            .sends("story-ban")
            .confirm("ban this user?", "ban"),
    ));
    let compact_row = Html::trusted(format!(
        "<div class=\"compact controls\">{}{}{}{}</div>",
        kit::Input::text("split")
            .label("separator")
            .placeholder("split on"),
        kit::Combo::new(
            kit::Select::new("sort")
                .options(["newest", "oldest"])
                .label("sort"),
            kit::Button::bordered("sort")
        ),
        kit::Button::bordered("credit artists"),
        kit::Button::quiet("pin").icon_only(kit::icon::PIN),
    ));
    let controls = Html::trusted(format!(
        "<div class=\"controls\">{}{}</div>",
        kit::Input::text("room")
            .label("room")
            .placeholder("room name"),
        kit::Button::bordered("join"),
    ));
    let pairs_html = kit::Pairs::new()
        .pair("last 30 days", "42 views")
        .wrap(kit::Markup::from(Html::trusted(
            "<dt>linked from</dt><dd>example.com 30 · other sites 12</dd>",
        )))
        .html();
    let summary_html = kit::Disclosure::new("")
        .wrap(kit::Markup::from(Html::trusted(
            "<summary><span>3. Held on the Tips of Fingers</span> <small>4:12</small></summary><p>Polar Bear · FLAC 16/44.1</p>",
        )))
        .html();
    let wrapped = kit::Sheet::new("story-wrapped", "rename")
        .close("close")
        .body(Html::trusted(
            "<p>The sheet holds whatever the block renders.</p>",
        ));

    vec![
        Section {
            title: "buttons",
            intro: "One type draws every action. Its weight is its constructor; bad, fill and a glyph qualify it.",
            entries: vec![
                entry!(kit::Button::primary("connect")),
                entry!(kit::Button::bordered("bordered")),
                entry!(kit::Button::standard("standard")),
                entry!(kit::Button::quiet("quiet")),
                entry!(kit::Button::link("link")),
                entry!(kit::Button::primary("sign in").href("#buttons")),
                entry!(kit::Button::bordered("disconnect").bad()),
                entry!(kit::Button::quiet("remove").bad()),
                entry!(kit::Button::bordered("delete").bad().fill()),
                entry!(kit::Button::bordered("filled").fill()),
                entry!(kit::Button::primary("connecting").busy(true)),
                entry!(kit::Button::bordered("disabled").disabled(true)),
                entry!(kit::Button::bordered("export").icon(kit::icon::EXPORT)),
                entry!(kit::Button::quiet("more").icon_only(kit::icon::MORE)),
                entry!(kit::Button::quiet("play").icon_only(kit::icon::PLAY).toggles(kit::icon::PAUSE, false)),
                entry!(kit::Button::quiet("mute").icon_only(kit::icon::VOLUME).toggles(kit::icon::MUTED, true)),
                entry!(kit::Button::quiet("pin").icon_only(kit::icon::PIN).compact()),
            ],
        },
        Section {
            title: "compact",
            intro: "The compact size, for controls in a row or a toolbar: a field, a combo and a button at this size line up exactly. A compact class on a container compacts everything inside it.",
            entries: vec![
                entry!(kit::Input::text("split").placeholder("split on").label("separator").compact()),
                entry!(kit::Combo::new(
                    kit::Input::search("q").placeholder("filter").label("filter"),
                    kit::Button::bordered("go")
                ).compact()),
                entry!(kit::Button::bordered("re-tag").compact()),
                entry!(kit::Button::primary("apply").compact()),
                entry!(kit::Field::number("slots", "upload slots").value("4").compact()),
                Entry {
                    code: COMPACT_ROW_CODE,
                    html: compact_row,
                },
                Entry {
                    code: CONTROLS_CODE,
                    html: controls,
                },
            ],
        },
        Section {
            title: "requests",
            intro: "A request carries its URL as data, and the button is busy for exactly as long as its own request runs.",
            entries: vec![
                entry!(kit::Button::primary("rescan").post("/stub/slow")),
                entry!(kit::Button::bordered("retry").post("/stub/slow").busy_key("card-1")),
                entry!(kit::Button::bordered("next copy").post("/stub/slow?next").busy_key("card-1")),
                entry!(kit::Button::bordered("say saved").get("/stub/flash")),
                entry!(kit::Button::bordered("say it failed").get("/stub/failed")),
                entry!(kit::Button::bordered("move with a link").get("/stub/moved")),
                entry!(kit::Button::bordered("raise from script").datastar(
                    "on:click",
                    "dispatchEvent(new CustomEvent('kit-flash', {detail: {message: 'Raised by the page.', stays: true}}))"
                )),
                entry!(kit::Link::new("load in place", "/stub/place").in_place().busy_key("story-place")),
                entry!(kit::Status::wait("queued").busy_with("card-1")),
            ],
        },
        Section {
            title: "asking first",
            intro: "A question, or the server's preview of a change, in the page's one Confirm. Yes waits for its own preview.",
            entries: vec![
                entry!(kit::Button::bordered("remove album")
                    .bad()
                    .post("/stub/slow")
                    .confirm("remove Held on the Tips of Fingers from the library?", "remove")),
                entry!(kit::Button::quiet("close unpinned")
                    .post("/stub/slow")
                    .confirm("close every unpinned search?", "close them")
                    .detail("Pinned searches stay open; the rest lose their results.")
                    .yes_bad()),
                entry!(kit::Button::primary("rename files")
                    .post("/stub/slow")
                    .preview("/stub/preview", "rename three files?", "rename")),
                Entry {
                    code: ASKING_FORM_CODE,
                    html: asking_form,
                },
            ],
        },
        Section {
            title: "sheets",
            intro: "A dialog opened and closed through commandfor, with no script: rising from the bottom on a phone.",
            entries: vec![
                entry!(kit::Button::primary("add a server").opens("story-sheet")),
                entry!(kit::Sheet::new("story-sheet", "add a server")
                    .close("close")
                    .body("A dialog that rises from the bottom on a phone and is a centred panel elsewhere.")
                    .action(kit::Button::quiet("cancel").closes("story-sheet"))
                    .action(kit::Button::primary("add").closes("story-sheet"))),
                entry!(kit::Button::bordered("open a wrapped sheet").opens("story-wrapped")),
                Entry {
                    code: WRAP_CODE,
                    html: wrapped.html(),
                },
            ],
        },
        Section {
            title: "fields",
            intro: "A label leading, the control filling the width, and a note under it that describes the control.",
            entries: vec![
                entry!(kit::Field::password("token", "api token")
                    .placeholder("paste the token")
                    .hint("Stored sealed; the gateway never shows it again.")),
                entry!(kit::Field::select("region", "region", [("eu-west", "eu-west"), ("us-east", "us-east")]).value("us-east")),
                entry!(kit::Field::url("server", "server url")
                    .value("htp://example")
                    .error("Not a URL: it must start with https://.")),
                entry!(kit::Field::textarea("notes", "notes")
                    .value("Text a person wrote keeps its Case.")
                    .warning("Saved on this device only.")),
                entry!(kit::Field::number("slots", "upload slots").value("4").min(0).max(20)),
                entry!(kit::Combo::new(
                    kit::Input::text("split").placeholder("split on").label("separator"),
                    kit::Button::bordered("credit artists")
                )),
                entry!(kit::Combo::new(
                    kit::Input::search("query").value("Artist Album").label("search"),
                    kit::Button::primary("grab")
                )),
            ],
        },
        Section {
            title: "choices",
            intro: "A box leading its words inline, or a settings row with the box where a switch would be.",
            entries: vec![
                entry!(kit::Check::new("lossless", "lossless only").checked(true)),
                entry!(kit::Check::radio("quality", "lossless", "lossless").checked(true)),
                entry!(kit::Check::radio("quality", "any", "any")),
                entry!(kit::Toggle::new("notify", "notify when done").checked(true)),
                entry!(kit::Toggle::new("lossy", "include lossy copies")),
                entry!(kit::Check::new("path", "select Untrue").bare().value("/music/untrue")),
                entry!(kit::Check::radio("to", "", "a new album").picture_icon(kit::icon::PLUS).checked(true)),
                entry!(kit::Check::radio("to", "a1", "Holiday, 2024").picture("").detail("12 images · 48 MB")),
            ],
        },
        Section {
            title: "forms",
            intro: "A form keeps its method and action, and its submit is busy while the form's request runs.",
            entries: vec![
                Entry {
                    code: FORM_CODE,
                    html: form_html,
                },
                Entry {
                    code: SEARCH_CODE,
                    html: search_html,
                },
                Entry {
                    code: ACTIONS_CODE,
                    html: actions_html,
                },
                entry!(kit::Field::custom("release type")
                    .hint("A group of controls a label cannot hold, named by the field's label.")
                    .wrap(kit::Markup::from(Html::trusted(concat!(
                        r#"<span class="controls">"#,
                        r#"<select name="primary" aria-label="primary type"><option>album</option><option>single</option></select>"#,
                        r#"<label class="check"><input type="checkbox" name="clear"> clear</label></span>"#
                    ))))),
            ],
        },
        Section {
            title: "state",
            intro: "A status's word says what its colour does; a meter is named for a screen reader.",
            entries: vec![
                entry!(kit::Status::ok("connected")),
                entry!(kit::Status::warn("not configured")),
                entry!(kit::Status::bad("rejected")),
                entry!(kit::Status::wait("queued")),
                entry!(kit::Status::wait("checking").busy(true)),
                entry!(kit::Status::wait("checking").loads("/stub/status").id("story-status")),
                entry!(kit::Spinner::new()),
                entry!(kit::Spinner::new().label("loading the album")),
                entry!(kit::Spinner::new().large().label("buffering")),
                entry!(kit::Meter::new(40, 100).label("download")),
                entry!(kit::Meter::new(80, 100).tone(kit::Tone::Ok).label("scan")),
                entry!(kit::Meter::new(30, 100).tone(kit::Tone::Warn).label("disk")),
                entry!(kit::Meter::new(60, 100).tone(kit::Tone::Bad).label("errors")),
                entry!(kit::Meter::waiting().label("connecting")),
            ],
        },
        Section {
            title: "messages",
            intro: "A notice says something about the page in place; an empty state stands where a list would be.",
            entries: vec![
                entry!(kit::Notice::new("Saved. The next sign-in uses it.")),
                entry!(kit::Notice::warn("The token expires in two days.")),
                entry!(kit::Notice::bad("The server refused the token: 401.")
                    .action(kit::Button::bordered("retry").post("/stub/slow"))),
                entry!(kit::Notice::warn("Unreachable since 14:02.").icon(kit::icon::WARNING)),
                entry!(kit::Empty::new("no rooms joined").compact()),
                entry!(kit::Empty::new("nothing downloading")
                    .detail("Albums you grab appear here.")
                    .icon(kit::icon::DOWNLOAD)),
            ],
        },
        Section {
            title: "content",
            intro: "Labelled pairs, a disclosure that opens or loads, and a value to copy.",
            entries: vec![
                entry!(kit::Pairs::new()
                    .pair("format", "FLAC 24/96")
                    .pair("size", "1.2 GB")
                    .optional("label", None::<&str>)
                    .pair("source", kit::Button::link("soulseek").href("#content"))),
                Entry {
                    code: PAIRS_CODE,
                    html: pairs_html,
                },
                entry!(kit::Disclosure::new("connection details").body("What only some readers need.")),
                entry!(kit::Disclosure::new("files").loads("/stub/files", "Reading…").id("story-files")),
                Entry {
                    code: SUMMARY_CODE,
                    html: summary_html,
                },
                entry!(kit::CopyField::new("mcp url", "https://mcp.example.com/mcp/github", "copy").copied("copied")),
                entry!(kit::CopyField::new("share link", "https://dither.example/a/x1", "copy").copied("copied").icon_only().name("link").id("story-share")),
            ],
        },
        Section {
            title: "icons",
            intro: "The kōan set, on a 24-unit grid, its stroke set by the size it is drawn at.",
            entries: kit::icon::ALL
                .iter()
                .map(|glyph| Entry {
                    code: glyph.name,
                    html: glyph.svg(24).label(glyph.name).html(),
                })
                .collect(),
        },
    ]
}

/// The storybook's page.
#[derive(Template)]
#[template(path = "storybook.html")]
struct Page {
    head: Html,
    sections: Vec<Section>,
    confirm: Html,
    flash: Html,
}

/// The storybook: static, for ui.koan.rocks, or `live` with Datastar from `/datastar.js` for `--serve`.
///
/// # Panics
///
/// Never: the page holds components and markup, whose `Display` cannot fail.
#[must_use]
pub fn page(live: bool) -> Html {
    crate::action::repeat_serials();
    let head = kit::Head::new("out.css").title("kōan web kit");
    let head = if live {
        head.datastar("/datastar.js")
    } else {
        head
    };
    let page = Page {
        head: head.html(),
        sections: sections(),
        confirm: kit::Confirm::new("cancel").html(),
        flash: kit::Flash::region().html(),
    };
    Html::trusted(
        page.render()
            .expect("the storybook holds components and markup, whose Display cannot fail"),
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    /// The class names kit.css and tokens.css give a rule.
    fn kit_classes() -> BTreeSet<String> {
        let css = [include_str!("../kit.css"), include_str!("../tokens.css")].concat();
        let mut classes = BTreeSet::new();
        for line in css.lines() {
            let selector = line.trim();
            if selector.starts_with("@apply")
                || selector.starts_with("/*")
                || selector.starts_with('*')
            {
                continue;
            }
            let mut rest = selector;
            while let Some(at) = rest.find('.') {
                rest = &rest[at + 1..];
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                    .collect();
                if name.starts_with(|c: char| c.is_ascii_alphabetic()) {
                    classes.insert(name);
                }
            }
        }
        classes
    }

    /// Every class attribute's names in `html`.
    fn classes_in(html: &str) -> Vec<String> {
        let mut classes = Vec::new();
        let mut rest = html;
        while let Some(at) = rest.find(" class=\"") {
            rest = &rest[at + 8..];
            let end = rest.find('"').expect("class attributes are closed");
            classes.extend(rest[..end].split_whitespace().map(str::to_owned));
            rest = &rest[end..];
        }
        classes
    }

    #[test]
    fn every_class_a_component_writes_has_a_rule_in_the_kit() {
        let kit = kit_classes();
        let page_layout = ["rows"];
        for section in sections() {
            for entry in section.entries {
                for class in classes_in(entry.html.as_str()) {
                    assert!(
                        kit.contains(&class) || page_layout.contains(&class.as_str()),
                        "{} draws .{class}, which kit.css has no rule for: {}",
                        section.title,
                        entry.code
                    );
                }
            }
        }
    }

    #[test]
    fn the_storybook_is_fresh() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/storybook/index.html");
        let committed = std::fs::read_to_string(path).unwrap_or_default();
        assert!(
            committed == page(false).as_str(),
            "storybook/index.html is stale: run `cargo run -p koan-kit-web --example storybook --features storybook`"
        );
    }
}
