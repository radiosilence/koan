//! Sorting and filtering the album, artist and track browsers.
//!
//! The state is the page's address and nothing else: a reload, the back
//! button and a copied link all land on the same listing, and the server
//! reads it straight into the SQL that narrows and orders the library. The
//! toolbar is a form that reads its page in place as it changes.
//!
//! The shelves are filters here: `fav`, `recent` and `q` narrow a browser
//! exactly as `koan_core::shelves` narrows a shelf, so a shelf page's section
//! heading ([`shelf_browser`]) opens the listing its preview is the head of.

use std::str::FromStr;
use std::sync::Arc;

use askama::Template;
use koan_core::db::queries::{
    self, AlbumFilter, AlbumOrder, AlbumQuery, ArtistOrder, ArtistQuery, TrackFilter, TrackOrder,
};
use koan_core::shelves::{Shelf, Tracks};
use koan_kit_web as kit;
use serde::{Deserialize, Deserializer};

use crate::html::{Html, Render};
use crate::ui::{UiState, open, paths};

/// How many genres the filter offers.
const GENRES_OFFERED: u32 = 80;

/// The order a browser lists in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::ui) enum Sort {
    /// Recently added.
    Recent,
    Title,
    Artist,
    /// Year, newest first.
    Year,
    YearAsc,
    Random,
    Name,
    /// Most albums first.
    Albums,
    Album,
    Duration,
    /// Last played, while the recently played filter is on.
    Played,
    /// Best match, while a search narrows the listing.
    Best,
}

impl Sort {
    const ALL: [Sort; 12] = [
        Sort::Recent,
        Sort::Title,
        Sort::Artist,
        Sort::Year,
        Sort::YearAsc,
        Sort::Random,
        Sort::Name,
        Sort::Albums,
        Sort::Album,
        Sort::Duration,
        Sort::Played,
        Sort::Best,
    ];

    fn as_str(self) -> &'static str {
        match self {
            Sort::Recent => "recent",
            Sort::Title => "title",
            Sort::Artist => "artist",
            Sort::Year => "year",
            Sort::YearAsc => "year-asc",
            Sort::Random => "random",
            Sort::Name => "name",
            Sort::Albums => "albums",
            Sort::Album => "album",
            Sort::Duration => "duration",
            Sort::Played => "played",
            Sort::Best => "best",
        }
    }

    fn label(self, kind: Kind) -> &'static str {
        match self {
            Sort::Recent => "Recently added",
            Sort::Title => "Title",
            Sort::Artist => "Artist",
            Sort::Year => "Year, newest",
            Sort::YearAsc => "Year, oldest",
            Sort::Random => "Random",
            Sort::Name => "Name",
            Sort::Albums => "Most albums",
            Sort::Album if kind == Kind::Tracks => "Album",
            Sort::Album => "Album",
            Sort::Duration => "Length",
            Sort::Played => "Last played",
            Sort::Best => "Best match",
        }
    }
}

impl FromStr for Sort {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, ()> {
        Sort::ALL.into_iter().find(|v| v.as_str() == s).ok_or(())
    }
}

/// Which browser.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::ui) enum Kind {
    Albums,
    Artists,
    Tracks,
}

impl Kind {
    /// The sorts it offers, as the macOS app offers them, the first the one
    /// it opens on.
    fn sorts(self) -> &'static [Sort] {
        match self {
            Kind::Albums => &[
                Sort::Recent,
                Sort::Title,
                Sort::Artist,
                Sort::Year,
                Sort::YearAsc,
                Sort::Random,
            ],
            Kind::Artists => &[Sort::Name, Sort::Albums, Sort::Recent],
            Kind::Tracks => &[Sort::Artist, Sort::Title, Sort::Album, Sort::Duration],
        }
    }

    fn route(self) -> &'static str {
        match self {
            Kind::Albums => paths::ALBUMS,
            Kind::Artists => paths::ARTISTS,
            Kind::Tracks => paths::TRACKS,
        }
    }
}

/// A browser's address. A value that does not parse, as a blank form field
/// or an old link sends, is unset rather than an error, so one stale value
/// costs only itself.
#[derive(Deserialize, Default, Clone, Debug, PartialEq, Eq)]
#[serde(default)]
pub(in crate::ui) struct Browse {
    #[serde(deserialize_with = "lenient")]
    sort: Option<Sort>,
    /// Fixes a random order, so a reload or a link lands on the same shuffle.
    #[serde(deserialize_with = "lenient")]
    seed: Option<i64>,
    /// The name filter: album title or artist name on the album browser, the
    /// artist's name on the artist browser.
    q: String,
    #[serde(deserialize_with = "flag")]
    fav: bool,
    /// Played in the last `shelves::RECENT_DAYS`.
    #[serde(deserialize_with = "flag")]
    recent: bool,
    #[serde(deserialize_with = "flag")]
    lossless: bool,
    codec: String,
    #[serde(deserialize_with = "lenient")]
    from: Option<i32>,
    #[serde(deserialize_with = "lenient")]
    to: Option<i32>,
    genre: String,
    /// The track browser's page, from 0.
    #[serde(deserialize_with = "lenient_page")]
    pub(in crate::ui) page: u32,
}

/// A value, or none where it is blank or does not parse.
fn lenient<'de, D: Deserializer<'de>, T: FromStr>(d: D) -> Result<Option<T>, D::Error> {
    let s = String::deserialize(d)?;
    Ok(s.trim().parse().ok())
}

fn lenient_page<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    lenient(d).map(Option::unwrap_or_default)
}

/// A checkbox: on when sent with any value but a blank or `0`.
pub(in crate::ui) fn flag<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    let s = String::deserialize(d)?;
    Ok(!matches!(s.trim(), "" | "0" | "false" | "off"))
}

fn set(s: &str) -> Option<&str> {
    Some(s.trim()).filter(|s| !s.is_empty())
}

impl Browse {
    /// A seed for a random order that arrived without one.
    pub(in crate::ui) fn seeded(mut self) -> Self {
        if self.sort == Some(Sort::Random) && self.seed.is_none() {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.subsec_nanos() as i64 ^ d.as_secs() as i64);
            self.seed = Some(now);
        }
        self
    }

    fn filter(&self) -> AlbumFilter<'_> {
        AlbumFilter {
            lossless: self.lossless,
            codec: set(&self.codec),
            year_from: self.from,
            year_to: self.to,
            genre: set(&self.genre),
            // The server's library is all on the server.
            on_device: false,
            ..Default::default()
        }
    }

    /// The sort asked for, where `kind` offers it, or the one it opens on:
    /// last played while the recently played filter is on.
    fn sort(&self, kind: Kind) -> Sort {
        let offered = |s: &Sort| {
            kind.sorts().contains(s)
                || (*s == Sort::Played && self.recent)
                || (*s == Sort::Best && set(&self.q).is_some())
        };
        match self.sort.filter(offered) {
            Some(s) => s,
            None if self.recent => Sort::Played,
            None => kind.sorts()[0],
        }
    }

    /// `user` is whose favourites and plays the shelf filters narrow to, at
    /// `now`.
    pub(in crate::ui) fn albums(&self, user: i64, now: i64) -> AlbumQuery<'_> {
        let order = match self.sort(Kind::Albums) {
            Sort::Title => AlbumOrder::Title,
            Sort::Artist => AlbumOrder::ArtistThenDate,
            Sort::Year => AlbumOrder::YearDesc,
            Sort::YearAsc => AlbumOrder::YearAsc,
            Sort::Random => AlbumOrder::Random(self.seed.unwrap_or(0)),
            Sort::Played => AlbumOrder::LastPlayed,
            Sort::Best => AlbumOrder::Relevance,
            _ => AlbumOrder::RecentlyAdded,
        };
        AlbumQuery {
            order,
            search: set(&self.q),
            favourites_of: if self.fav {
                Shelf::Favourites.albums(user, now).favourites_of
            } else {
                None
            },
            played: self
                .recent
                .then(|| Shelf::Recent.albums(user, now).played)
                .flatten(),
            filter: self.filter(),
            ..Default::default()
        }
    }

    pub(in crate::ui) fn artists(&self, user: i64, now: i64) -> ArtistQuery<'_> {
        let order = match self.sort(Kind::Artists) {
            Sort::Albums => ArtistOrder::AlbumCount,
            Sort::Recent => ArtistOrder::RecentlyAdded,
            Sort::Played => ArtistOrder::LastPlayed,
            Sort::Best => ArtistOrder::Relevance,
            _ => ArtistOrder::Name,
        };
        ArtistQuery {
            order,
            search: set(&self.q),
            favourites_of: if self.fav {
                Shelf::Favourites.artists(user, now).favourites_of
            } else {
                None
            },
            played: self
                .recent
                .then(|| Shelf::Recent.artists(user, now).played)
                .flatten(),
            filter: self.filter(),
            ..Default::default()
        }
    }

    /// The track browser's listing. `q` is the full-text index, as the search
    /// shelf's tracks are.
    pub(in crate::ui) fn tracks(&self, user: i64, now: i64) -> Tracks {
        let (order, descending) = match self.sort(Kind::Tracks) {
            Sort::Title => (TrackOrder::Title, false),
            Sort::Album => (TrackOrder::Album, false),
            Sort::Duration => (TrackOrder::Duration, false),
            Sort::Played => (TrackOrder::LastPlayed, true),
            Sort::Best => (TrackOrder::Relevance, false),
            _ => (TrackOrder::ArtistAlbumDiscTrack, false),
        };
        let shelf = |s: Shelf| s.tracks(user, now).filter;
        Tracks {
            filter: TrackFilter {
                search: set(&self.q).and_then(|q| Shelf::Search(q).tracks(user, now).filter.search),
                favourites_of: if self.fav {
                    shelf(Shelf::Favourites).favourites_of
                } else {
                    None
                },
                played: self.recent.then(|| shelf(Shelf::Recent).played).flatten(),
                codec: set(&self.codec).map(str::to_owned),
                genre: set(&self.genre).map(str::to_owned),
                year_start: self.from,
                year_end: self.to,
                ..Default::default()
            },
            order,
            descending,
        }
    }

    /// Whether anything narrows the listing.
    pub(in crate::ui) fn filtered(&self) -> bool {
        self.active() > 0
    }

    /// How many filters are on, for the collapsed toolbar's label.
    fn active(&self) -> usize {
        [
            set(&self.q).is_some(),
            self.fav,
            self.recent,
            self.lossless,
            set(&self.codec).is_some(),
            set(&self.genre).is_some(),
            self.from.is_some() || self.to.is_some(),
        ]
        .into_iter()
        .filter(|on| *on)
        .count()
    }

    /// The query string for this state, unset values left out.
    /// Percent-encoded throughout, so it is safe inside an attribute.
    pub(in crate::ui) fn query(&self) -> String {
        let mut q = form_urlencoded::Serializer::new(String::new());
        let seed = self.seed.map(|s| s.to_string());
        let from = self.from.map(|y| y.to_string());
        let to = self.to.map(|y| y.to_string());
        let on = |b: bool| b.then_some("1");
        for (k, v) in [
            ("sort", self.sort.map(Sort::as_str)),
            ("seed", seed.as_deref()),
            ("q", set(&self.q)),
            ("fav", on(self.fav)),
            ("recent", on(self.recent)),
            ("lossless", on(self.lossless)),
            ("codec", set(&self.codec)),
            ("from", from.as_deref()),
            ("to", to.as_deref()),
            ("genre", set(&self.genre)),
        ] {
            if let Some(v) = v {
                q.append_pair(k, v);
            }
        }
        q.finish()
    }

    /// This listing's address at track page `page`.
    pub(in crate::ui) fn at_page(&self, page: u32) -> String {
        let mut q = self.query();
        if page > 0 {
            if !q.is_empty() {
                q.push('&');
            }
            q.push_str(&format!("page={page}"));
        }
        paths::browser(paths::TRACKS, &q)
    }
}

/// Where a shelf section's heading goes: the browser for `kind` with the
/// shelf as its filter and the shelf's order as its sort, so the listing it
/// opens is the one the preview was the head of.
pub(in crate::ui) fn shelf_browser(shelf: Shelf, kind: Kind) -> String {
    let mut b = Browse::default();
    let sort = match (shelf, kind) {
        (Shelf::Recent, _) => Sort::Played,
        (Shelf::Search(_), _) => Sort::Best,
        (Shelf::Favourites | Shelf::Downloaded, Kind::Artists) => Sort::Name,
        (Shelf::Favourites | Shelf::Downloaded, _) => Sort::Artist,
    };
    match shelf {
        Shelf::Favourites => b.fav = true,
        Shelf::Recent => b.recent = true,
        Shelf::Search(q) => b.q = q.into(),
        // The web UI downloads nothing, so it has no such shelf to link from.
        Shelf::Downloaded => {}
    }
    b.sort = Some(sort);
    paths::browser(kind.route(), &b.query())
}

/// The codecs and genres the filters offer.
pub(in crate::ui) type Options = Arc<(Vec<String>, Vec<String>)>;

/// The filter options, read again only when the library has moved
/// (`library_stamp`): counting genres reads every track. Read outside the
/// lock, and a failed read is shown as no options and not kept.
pub(in crate::ui) fn options(s: &UiState) -> Option<Options> {
    let db = open(&s.pool)?;
    let stamp = match queries::library_stamp(&db.conn) {
        Ok(stamp) => stamp.key(),
        Err(e) => {
            log::warn!("library stamp: {e}");
            return Some(Options::default());
        }
    };
    if let Some((at, options)) = s.options.lock().ok()?.as_ref()
        && *at == stamp
    {
        return Some(options.clone());
    }
    let read = queries::album_codecs(&db.conn)
        .and_then(|codecs| Ok((codecs, queries::genres(&db.conn, GENRES_OFFERED)?)));
    match read {
        Ok(read) => {
            let options = Arc::new(read);
            *s.options.lock().ok()? = Some((stamp, options.clone()));
            Some(options)
        }
        Err(e) => {
            log::warn!("filter options: {e}");
            Some(Options::default())
        }
    }
}

/// The sort and filter controls: a row above the listing on a wide screen,
/// one "sort · filter" disclosure on a phone. A form that reads its page in
/// place as it changes; without script, an ordinary GET.
#[derive(Template)]
#[template(path = "parts/toolbar.html")]
pub(in crate::ui) struct Toolbar<'a> {
    b: &'a Browse,
    kind: Kind,
    codecs: &'a [String],
    genres: &'a [String],
}

impl<'a> Toolbar<'a> {
    pub(in crate::ui) fn of(b: &'a Browse, kind: Kind, options: &'a Options) -> Html {
        Self {
            b,
            kind,
            codecs: &options.0,
            genres: &options.1,
        }
        .html()
    }

    fn form(&self) -> kit::Form<'static> {
        kit::Form::get(paths::browser(self.kind.route(), "")).live()
    }

    /// The phone's disclosure: "sort · filter", and how many filters are on.
    fn summary(&self) -> String {
        match self.b.active() {
            0 => "sort · filter".to_owned(),
            n => format!("sort · filter · {n}"),
        }
    }

    fn sort(&self) -> kit::Select<'static> {
        let mut sorts: Vec<Sort> = self.kind.sorts().to_vec();
        if self.b.recent {
            sorts.insert(0, Sort::Played);
        }
        if set(&self.b.q).is_some() {
            sorts.insert(0, Sort::Best);
        }
        let kind = self.kind;
        kit::Select::new("sort")
            .options(sorts.into_iter().map(|s| (s.as_str(), s.label(kind))))
            .selected(self.b.sort(kind).as_str())
            .compact()
    }

    /// Kept with a random order, so applying a filter keeps the shuffle.
    fn seed(&self) -> Option<String> {
        self.b
            .seed
            .filter(|_| self.b.sort(self.kind) == Sort::Random)
            .map(|s| s.to_string())
    }

    /// The address of a fresh shuffle, on the album browser's random order.
    fn reshuffle(&self) -> Option<String> {
        (self.kind == Kind::Albums && self.b.sort(self.kind) == Sort::Random).then(|| {
            let fresh = Browse {
                seed: None,
                ..self.b.clone()
            };
            paths::browser(self.kind.route(), &fresh.query())
        })
    }

    fn reset(&self) -> String {
        paths::browser(self.kind.route(), "")
    }

    fn name(&self) -> kit::Input<'static> {
        let (hint, width) = match self.kind {
            Kind::Albums => (
                "Album or artist",
                "w-[12em] max-wide:w-auto max-wide:flex-1",
            ),
            Kind::Artists => ("Artist", "w-[12em] max-wide:w-auto max-wide:flex-1"),
            Kind::Tracks => (
                "Title, artist or record",
                "w-[17em] max-wide:w-auto max-wide:flex-1",
            ),
        };
        kit::Input::search("q")
            .value(self.b.q.clone())
            .placeholder(hint)
            .label("Filter by name")
            .class(width)
            .compact()
    }

    fn checks(&self) -> Vec<kit::Check<'static>> {
        let mut checks = vec![
            kit::Check::new("fav", "favourites")
                .value("1")
                .checked(self.b.fav),
            kit::Check::new("recent", "recently played")
                .value("1")
                .checked(self.b.recent),
        ];
        // A track's codec says as much; the album browsers' lossless means a
        // whole record.
        if self.kind != Kind::Tracks {
            checks.push(
                kit::Check::new("lossless", "lossless")
                    .value("1")
                    .checked(self.b.lossless),
            );
        }
        checks
    }

    /// "Any", then each value offered, then the current one if a link set one
    /// that is not offered, so it still shows.
    fn choice(
        &self,
        name: &'static str,
        offered: &[String],
        current: &str,
    ) -> kit::Select<'static> {
        let mut values: Vec<String> = offered.to_vec();
        if let Some(c) = set(current)
            && !values.iter().any(|v| v == c)
        {
            values.push(c.to_owned());
        }
        kit::Select::new(name)
            .option("", "Any")
            .options(values.into_iter().map(|v| (v.clone(), v)))
            .selected(current.to_owned())
            .compact()
    }

    fn codec(&self) -> kit::Select<'static> {
        self.choice("codec", self.codecs, &self.b.codec)
    }

    fn genre(&self) -> kit::Select<'static> {
        self.choice("genre", self.genres, &self.b.genre)
    }

    fn year(
        &self,
        name: &'static str,
        value: Option<i32>,
        label: &'static str,
    ) -> kit::Input<'static> {
        kit::Input::text(name)
            .value(value.map(|y| y.to_string()).unwrap_or_default())
            .placeholder(if name == "from" { "From" } else { "To" })
            .label(label)
            .attr("inputmode", "numeric")
            .attr("maxlength", "4")
            .class("w-[4.5em]")
            .compact()
    }

    fn from(&self) -> kit::Input<'static> {
        self.year("from", self.b.from, "From year")
    }

    fn to(&self) -> kit::Input<'static> {
        self.year("to", self.b.to, "To year")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn browse(q: &str) -> Browse {
        askama_datastar::View::<Browse>::of(&format!("/albums?{q}")).0
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

    #[test]
    fn a_value_that_does_not_parse_costs_only_itself() {
        let b = browse("sort=nonsense&from=19x0&q=moss&fav=1");
        assert_eq!(b.sort, None);
        assert_eq!(b.from, None);
        assert_eq!(b.q, "moss");
        assert!(b.fav);
        assert_eq!(
            browse("sort=played").albums(0, 0).order,
            AlbumOrder::RecentlyAdded,
            "last played only while the recent filter is on"
        );
    }

    /// A shelf's heading opens its own listing: the same narrowing, the same
    /// order. The counts and rows matching follow from that; the pages' tests
    /// check them against a library.
    #[test]
    fn a_shelf_heading_links_to_its_shelfs_listing() {
        let (user, now) = (7, 1_800_000_000);
        for shelf in [Shelf::Favourites, Shelf::Recent, Shelf::Search("moss")] {
            let open = |kind: Kind| {
                let link = shelf_browser(shelf, kind);
                assert!(
                    link.starts_with(kind.route().trim_start_matches('/')),
                    "{link}"
                );
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

#[cfg(test)]
mod page_tests {
    use crate::ui::fixture::*;

    #[tokio::test]
    async fn sorting_and_filtering_live_in_the_query_string() {
        let f = setup(true);
        let page = |uri: &str| {
            let req = authed(&f.state, uri).body(Body::empty()).unwrap();
            let app = f.app.clone();
            async move { send(&app, req).await }
        };
        let r = page("/albums?sort=title&lossless=1&genre=").await;
        assert_eq!(r.status, StatusCode::OK);
        assert!(
            r.body.contains("Hymn &#60;to&#62; Moisture"),
            "FLAC is lossless"
        );
        assert!(r.body.contains("<option value=\"title\" selected>"));
        assert!(r.body.contains("name=\"lossless\" value=\"1\" checked"));
        assert!(r.body.contains("sort · filter · 1"), "{}", r.body);

        let r = page("/albums?codec=MP3").await;
        assert!(r.body.contains("No albums match."));
        let r = page("/albums?from=2020").await;
        assert!(r.body.contains("No albums match."), "released 2019");
        let r = page("/albums?from=2015&to=2019").await;
        assert!(r.body.contains("Hymn &#60;to&#62; Moisture"));

        // A value from a link, not among those offered, is still shown and kept.
        let r = page("/albums?genre=%22%3E%3Cscript%3E").await;
        assert!(
            r.body
                .contains("<option value=\"&#34;&#62;&#60;script&#62;\" selected>")
        );
        assert!(!r.body.contains("\"><script>"));

        for uri in [
            "/artists?sort=albums&fav=1",
            "/artists?sort=recent&lossless=1",
            "/albums?sort=random",
        ] {
            assert_eq!(page(uri).await.status, StatusCode::OK, "{uri}");
        }
        let r = page("/albums?sort=random").await;
        assert!(
            r.body
                .contains("<input type=\"hidden\" name=\"seed\" value=\""),
            "the shuffle is pinned"
        );
    }

    #[tokio::test]
    async fn a_shelf_heading_opens_the_browser_its_preview_is_the_head_of() {
        let f = setup(true);
        {
            let db = Database::open(&f.dir.path().join("koan.db")).unwrap();
            queries::add_favourite(&db.conn, 1, f.track_id).unwrap();
            for n in 2..=12 {
                let path = f.dir.path().join(format!("{n}.flac"));
                std::fs::write(&path, b"x").unwrap();
                let id = queries::upsert_track(&db.conn, &meta(&path, &format!("Track {n:02}"), n))
                    .unwrap();
                queries::add_favourite(&db.conn, 1, id).unwrap();
            }
        }
        let get = |uri: &str| authed(&f.state, uri).body(Body::empty()).unwrap();
        let shelf = send(&f.app, get("/favourites")).await.body;
        let preview = row_ids(&shelf);
        assert_eq!(preview.len(), 10, "ten in the preview");
        // The Tracks heading: a link, with the shelf's whole count in it.
        let heading = shelf
            .split("<h2>")
            .skip(1)
            .find(|h| h.contains(">Tracks<"))
            .expect("a Tracks heading");
        assert!(heading.contains(">12</span>"), "{heading}");
        let link = heading
            .split("href=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap()
            .replace("&#38;", "&");
        assert!(link.starts_with("tracks?"), "{link}");

        let browser = send(&f.app, get(&format!("/{link}"))).await.body;
        assert!(browser.contains("12 tracks"), "the count the heading gave");
        assert_eq!(
            row_ids(&browser)[..10],
            preview[..],
            "the same tracks, in the same order"
        );
        assert!(
            browser.contains("name=\"fav\" value=\"1\" checked"),
            "the filter shows, to be cleared"
        );
    }
}
