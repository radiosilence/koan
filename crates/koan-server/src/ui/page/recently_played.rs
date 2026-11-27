//! Recently played: what the account played lately, each once and newest
//! first by its latest play, as the apps' Recently played shelf.

use koan_core::shelves::Shelf;

use super::{Body, Page, Refusal, Section, favourites::shelf};
use crate::auth::AuthUser;
use crate::ui::UiState;

pub(in crate::ui) struct RecentlyPlayed;

impl Page for RecentlyPlayed {
    const SECTION: Section = Section::RecentlyPlayed;
    type Args = ();

    async fn body(s: &UiState, user: &AuthUser, (): ()) -> Result<Body, Refusal> {
        shelf(
            s,
            user,
            Shelf::Recent,
            "Recently played",
            "Nothing played in the last 30 days.",
            "What you play here, in the kōan apps or in a Subsonic app signed in as you, is gathered here for a month, each artist, record and track once.",
        )
        .await
    }
}
