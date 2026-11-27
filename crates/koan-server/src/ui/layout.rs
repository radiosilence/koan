//! The shell every page is drawn in: the nav, which is the sidebar on a wide
//! screen and the tab bar on a phone; the transport bar and the player's
//! script; the track menu; and `main`, the page's own region, which a link
//! or form loading in place replaces. Its regions are the title, the nav and
//! `main`.

use askama::Template;
use koan_kit_web as kit;

use super::page::Section;
use super::{UiState, paths};
use crate::auth::AuthUser;
use crate::html::Html;

/// The document every page is drawn in, around the page's own body.
#[derive(Template)]
#[template(path = "layout.html")]
pub(super) struct Layout {
    /// What `<base href>` names.
    pub(super) base: String,
    pub(super) title: Html,
    pub(super) nav: Html,
    pub(super) main: Html,
    /// The page's starting signals, as JSON.
    pub(super) signals: String,
    /// The live stream's address, under the base path.
    pub(super) live: String,
    /// An authenticating proxy is trusted, so the page renews its session
    /// from the proxy rather than a refresh cookie.
    pub(super) proxied: bool,
}

impl Layout {
    fn assets(&self) -> &'static super::AssetUrls {
        &super::ASSETS
    }

    fn head(&self) -> kit::Head<'static> {
        kit::Head::new(super::ASSETS.css.as_str())
            .datastar(&paths::asset("datastar.js"))
            .import_map(&super::ASSETS.imports)
            .noindex()
    }

    fn icons(&self) -> Html {
        super::Icons {
            under: "./ui/assets",
        }
        .html()
    }

    /// The live stream, opened again whenever it ends or goes quiet, and
    /// afresh when the page moves.
    fn live(&self) -> Html {
        askama_datastar::reopen_on(&self.live, &["url"])
    }

    fn renew(&self) -> [&'static str; 2] {
        if self.proxied {
            [paths::proxy_renew(), paths::renew()]
        } else {
            [paths::renew(), paths::proxy_renew()]
        }
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    /// The stylesheet's address without its version, for the script to swap
    /// in the one a response names after an upgrade.
    fn stylesheet(&self) -> String {
        paths::asset("ui.css")
    }
}

/// The document's title: what the page shows, then the app.
#[derive(Template)]
#[template(path = "layout/title.html")]
pub(super) struct Title<'a> {
    pub(super) name: &'a str,
}

/// The page's own region, which a link or form loading in place replaces.
#[derive(Template)]
#[template(path = "layout/main.html")]
pub(super) struct Main {
    pub(super) body: Html,
}

/// The nav, the entry for the page shown marked. On a wide screen it ends
/// with who is signed in and the version; on a phone it is six tabs, each as
/// wide as its label, which fit a 360 px screen at the meta size, so there
/// Playlists and the other lists become Library, a page of them all.
#[derive(Template)]
#[template(path = "layout/nav.html")]
pub(super) struct Nav<'a> {
    section: Section,
    username: &'a str,
    /// Whether the wide sidebar links the Library page, for its rescan.
    admin: bool,
    auth_enabled: bool,
}

/// A link in the nav: where it goes, what it says, the section it marks, and
/// where it shows.
struct Entry {
    href: &'static str,
    label: &'static str,
    current: fn(Section) -> bool,
    /// Classes placing it: hidden on one width or the other, or set apart.
    place: &'static str,
}

/// The nav row's look: lowercase in `muted`, the page shown in the accent
/// with a rule on its leading edge, and no fill. On a phone, the tab bar's:
/// the page shown in the accent and underlined.
const ENTRY: &str = "border-l-2 border-transparent px-2.5 py-1.5 text-muted lowercase hover:text-ink hover:no-underline aria-[current=page]:border-brand aria-[current=page]:text-brand max-wide:flex max-wide:flex-auto max-wide:items-center max-wide:justify-center max-wide:border-l-0 max-wide:px-0 max-wide:text-meta max-wide:underline-offset-4 max-wide:aria-[current=page]:underline";

impl<'a> Nav<'a> {
    pub(super) fn new(s: &UiState, user: &'a AuthUser, section: Section) -> Self {
        Self {
            section,
            username: &user.username,
            admin: super::is_admin(user),
            auth_enabled: s.auth_enabled,
        }
    }

    fn entry(&self) -> &'static str {
        ENTRY
    }

    fn entries(&self) -> Vec<Entry> {
        let mut all = vec![
            Entry {
                href: paths::albums(),
                label: "Albums",
                current: |s| s == Section::Albums,
                place: "",
            },
            Entry {
                href: paths::artists(),
                label: "Artists",
                current: |s| s == Section::Artists,
                place: "",
            },
            Entry {
                href: paths::tracks(),
                label: "Tracks",
                current: |s| s == Section::Tracks,
                place: "max-wide:hidden",
            },
            Entry {
                href: paths::playlists(),
                label: "Playlists",
                current: |s| s == Section::Playlists,
                place: "max-wide:hidden",
            },
            Entry {
                href: paths::library(),
                label: "Library",
                current: Section::in_library,
                place: "wide:hidden",
            },
            Entry {
                href: paths::search(),
                label: "Search",
                current: |s| s == Section::Search,
                place: "",
            },
            Entry {
                href: paths::queue(),
                label: "Queue",
                current: |s| s == Section::Queue,
                place: "",
            },
            Entry {
                href: paths::recently_played(),
                label: "Recently played",
                current: |s| s == Section::RecentlyPlayed,
                place: "mt-3 max-wide:hidden",
            },
            Entry {
                href: paths::favourites(),
                label: "Favourites",
                current: |s| s == Section::Favourites,
                place: "max-wide:hidden",
            },
            Entry {
                href: paths::history(),
                label: "History",
                current: |s| s == Section::History,
                place: "max-wide:hidden",
            },
        ];
        // The Library page holds the rescan and every scan's progress. A
        // phone reaches it through its Library tab; a wide screen lists the
        // page's other links itself, so only an admin needs it there.
        if self.admin {
            all.push(Entry {
                href: paths::library(),
                label: "Library",
                current: |s| s == Section::Library,
                place: "max-wide:hidden",
            });
        }
        all.push(Entry {
            href: paths::account(),
            label: "Account",
            current: |s| s == Section::Account,
            place: "wide:hidden",
        });
        all
    }

    fn current(&self, entry: &Entry) -> bool {
        (entry.current)(self.section)
    }

    fn on_account(&self) -> bool {
        self.section == Section::Account
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }
}
