//! Sorting and filtering the album, artist and track browsers.
//!
//! The state is the page's query string and nothing else: a reload, the back
//! button and a copied link all land on the same listing, and the server reads
//! it straight into the SQL that narrows and orders the library.
//!
//! The shelves are filters here: `fav`, `recent` and `q` narrow a browser
//! exactly as `koan_core::shelves` narrows a shelf, so a shelf page's section
//! heading (`shelf_browser`) opens the listing its preview is the head of.

use askama::Template;
use koan_core::db::queries::{
    AlbumFilter, AlbumOrder, AlbumQuery, ArtistOrder, ArtistQuery, TrackFilter, TrackOrder,
};
use koan_core::shelves::{Shelf, Tracks};
use serde::Deserialize;

use crate::html::{Html, Render};

/// A browser's query string. Blank values, as an unfilled form field sends
/// them, mean unset.
#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub(super) struct Browse {
    sort: String,
    /// Fixes a random order, so a reload or a link lands on the same shuffle.
    seed: Option<i64>,
    /// The name filter: album title or artist name on the album browser, the
    /// artist's name on the artist browser.
    q: String,
    fav: String,
    /// Played in the last `shelves::RECENT_DAYS`.
    recent: String,
    lossless: String,
    codec: String,
    from: String,
    to: String,
    genre: String,
    /// The track browser's page, from 0.
    pub page: u32,
}

/// Album sorts, as the macOS app offers them.
const ALBUM_SORTS: [(&str, &str); 6] = [
    ("recent", "Recently added"),
    ("title", "Title"),
    ("artist", "Artist"),
    ("year", "Year, newest"),
    ("year-asc", "Year, oldest"),
    ("random", "Random"),
];

const ARTIST_SORTS: [(&str, &str); 3] = [
    ("name", "Name"),
    ("albums", "Most albums"),
    ("recent", "Recently added"),
];

const TRACK_SORTS: [(&str, &str); 4] = [
    ("artist", "Artist"),
    ("title", "Title"),
    ("album", "Album"),
    ("duration", "Length"),
];

/// Offered only while the recently played filter is on: there is nothing to
/// sort by otherwise.
const PLAYED: (&str, &str) = ("played", "Last played");

/// Offered only while a search narrows the listing: the closest matches
/// first, the search shelf's order.
const BEST: (&str, &str) = ("best", "Best match");

/// Which browser.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Kind {
    Albums,
    Artists,
    Tracks,
}

impl Kind {
    /// What the browser sorts by when nothing is asked for.
    fn default_sort(self) -> &'static str {
        match self {
            Kind::Albums => ALBUM_SORTS[0].0,
            Kind::Artists => ARTIST_SORTS[0].0,
            Kind::Tracks => TRACK_SORTS[0].0,
        }
    }

    pub(super) fn path(self) -> &'static str {
        match self {
            Kind::Albums => "/albums",
            Kind::Artists => "/artists",
            Kind::Tracks => "/tracks",
        }
    }
}

fn set(s: &str) -> Option<&str> {
    Some(s.trim()).filter(|s| !s.is_empty())
}

impl Browse {
    /// A seed for a random order that arrived without one.
    pub(super) fn seeded(mut self) -> Self {
        if self.sort == "random" && self.seed.is_none() {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.subsec_nanos() as i64 ^ d.as_secs() as i64);
            self.seed = Some(now);
        }
        self
    }

    fn filter(&self) -> AlbumFilter<'_> {
        let year = |s: &str| set(s).and_then(|y| y.parse().ok());
        AlbumFilter {
            lossless: set(&self.lossless).is_some(),
            codec: set(&self.codec),
            year_from: year(&self.from),
            year_to: year(&self.to),
            genre: set(&self.genre),
            // The server's library is all on the server.
            on_device: false,
        }
    }

    fn recent_on(&self) -> bool {
        set(&self.recent).is_some()
    }

    /// The sort asked for, or the one the browser opens on: last played while
    /// the recently played filter is on.
    fn sort_or(&self, default: &'static str) -> &str {
        match set(&self.sort) {
            Some(s) => s,
            None if self.recent_on() => PLAYED.0,
            None => default,
        }
    }

    /// `user` is whose favourites and plays the shelf filters narrow to, at
    /// `now`.
    pub(super) fn albums(&self, user: i64, now: i64) -> AlbumQuery<'_> {
        let order = match self.sort_or(Kind::Albums.default_sort()) {
            "title" => AlbumOrder::Title,
            "artist" => AlbumOrder::ArtistThenDate,
            "year" => AlbumOrder::YearDesc,
            "year-asc" => AlbumOrder::YearAsc,
            "random" => AlbumOrder::Random(self.seed.unwrap_or(0)),
            "played" => AlbumOrder::LastPlayed,
            "best" => AlbumOrder::Relevance,
            _ => AlbumOrder::RecentlyAdded,
        };
        AlbumQuery {
            order,
            search: set(&self.q),
            favourites_of: set(&self.fav).and(Shelf::Favourites.albums(user, now).favourites_of),
            played: self
                .recent_on()
                .then(|| Shelf::Recent.albums(user, now).played)
                .flatten(),
            filter: self.filter(),
            ..Default::default()
        }
    }

    pub(super) fn artists(&self, user: i64, now: i64) -> ArtistQuery<'_> {
        let order = match self.sort_or(Kind::Artists.default_sort()) {
            "albums" => ArtistOrder::AlbumCount,
            "recent" => ArtistOrder::RecentlyAdded,
            "played" => ArtistOrder::LastPlayed,
            "best" => ArtistOrder::Relevance,
            _ => ArtistOrder::Name,
        };
        ArtistQuery {
            order,
            search: set(&self.q),
            favourites_of: set(&self.fav).and(Shelf::Favourites.artists(user, now).favourites_of),
            played: self
                .recent_on()
                .then(|| Shelf::Recent.artists(user, now).played)
                .flatten(),
            filter: self.filter(),
            ..Default::default()
        }
    }

    /// The track browser's listing. `q` is the full-text index, as the search
    /// shelf's tracks are.
    pub(super) fn tracks(&self, user: i64, now: i64) -> Tracks {
        let (order, descending) = match self.sort_or(Kind::Tracks.default_sort()) {
            "title" => (TrackOrder::Title, false),
            "album" => (TrackOrder::Album, false),
            "duration" => (TrackOrder::Duration, false),
            "played" => (TrackOrder::LastPlayed, true),
            "best" => (TrackOrder::Relevance, false),
            _ => (TrackOrder::ArtistAlbumDiscTrack, false),
        };
        let shelf = |s: Shelf| s.tracks(user, now).filter;
        let year = |s: &str| set(s).and_then(|y| y.parse().ok());
        Tracks {
            filter: TrackFilter {
                search: set(&self.q).and_then(|q| Shelf::Search(q).tracks(user, now).filter.search),
                favourites_of: set(&self.fav).and(shelf(Shelf::Favourites).favourites_of),
                played: self
                    .recent_on()
                    .then(|| shelf(Shelf::Recent).played)
                    .flatten(),
                codec: set(&self.codec).map(str::to_owned),
                genre: set(&self.genre).map(str::to_owned),
                year_start: year(&self.from),
                year_end: year(&self.to),
                ..Default::default()
            },
            order,
            descending,
        }
    }

    /// Whether anything narrows the listing.
    pub(super) fn filtered(&self) -> bool {
        self.active() > 0
    }

    /// How many filters are on, for the collapsed toolbar's label.
    fn active(&self) -> usize {
        [
            &self.q,
            &self.fav,
            &self.recent,
            &self.lossless,
            &self.codec,
            &self.genre,
        ]
        .into_iter()
        .filter(|v| set(v).is_some())
        .count()
            + usize::from(set(&self.from).is_some() || set(&self.to).is_some())
    }

    /// The query string for this state, blank values dropped.
    /// Percent-encoded throughout, so it is safe inside an attribute and a
    /// quoted script string alike.
    pub(super) fn query(&self) -> String {
        let mut q = form_urlencoded::Serializer::new(String::new());
        let seed = self.seed.map(|s| s.to_string()).unwrap_or_default();
        for (k, v) in [
            ("sort", self.sort.as_str()),
            ("seed", &seed),
            ("q", &self.q),
            ("fav", &self.fav),
            ("recent", &self.recent),
            ("lossless", &self.lossless),
            ("codec", &self.codec),
            ("from", &self.from),
            ("to", &self.to),
            ("genre", &self.genre),
        ] {
            if let Some(v) = set(v) {
                q.append_pair(k, v);
            }
        }
        q.finish()
    }
}

/// Where a shelf section's heading goes: the browser for `kind` with the
/// shelf as its filter and the shelf's order as its sort, so the listing it
/// opens is the one the preview was the head of.
pub(super) fn shelf_browser(shelf: Shelf, kind: Kind) -> String {
    let mut b = Browse::default();
    let sort = match (shelf, kind) {
        (Shelf::Recent, _) => PLAYED.0,
        (Shelf::Search(_), _) => BEST.0,
        (Shelf::Favourites, Kind::Artists) => "name",
        (Shelf::Favourites, _) => "artist",
        (Shelf::Downloaded, Kind::Artists) => "name",
        (Shelf::Downloaded, _) => "artist",
    };
    match shelf {
        Shelf::Favourites => b.fav = "1".into(),
        Shelf::Recent => b.recent = "1".into(),
        Shelf::Search(q) => b.q = q.into(),
        // The web UI downloads nothing, so it has no such shelf to link from.
        Shelf::Downloaded => {}
    }
    b.sort = sort.into();
    format!("{}?{}", kind.path(), b.query())
}

/// A labelled control in the toolbar; on a phone, label and control at either
/// end of a row of the sheet.
pub(super) const LABEL: &str = "inline-flex items-center gap-1.5 max-wide:justify-between";
/// The toolbar's selects and year fields, smaller than a form's.
pub(super) const FIELD: &str = "px-2 py-[5px] text-meta max-wide:text-input";

/// A select's options: value, text, and whether it is the one chosen.
type Options = Vec<(String, String, bool)>;

#[derive(Template)]
#[template(path = "browse/toolbar.html")]
struct Toolbar<'a> {
    path: &'static str,
    /// How many filters are on, shown on the phone's button.
    active: usize,
    sorts: Options,
    /// Kept with a random order, so applying a filter keeps the shuffle.
    seed: Option<i64>,
    /// The query for a fresh shuffle, on the album browser's random order.
    reshuffle: Option<String>,
    fav: bool,
    recent: bool,
    /// Whether lossless is on; `None` on the track browser, which has none.
    lossless: Option<bool>,
    codecs: Options,
    genres: Options,
    name_hint: &'static str,
    name_width: &'static str,
    q: &'a str,
    from: &'a str,
    to: &'a str,
}

/// "Any", then each value offered, then the current one if a link set one
/// that is not offered, so it still shows.
fn options(offered: &[String], current: &str) -> Options {
    let mut out = vec![(String::new(), "Any".to_owned())];
    out.extend(offered.iter().map(|v| (v.clone(), v.clone())));
    if let Some(c) = set(current)
        && !out.iter().any(|(v, _)| v == c)
    {
        out.push((c.to_owned(), c.to_owned()));
    }
    chosen(out, current)
}

fn chosen(options: Vec<(String, String)>, current: &str) -> Options {
    options
        .into_iter()
        .map(|(v, t)| {
            let on = v == current;
            (v, t, on)
        })
        .collect()
}

/// The sort and filter controls: a row above the grid on a wide screen, one
/// "Sort · Filter" button that opens them on a phone. A plain GET form, so it
/// works without script; with it, a change applies at once.
pub(super) fn toolbar(b: &Browse, kind: Kind, codecs: &[String], genres: &[String]) -> Html {
    let mut sorts: Vec<(String, String)> = match kind {
        Kind::Albums => &ALBUM_SORTS[..],
        Kind::Artists => &ARTIST_SORTS[..],
        Kind::Tracks => &TRACK_SORTS[..],
    }
    .iter()
    .map(|(v, t)| (v.to_string(), t.to_string()))
    .collect();
    if b.recent_on() {
        sorts.insert(0, (PLAYED.0.into(), PLAYED.1.into()));
    }
    if set(&b.q).is_some() {
        sorts.insert(0, (BEST.0.into(), BEST.1.into()));
    }
    let sort = b.sort_or(kind.default_sort()).to_owned();
    let random = sort == "random";
    Toolbar {
        path: kind.path(),
        active: b.active(),
        seed: b.seed.filter(|_| random),
        reshuffle: (random && kind == Kind::Albums).then(|| {
            Browse {
                seed: None,
                ..b.clone()
            }
            .query()
        }),
        sorts: chosen(sorts, &sort),
        fav: set(&b.fav).is_some(),
        recent: b.recent_on(),
        lossless: (kind != Kind::Tracks).then(|| set(&b.lossless).is_some()),
        codecs: options(codecs, &b.codec),
        genres: options(genres, &b.genre),
        name_hint: match kind {
            Kind::Albums => "Album or artist",
            Kind::Artists => "Artist",
            Kind::Tracks => "Title, artist or record",
        },
        // Wide enough for the hint.
        name_width: match kind {
            Kind::Tracks => "w-[17em]",
            Kind::Albums | Kind::Artists => "w-[12em]",
        },
        q: &b.q,
        from: &b.from,
        to: &b.to,
    }
    .html()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn browse(q: &str) -> Browse {
        let uri: axum::http::Uri = format!("/albums?{q}").parse().unwrap();
        axum::extract::Query::<Browse>::try_from_uri(&uri)
            .unwrap()
            .0
    }

    #[test]
    fn blank_fields_are_unset_and_the_query_round_trips() {
        let b =
            browse("sort=year&q=+aphex+&fav=&lossless=1&codec=&from=1990&to=&genre=Drum+%26+Bass");
        let q = b.albums(0, 0);
        assert_eq!(q.search, Some("aphex"));
        assert_eq!(b.artists(0, 0).search, Some("aphex"));
        assert_eq!(q.order, AlbumOrder::YearDesc);
        assert_eq!(
            browse("sort=year-asc").albums(0, 0).order,
            AlbumOrder::YearAsc
        );
        assert!(q.favourites_of.is_none() && q.filter.lossless);
        assert_eq!(
            (q.filter.codec, q.filter.year_from, q.filter.year_to),
            (None, Some(1990), None)
        );
        assert_eq!(q.filter.genre, Some("Drum & Bass"));
        assert_eq!(
            b.query(),
            "sort=year&q=aphex&lossless=1&from=1990&genre=Drum+%26+Bass"
        );
        assert_eq!(b.active(), 4);
    }

    /// A shelf's heading opens its own listing: the same narrowing, the same
    /// order. The counts and rows matching follow from that; the UI tests
    /// check them against a library.
    #[test]
    fn a_shelf_heading_links_to_its_shelfs_listing() {
        let (user, now) = (7, 1_800_000_000);
        for shelf in [Shelf::Favourites, Shelf::Recent, Shelf::Search("moss")] {
            let open = |kind: Kind| {
                let link = shelf_browser(shelf, kind);
                assert!(link.starts_with(kind.path()), "{link}");
                browse(link.split_once('?').unwrap().1)
            };
            let (want, got) = (shelf.albums(user, now), open(Kind::Albums));
            let got = got.albums(user, now);
            assert_eq!(
                (got.order, got.favourites_of, got.played, got.search),
                (want.order, want.favourites_of, want.played, want.search),
                "{shelf:?} albums"
            );
            let (want, got) = (shelf.artists(user, now), open(Kind::Artists));
            let got = got.artists(user, now);
            assert_eq!(
                (got.order, got.favourites_of, got.played, got.search),
                (want.order, want.favourites_of, want.played, want.search),
                "{shelf:?} artists"
            );
            let (want, got) = (
                shelf.tracks(user, now),
                open(Kind::Tracks).tracks(user, now),
            );
            assert_eq!(
                (
                    got.order,
                    got.descending,
                    got.filter.favourites_of,
                    got.filter.played
                ),
                (
                    want.order,
                    want.descending,
                    want.filter.favourites_of,
                    want.filter.played
                ),
                "{shelf:?} tracks"
            );
            assert_eq!(got.filter.search, want.filter.search);
        }
    }

    #[test]
    fn a_random_order_keeps_its_seed_and_hostile_values_stay_encoded() {
        let b = browse("sort=random").seeded();
        let seed = b.seed.unwrap();
        assert_eq!(b.albums(0, 0).order, AlbumOrder::Random(seed));
        assert!(b.query().contains(&format!("seed={seed}")));
        let evil = browse("genre=%27%29%3Balert(1)%2F%2F%22%3E%3C");
        let q = evil.query();
        assert!(
            !q.contains('\'') && !q.contains('"') && !q.contains('<'),
            "{q}"
        );
    }
}
