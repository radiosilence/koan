//! Search: the library's artists, records and tracks matching what is typed,
//! as the apps' search shelf. A form that reads its page in place as it is
//! typed, so the address is always the search shown.

use askama::Template;
use askama_datastar::View;
use koan_core::shelves::Shelf;
use koan_kit_web as kit;

use super::{Body, Page, Refusal, Section};
use crate::auth::AuthUser;
use crate::html::Html;
use crate::share::blocking;
use crate::ui::parts::shelf::Read;
use crate::ui::{UiState, paths};

pub(in crate::ui) struct Search;

#[derive(serde::Deserialize, Default)]
#[serde(default)]
pub(in crate::ui) struct Query {
    q: String,
}

/// What the results show.
enum Found {
    Blank,
    Nothing,
    Shelf(Html),
}

#[derive(Template)]
#[template(path = "search/page.html")]
struct SearchPage<'a> {
    q: &'a str,
    found: Found,
}

impl SearchPage<'_> {
    fn form(&self) -> kit::Form<'static> {
        kit::Form::get(paths::search()).live()
    }

    fn field(&self) -> kit::Input<'static> {
        kit::Input::search("q")
            .value(self.q.to_owned())
            .placeholder("Albums, artists, tracks")
            .label("Search")
            .autocomplete("off")
            .attr("autocapitalize", "none")
            .attr("spellcheck", "false")
            .attr("enterkeyhint", "search")
            .autofocus()
            // Kept by its id as the page is drawn again around it, so typing
            // goes on uninterrupted while the answers arrive.
            .id("search")
            .class("w-full max-w-panel px-3 py-2.5 text-input")
    }

    fn nothing(&self) -> String {
        format!("Nothing matches “{}”.", self.q)
    }
}

impl Page for Search {
    const SECTION: Section = Section::Search;
    type Args = View<Query>;

    async fn body(s: &UiState, user: &AuthUser, View(q): View<Query>) -> Result<Body, Refusal> {
        let query = q.q.trim().to_owned();
        let found = if query.is_empty() {
            Found::Blank
        } else {
            let (st, who, text) = (s.clone(), user.clone(), query.clone());
            blocking(move || {
                let shelf = Shelf::Search(&text);
                let read = Read::of(&st, &who, shelf)?;
                Some(if read.is_empty() {
                    Found::Nothing
                } else {
                    Found::Shelf(read.draw(shelf))
                })
            })
            .await
            .ok_or(Refusal::Unavailable)?
        };
        let view = SearchPage { q: &query, found };
        Ok(Body::of("Search", &view))
    }
}
