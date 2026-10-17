//! The library browsers: the album, artist and track listings, narrowed by the
//! browse filters and the name filter and put in the order chosen. Every front
//! end offers the same filters and sorts, so the queries they make are built
//! here, once.
//!
//! A shelf's "See all" opens a browser set up as the shelf ([`of_shelf`]), so
//! the listing it opens on is the one the shelf's preview is the head of.

use crate::db::queries::{
    AlbumFilter, AlbumOrder, AlbumQuery, ArtistOrder, ArtistQuery, LOCAL_USER, PlayedSince,
    TrackFilter, TrackOrder,
};
use crate::shelves::{self, Shelf, Tracks};

/// What the browsers are narrowed to beyond the name filter. An artist passes
/// when any of their albums does.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BrowseFilter {
    pub favourites: bool,
    /// Only what was played in the last `shelves::RECENT_DAYS`: the Recently
    /// Played shelf as a filter.
    pub recent: bool,
    /// Only what can play on this device: the Downloaded shelf as a filter.
    pub downloaded: bool,
    /// Only albums in a lossless codec. Tracks are narrowed by codec instead.
    pub lossless: bool,
    pub codec: Option<String>,
    pub genre: Option<String>,
    /// Release year bounds, inclusive. Albums without a date are left out when
    /// either is set.
    pub year_from: Option<i32>,
    pub year_to: Option<i32>,
}

impl BrowseFilter {
    /// How many filters are on, as the control showing them counts them.
    pub fn active_count(&self) -> usize {
        [
            self.favourites,
            self.recent,
            self.downloaded,
            self.lossless,
            self.codec.is_some(),
            self.genre.is_some(),
            self.year_from.is_some() || self.year_to.is_some(),
        ]
        .into_iter()
        .filter(|on| *on)
        .count()
    }

    fn played(&self) -> Option<PlayedSince> {
        self.recent
            .then(|| Shelf::Recent.albums(LOCAL_USER, shelves::now()).played)
            .flatten()
    }

    fn albums(&self) -> AlbumFilter<'_> {
        AlbumFilter {
            lossless: self.lossless,
            codec: trimmed(self.codec.as_deref()),
            year_from: self.year_from,
            year_to: self.year_to,
            genre: trimmed(self.genre.as_deref()),
            on_device: self.downloaded || crate::remote::offline::active(),
        }
    }
}

/// The album browser's orders.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AlbumSort {
    /// Newest first: the album you just added is the one you are looking for.
    #[default]
    RecentlyAdded,
    Title,
    Artist,
    Year,
    /// Shuffled by a seed held beside the sort, so narrowing a shuffle does
    /// not deal it again.
    Random,
    /// Only offered with the Recently Played filter, which knows when.
    LastPlayed,
    /// Fully on this device first: only offered with the Downloaded filter.
    Downloaded,
    /// The closest match to the name filter first: only offered with one.
    BestMatch,
}

impl AlbumSort {
    /// The sorts that mean something for `filter`, and with a name filter
    /// when `searching`.
    pub fn offered(filter: &BrowseFilter, searching: bool) -> Vec<AlbumSort> {
        let mut sorts = Vec::new();
        if searching {
            sorts.push(AlbumSort::BestMatch);
        }
        if filter.recent {
            sorts.push(AlbumSort::LastPlayed);
        }
        if filter.downloaded {
            sorts.push(AlbumSort::Downloaded);
        }
        sorts.extend([
            AlbumSort::RecentlyAdded,
            AlbumSort::Title,
            AlbumSort::Artist,
            AlbumSort::Year,
            AlbumSort::Random,
        ]);
        sorts
    }

    fn order(self, seed: i64) -> AlbumOrder {
        match self {
            AlbumSort::RecentlyAdded => AlbumOrder::RecentlyAdded,
            AlbumSort::Title => AlbumOrder::Title,
            AlbumSort::Artist => AlbumOrder::ArtistThenDate,
            AlbumSort::Year => AlbumOrder::YearDesc,
            AlbumSort::Random => AlbumOrder::Random(seed),
            AlbumSort::LastPlayed => AlbumOrder::LastPlayed,
            AlbumSort::Downloaded => AlbumOrder::Downloaded,
            AlbumSort::BestMatch => AlbumOrder::Relevance,
        }
    }
}

/// The track browser's orders.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TrackSort {
    /// Artist, then album, disc and track: the library as it would be shelved.
    #[default]
    Artist,
    Title,
    /// By album title, then in running order.
    Album,
    Duration,
    /// Most recently played first: only offered with the Recently Played filter.
    LastPlayed,
    /// The closest match to the name filter first: only offered with one.
    BestMatch,
}

impl TrackSort {
    /// As [`AlbumSort::offered`].
    pub fn offered(filter: &BrowseFilter, searching: bool) -> Vec<TrackSort> {
        let mut sorts = Vec::new();
        if searching {
            sorts.push(TrackSort::BestMatch);
        }
        if filter.recent {
            sorts.push(TrackSort::LastPlayed);
        }
        sorts.extend([
            TrackSort::Artist,
            TrackSort::Title,
            TrackSort::Album,
            TrackSort::Duration,
        ]);
        sorts
    }

    /// The order, and whether it runs descending as `filter_tracks` takes it.
    fn order(self) -> (TrackOrder, bool) {
        match self {
            TrackSort::Artist => (TrackOrder::ArtistAlbumDiscTrack, false),
            TrackSort::Title => (TrackOrder::Title, false),
            TrackSort::Album => (TrackOrder::Album, false),
            TrackSort::Duration => (TrackOrder::Duration, false),
            TrackSort::LastPlayed => (TrackOrder::LastPlayed, true),
            TrackSort::BestMatch => (TrackOrder::Relevance, false),
        }
    }
}

/// A name filter someone typed, or nothing: whitespace is not a filter.
pub fn trimmed(search: Option<&str>) -> Option<&str> {
    search.map(str::trim).filter(|s| !s.is_empty())
}

/// The album browser's listing, or one artist's albums with `artist`.
/// `seed` fixes the shuffle under [`AlbumSort::Random`].
pub fn albums<'a>(
    filter: &'a BrowseFilter,
    search: Option<&'a str>,
    sort: AlbumSort,
    seed: i64,
) -> AlbumQuery<'a> {
    AlbumQuery {
        search: trimmed(search),
        order: sort.order(seed),
        favourites_of: filter.favourites.then_some(LOCAL_USER),
        played: filter.played(),
        filter: filter.albums(),
        ..Default::default()
    }
}

/// The artist browser's listing. It has no sort of its own: Recently Played's
/// order with that filter, else the closest match to the name filter, and by
/// name without one.
pub fn artists<'a>(filter: &'a BrowseFilter, search: Option<&'a str>) -> ArtistQuery<'a> {
    let played = filter.played();
    ArtistQuery {
        search: trimmed(search),
        favourites_of: filter.favourites.then_some(LOCAL_USER),
        order: if played.is_some() {
            ArtistOrder::LastPlayed
        } else {
            ArtistOrder::Relevance
        },
        played,
        filter: filter.albums(),
        ..Default::default()
    }
}

/// The track browser's listing. `filter.lossless` does not apply to tracks.
pub fn tracks(filter: &BrowseFilter, search: Option<&str>, sort: TrackSort) -> Tracks {
    let (order, descending) = sort.order();
    let (user, now) = (LOCAL_USER, shelves::now());
    Tracks {
        filter: TrackFilter {
            search: trimmed(search).and_then(|q| Shelf::Search(q).tracks(user, now).filter.search),
            favourites_of: filter.favourites.then_some(user),
            played: filter.played(),
            codec: trimmed(filter.codec.as_deref()).map(str::to_owned),
            genre: trimmed(filter.genre.as_deref()).map(str::to_owned),
            year_start: filter.year_from,
            year_end: filter.year_to,
            on_device: filter.downloaded || crate::remote::offline::active(),
            ..Default::default()
        },
        order,
        descending,
    }
}

/// A browser set up as `shelf`, for its "See all": the filter, and the album
/// and track sorts that read as the shelf does. A search's query becomes the
/// name filter, which the caller carries.
pub fn of_shelf(shelf: Shelf) -> (BrowseFilter, AlbumSort, TrackSort) {
    match shelf {
        Shelf::Favourites => (
            BrowseFilter {
                favourites: true,
                ..Default::default()
            },
            AlbumSort::Artist,
            TrackSort::Artist,
        ),
        Shelf::Recent => (
            BrowseFilter {
                recent: true,
                ..Default::default()
            },
            AlbumSort::LastPlayed,
            TrackSort::LastPlayed,
        ),
        Shelf::Search(_) => (
            BrowseFilter::default(),
            AlbumSort::BestMatch,
            TrackSort::BestMatch,
        ),
        Shelf::Downloaded => (
            BrowseFilter {
                downloaded: true,
                ..Default::default()
            },
            AlbumSort::Downloaded,
            TrackSort::Artist,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorts_are_offered_where_they_mean_something() {
        let none = BrowseFilter::default();
        assert_eq!(
            AlbumSort::offered(&none, false)[0],
            AlbumSort::RecentlyAdded
        );
        assert!(!AlbumSort::offered(&none, false).contains(&AlbumSort::LastPlayed));
        let recent = BrowseFilter {
            recent: true,
            downloaded: true,
            ..Default::default()
        };
        assert_eq!(
            AlbumSort::offered(&recent, true)[..3],
            [
                AlbumSort::BestMatch,
                AlbumSort::LastPlayed,
                AlbumSort::Downloaded
            ]
        );
        assert_eq!(TrackSort::offered(&recent, false)[0], TrackSort::LastPlayed);
    }

    #[test]
    fn a_year_range_counts_once() {
        let f = BrowseFilter {
            year_from: Some(1990),
            year_to: Some(1999),
            lossless: true,
            ..Default::default()
        };
        assert_eq!(f.active_count(), 2);
        assert_eq!(BrowseFilter::default().active_count(), 0);
    }

    #[test]
    fn a_blank_name_filter_is_no_filter() {
        let f = BrowseFilter::default();
        assert_eq!(albums(&f, Some("  "), AlbumSort::Title, 0).search, None);
        assert_eq!(artists(&f, Some(" x ")).search, Some("x"));
    }
}
