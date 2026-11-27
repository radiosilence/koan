//! A track's row, as the library, a playlist, search, history and the queue
//! draw it: the same template everywhere. The player reads the track from
//! the row's data attributes, each address among them written here, so the
//! script formats none.

use askama::Template;
use koan_core::db::queries::TrackRow;
use koan_core::helpers::credit_text;
use koan_kit_web as kit;

use super::covers::Versions;
use super::credit::{Credit, Credits, Link};
use super::heart::Hearts;
use crate::html::{Html, Render};
use crate::ui::paths;

/// What a page read once for all its rows: covers, credits and hearts.
pub(in crate::ui) struct Rows<'a> {
    pub(in crate::ui) versions: &'a Versions,
    pub(in crate::ui) credits: &'a Credits,
    pub(in crate::ui) hearts: Option<&'a Hearts>,
}

/// What leads a row.
pub(in crate::ui) enum Lead {
    /// Its place in the list or on the record.
    Number(usize),
    /// A play in history: its time, and a box to pick it with.
    Played { time: String, play: i64 },
    /// A place in the queue, which the player plays from.
    Queued(usize),
}

/// One track's row.
#[derive(Template)]
#[template(path = "parts/track.html")]
pub(in crate::ui) struct Row<'a> {
    t: &'a TrackRow,
    lead: Lead,
    cover: String,
    /// The credit as text, for the player's now-playing metadata.
    artist: String,
    /// The credit for the player's Now Playing, as `credit::for_player`
    /// writes it, where it is more than one name.
    credit: Option<String>,
    /// The credit and record under the title.
    sub: Html,
    heart: Html,
    mark: bool,
    /// Share this track: its album, cued to it.
    share: Option<String>,
}

impl<'a> Row<'a> {
    /// `artist` and `album` choose what the line under the title names.
    pub(in crate::ui) fn new(
        t: &'a TrackRow,
        lead: Lead,
        rows: &Rows,
        artist: bool,
        album: bool,
    ) -> Self {
        let parts = rows.credits.track(t);
        Self {
            t,
            lead,
            cover: t
                .album_id
                .map(|a| rows.versions.url(a, crate::covers::LARGE))
                .unwrap_or_default(),
            artist: credit_text(&parts),
            credit: (parts.len() > 1 || parts.iter().any(|p| p.artist_id.is_some()))
                .then(|| super::credit::for_player(&parts)),
            sub: Sub {
                artist: artist.then(|| Credit::links(&parts)),
                album: (album && !t.album_title.is_empty())
                    .then(|| Link::album(t.album_id, &t.album_title)),
            }
            .html(),
            heart: rows.hearts.map(|h| h.track(t.id)).unwrap_or_default(),
            mark: rows.hearts.is_some_and(|h| h.track_on(t.id)),
            share: None,
        }
    }

    /// With a button sharing it, where the account may share and the page
    /// has the region to show the link in.
    pub(in crate::ui) fn shared(mut self) -> Self {
        self.share = self
            .t
            .album_id
            .map(|a| paths::album_share(a, Some(self.t.id)));
        self
    }

    fn duration(&self) -> String {
        crate::share::duration(self.t.duration_ms)
    }

    fn src(&self) -> String {
        paths::audio(self.t.id)
    }

    fn album_href(&self) -> String {
        self.t.album_id.map(paths::album).unwrap_or_default()
    }

    fn properties(&self) -> String {
        paths::properties(self.t.id)
    }

    fn queued(&self) -> Option<usize> {
        match self.lead {
            Lead::Queued(i) => Some(i),
            _ => None,
        }
    }

    fn share_button(&self) -> Option<kit::Button<'static>> {
        let url = self.share.clone()?;
        Some(
            kit::Button::quiet("Share this track")
                .icon_only(crate::icon::EXPORT)
                .data("share", "track")
                .post(url),
        )
    }

    fn add(&self) -> kit::Button<'static> {
        if self.queued().is_some() {
            kit::Button::quiet("Remove from queue")
                .icon_only(crate::icon::REMOVE)
                .data("act", "remove")
        } else {
            kit::Button::quiet("Add to queue")
                .icon_only(crate::icon::ADD_TO_QUEUE)
                .data("act", "add")
        }
    }

    fn menu(&self) -> kit::Button<'static> {
        kit::Button::quiet("More")
            .icon_only(crate::icon::MORE)
            .class("wide:hidden")
            .data("act", "menu")
            .attr("aria-haspopup", "menu")
    }

    fn mark_html(&self) -> Html {
        if self.mark {
            super::heart::Mark.html()
        } else {
            Html::default()
        }
    }
}

/// A row's subtitle: its artist credit, where given, and its album, where
/// asked for, each named part linked to its page.
#[derive(Template)]
#[template(path = "parts/sub.html")]
struct Sub {
    artist: Option<Html>,
    album: Option<Html>,
}

#[cfg(test)]
mod tests {
    use crate::ui::fixture::*;

#[tokio::test]
async fn track_rows_carry_what_the_track_menu_needs() {
    let f = setup(true);
    let r = send(
        &f.app,
        authed(&f.state, &format!("/album/{}", f.album_id))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert!(
        r.body.contains("<div id=\"track-menu\" popover"),
        "the shell's one menu"
    );
    let row = r
        .body
        .split("<li tabindex=\"0\" data-id=\"")
        .nth(1)
        .expect("a track row");
    let row = &row[..row.find("</li>").unwrap()];
    assert!(row.contains(&format!("data-album-id=\"{}\"", f.album_id)));
    assert!(row.contains("data-artist-id="), "for Go to Artist");
    assert!(row.contains("data-act=\"menu\""), "the phone's way in");
    assert!(row.contains("data-fav="), "the heart the menu presses");
    assert!(row.contains("data-act-share"), "the share the menu presses");
}
}
