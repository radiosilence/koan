//! Favourites: the account's favourite artists, records and tracks, as a
//! shelf.

use askama::Template;
use koan_core::shelves::Shelf;

use super::{Body, Page, Refusal, Section};
use crate::auth::AuthUser;
use crate::html::Html;
use crate::share::blocking;
use crate::ui::UiState;
use crate::ui::parts::shelf::Read;

pub(in crate::ui) struct Favourites;

#[derive(Template)]
#[template(path = "shelf/page.html")]
pub(in crate::ui) struct ShelfPage {
    pub(in crate::ui) title: &'static str,
    pub(in crate::ui) sections: Option<Html>,
    pub(in crate::ui) empty: &'static str,
    pub(in crate::ui) detail: &'static str,
}

/// A shelf page: its title, then its previews, or what it says when empty.
pub(in crate::ui) async fn shelf(
    s: &UiState,
    user: &AuthUser,
    shelf: Shelf<'static>,
    title: &'static str,
    empty: &'static str,
    detail: &'static str,
) -> Result<Body, Refusal> {
    let (st, who) = (s.clone(), user.clone());
    let sections = blocking(move || {
        let read = Read::of(&st, &who, shelf)?;
        Some((!read.is_empty()).then(|| read.draw(shelf)))
    })
    .await
    .ok_or(Refusal::Unavailable)?;
    let view = ShelfPage {
        title,
        sections,
        empty,
        detail,
    };
    Ok(Body::of(title, &view))
}

impl Page for Favourites {
    const SECTION: Section = Section::Favourites;
    type Args = ();

    async fn body(s: &UiState, user: &AuthUser, (): ()) -> Result<Body, Refusal> {
        shelf(
            s,
            user,
            Shelf::Favourites,
            "Favourites",
            "Nothing favourited yet.",
            "Artists, records and tracks favourited in the kōan apps, or in any Subsonic app signed in as you, are listed here.",
        )
        .await
    }
}
