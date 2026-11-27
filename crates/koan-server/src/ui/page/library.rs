//! Library: the library's own lists, which the sidebar shows as links and a
//! phone, with one tab for them all, as this page; and for an admin, the
//! rescan and the caches, which the page's stream keeps current.

use askama::Template;

use super::{Body, Page, Refusal, Section};
use crate::auth::AuthUser;
use crate::html::Html;
use crate::share::blocking;
use crate::ui::{UiState, is_admin, paths};

pub(in crate::ui) mod caches;
pub(in crate::ui) mod rescan;

pub(in crate::ui) struct Library;

#[derive(Template)]
#[template(path = "library/page.html")]
struct LibraryPage {
    /// The rescan and the caches, for an admin.
    admin: Option<(Html, Html)>,
}

impl LibraryPage {
    fn lists(&self) -> [(&'static str, &'static str); 5] {
        [
            (paths::playlists(), "Playlists"),
            (paths::tracks(), "Tracks"),
            (paths::recently_played(), "Recently played"),
            (paths::favourites(), "Favourites"),
            (paths::history(), "History"),
        ]
    }
}

impl Page for Library {
    const SECTION: Section = Section::Library;
    type Args = ();

    async fn body(s: &UiState, user: &AuthUser, (): ()) -> Result<Body, Refusal> {
        let admin = if is_admin(user) {
            let state = s.clone();
            let caches = blocking(move || caches::status(&state))
                .await
                .ok_or(Refusal::Unavailable)?;
            Some((rescan::section(s), caches::section(caches)))
        } else {
            None
        };
        Ok(Body::of("Library", &LibraryPage { admin }))
    }
}
