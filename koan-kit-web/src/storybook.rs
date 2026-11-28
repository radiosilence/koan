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
use crate::attrs::render;

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

/// An example written as an app writes it, in a template: the code shown is the template that drew it.
macro_rules! story {
    ($name:ident, $file:literal $(, $field:ident: $ty:ty = $value:expr)*) => {{
        #[derive(Template)]
        #[template(path = $file)]
        struct $name {
            $($field: $ty,)*
        }
        Entry {
            code: include_str!(concat!("../templates/", $file)).trim_end(),
            html: render(&$name { $($field: $value,)* }),
        }
    }};
}

/// Every section, in the order the page shows them.
///
/// # Panics
///
/// Never: its QR code's link is short enough for a code.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "a list of every component in every state, read top to bottom"
)]
pub fn sections() -> Vec<Section> {
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
                story!(CompactRow, "story/compact_row.html"),
                story!(Controls, "story/controls.html"),
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
                entry!(kit::Button::bordered("fail on the server").get("/stub/error")),
                story!(CoverStory, "story/cover.html"),
            ],
        },
        Section {
            title: "frame",
            intro: "The bar over an app's pages and the foot under them, each clear of a phone's notch and home indicator.",
            entries: vec![
                story!(Frame, "story/topbar.html"),
                entry!(kit::Wordmark::new("dither", "/").home_label("dither: your albums")),
                entry!(kit::Footer::new().body(kit::Release::new("koan-kit-web", "0.9.0", "https://github.com/radiosilence/koan"))),
            ],
        },
        Section {
            title: "navigation",
            intro: "Tabs, cards and lists whose links load in place. One Navigation in the shell loads every link of a marked region in place, and Back and Forward with them.",
            entries: vec![
                entry!(kit::Tabs::new("searches")
                    .tab(kit::Tab::new("Burial", "/stub/place?burial").current(true).in_place())
                    .tab(kit::Tab::new("Phace", "/stub/place?phace").count(12).in_place()
                        .action(kit::Button::quiet("close").icon_only(kit::icon::CLOSE).compact().post("/stub/slow")))
                    .tab(kit::Tab::new("Polar Bear", "/stub/place?polar").in_place())
                    .tab(kit::Tab::new("Moodymann", "/stub/place?moodymann").busy(true).in_place())
                    .tab(kit::Tab::new("Basic Channel", "/stub/place?basic").tone(kit::Tone::Bad).in_place())
                    .tab(kit::Tab::new("Shackleton", "/stub/place?shackleton").count(31).italic().in_place())),
                entry!(kit::Status::wait("last checked an hour ago").italic()),
                story!(Card, "story/card.html"),
                // Shown, not drawn: the page's one Navigation is in its shell, and a second would take every click.
                Entry {
                    code: "kit::Navigation::new().follows(\"url\") // once, in the shell",
                    html: Html::new(),
                },
                story!(InPlace, "story/in_place.html"),
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
                story!(AskingForm, "story/asking_form.html"),
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
                story!(Wrapped, "story/wrap.html"),
                entry!(kit::Button::bordered("move to…").opens("story-loads")),
                entry!(kit::Sheet::new("story-loads", "move to")
                    .loads("/stub/sheet")
                    .action(kit::Button::quiet("cancel").closes("story-loads"))),
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
                entry!(kit::Field::number("slots", "upload slots").value("4").min(0).max(20).short()),
                entry!(kit::Field::text("a-genre", "genre").placeholder("leave as is").clearable("clear-genre", "clear")),
                entry!(kit::Field::select("show", "which albums", [("", "every album"), ("dups", "held more than once")]).submits()),
                entry!(kit::Field::select("model", "model", ["…"]).loads("/stub/model").id("story-model")),
                entry!(kit::Combo::new(
                    kit::Input::text("split").placeholder("split on").label("separator"),
                    kit::Button::bordered("credit artists")
                )),
                entry!(kit::Combo::new(
                    kit::Input::search("query").value("Artist Album").label("search"),
                    kit::Button::primary("grab")
                )),
                entry!(kit::RepeatField::new(
                    "magnet links",
                    kit::Input::url("magnet").placeholder("paste a magnet link"),
                    "add another",
                    "remove this link"
                ).values(["magnet:?xt=urn:btih:1", "magnet:?xt=urn:btih:2"]).paste("paste from the clipboard").split_words().hint("Paste several at once and each takes a row.")),
            ],
        },
        Section {
            title: "choices",
            intro: "A box leading its words inline, or a settings row with the box where a switch would be, and a list of rows to tick and act on together.",
            entries: vec![
                entry!(kit::Check::new("lossless", "lossless only").checked(true)),
                entry!(kit::Check::radio("quality", "lossless", "lossless").checked(true)),
                entry!(kit::Check::radio("quality", "any", "any")),
                entry!(kit::Toggle::new("notify", "notify when done").checked(true)),
                entry!(kit::Toggle::new("lossy", "include lossy copies")),
                entry!(kit::Check::new("path", "select Untrue").bare().value("/music/untrue")),
                entry!(kit::Check::radio("to", "", "a new album").picture_icon(kit::icon::PLUS).checked(true)),
                entry!(kit::Check::radio("to", "a1", "Holiday, 2024").picture("").detail("12 images · 48 MB")),
                story!(
                    SelectionStory,
                    "story/selection.html",
                    albums: Vec<(&'static str, &'static str)> = vec![
                        ("/music/untrue", "Burial: Untrue"),
                        ("/music/love", "Phace: Love Songs"),
                        ("/music/held", "Polar Bear: Held on the Tips of Fingers"),
                    ]
                ),
            ],
        },
        Section {
            title: "forms",
            intro: "A form keeps its method and action, and its submit is busy while the form's request runs.",
            entries: vec![
                story!(FormStory, "story/form.html"),
                story!(Search, "story/search.html"),
                story!(Actions, "story/actions.html"),
                story!(Group, "story/group.html"),
                story!(
                    Repeated,
                    "story/repeated.html",
                    credentials: Vec<(&'static str, &'static str)> = vec![
                        ("story-cred-github", "/stub/slow?github"),
                        ("story-cred-slack", "/stub/slow?slack"),
                    ]
                ),
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
                entry!(kit::Status::neutral("configured")),
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
            intro: "Labelled pairs, a bulleted list of values, times in the reader's zone and counters of time spent or left, rendered Markdown, a disclosure that opens or loads, and a value to copy.",
            entries: vec![
                entry!(kit::Pairs::new()
                    .pair("format", "FLAC 24/96")
                    .pair("size", "1.2 GB")
                    .optional("label", None::<&str>)
                    .pair("source", kit::Button::link("soulseek").href("#content"))),
                entry!(kit::Pairs::new().pair("released", "2007").pair("label", "Hyperdub").tight()),
                story!(
                    PairsStory,
                    "story/pairs.html",
                    referrers: Vec<(&'static str, u32)> = vec![("example.com", 30), ("other sites", 12)]
                ),
                entry!(kit::List::new(["mail.example.com", "calendar.read", "an.unbroken.value.that.wraps.anywhere.rather.than.running.off.the.page"]).strong()),
                entry!(kit::List::new(Vec::<&str>::new()).empty("no scopes asked for")),
                entry!(kit::Time::millis(1_795_791_315_000)),
                entry!(kit::Time::millis(1_795_791_315_000).date()),
                entry!(kit::Time::millis(1_795_791_315_000).clock()),
                entry!(kit::Time::millis(1_795_791_315_000).relative()),
                entry!(kit::Time::millis(1_795_791_315_000).elapsed().as_of(1_795_791_562_000)),
                entry!(kit::Time::millis(1_795_795_215_000).remaining().as_of(1_795_791_315_000)),
                story!(Prose, "story/prose.html"),
                entry!(kit::Disclosure::new("connection details").body("What only some readers need.")),
                entry!(kit::Disclosure::new("files").loads("/stub/files", "Reading…").id("story-files")),
                story!(Summary, "story/summary.html"),
                entry!(kit::CopyField::new("mcp url", "https://mcp.example.com/mcp/github", "copy").copied("copied")),
                entry!(kit::CopyField::new("connector url", "https://mcp.example.com/mcp/github", "copy")
                    .copied("copied")
                    .description("Settings → Connectors → Add custom connector, paste this URL, and sign in when prompted.")
                    .id("story-connector")),
                entry!(kit::Qr::new("https://koan.rocks/join/x1").expect("fits").label("invite link")),
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
    navigation: Html,
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
        flash: kit::Flash::region()
            .failed("That did not work. Try again.")
            .unreachable("The storybook's server could not be reached.")
            .html(),
        navigation: kit::Navigation::new().follows("url").html(),
    };
    render(&page)
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
