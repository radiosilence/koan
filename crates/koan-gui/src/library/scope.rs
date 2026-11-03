//! What a shelf's "see all" narrows a browser by: the shelf's own filter and
//! order, which the browser reads in place of the shared ones, so opening a
//! shelf's listing leaves the library browser's filter as it was.

use koan_kit::app::*;
use koan_runtime::{AlbumSort, BrowseFilter, TrackBrowseSort};

use super::Lib;
use crate::navigator::{Listing, Shelf};

pub fn filter(shelf: Shelf) -> BrowseFilter {
    let mut filter = BrowseFilter::default();
    match shelf {
        Shelf::Favourites => filter.favourites = true,
        Shelf::Recent => filter.recent = true,
        Shelf::Downloaded => filter.downloaded = true,
        Shelf::Search => {}
    }
    filter
}

pub fn album_sort(shelf: Shelf) -> AlbumSort {
    match shelf {
        Shelf::Favourites => AlbumSort::Artist,
        Shelf::Recent => AlbumSort::LastPlayed,
        Shelf::Downloaded => AlbumSort::Downloaded,
        Shelf::Search => AlbumSort::BestMatch,
    }
}

pub fn track_sort(shelf: Shelf) -> TrackBrowseSort {
    match shelf {
        Shelf::Favourites | Shelf::Downloaded => TrackBrowseSort::Artist,
        Shelf::Recent => TrackBrowseSort::LastPlayed,
        Shelf::Search => TrackBrowseSort::BestMatch,
    }
}

/// The search a shelf's listing is narrowed by: the sidebar's query, for the
/// search's own shelf.
pub fn search(shelf: Shelf, lib: &Lib, cx: &App) -> Option<String> {
    match shelf {
        Shelf::Search => {
            let query = lib.search.read(cx).query().trim().to_string();
            (!query.is_empty()).then_some(query)
        }
        _ => None,
    }
}

/// The page's title: what the shelf's listing is.
pub fn title(shelf: Shelf, listing: Listing) -> &'static str {
    match (shelf, listing) {
        (Shelf::Favourites, Listing::Albums) => "favourite albums",
        (Shelf::Favourites, Listing::Artists) => "favourite artists",
        (Shelf::Favourites, Listing::Tracks) => "favourite tracks",
        (Shelf::Recent, Listing::Albums) => "albums played lately",
        (Shelf::Recent, Listing::Artists) => "artists played lately",
        (Shelf::Recent, Listing::Tracks) => "tracks played lately",
        (Shelf::Downloaded, Listing::Albums) => "downloaded albums",
        (Shelf::Downloaded, Listing::Artists) => "downloaded artists",
        (Shelf::Downloaded, Listing::Tracks) => "downloaded tracks",
        (Shelf::Search, Listing::Albums) => "albums found",
        (Shelf::Search, Listing::Artists) => "artists found",
        (Shelf::Search, Listing::Tracks) => "tracks found",
    }
}
