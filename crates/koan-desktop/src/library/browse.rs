//! What the browsers are narrowed and ordered by. The filter and the search
//! are shared by the album, artist and track browsers, as the Mac's are; each
//! listing keeps its own sort. Kept in `desktop-ui.json` beside the
//! configuration: these are one front end's preferences, not engine settings,
//! so they stay out of `config.toml`.

use std::path::PathBuf;

use gpui_kit::*;
use koan_ffi::{AlbumSort, BrowseFilter, TrackBrowseSort};
use serde::{Deserialize, Serialize};

pub struct Browse {
    pub filter: BrowseFilter,
    pub search: String,
    pub album_sort: AlbumSort,
    pub track_sort: TrackBrowseSort,
    /// Fixes the order under `AlbumSort::Random`, so narrowing a shuffled
    /// listing does not deal it again. A reshuffle is a new seed.
    pub seed: i64,
}

impl Browse {
    pub fn load() -> Self {
        let stored: Stored = std::fs::read(path())
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            filter: BrowseFilter {
                favourites: stored.favourites,
                recent: stored.recent,
                downloaded: stored.downloaded,
                lossless: stored.lossless,
                codec: stored.codec,
                year_from: stored.year_from,
                year_to: stored.year_to,
                genre: stored.genre,
            },
            search: String::new(),
            album_sort: stored
                .album_sort
                .as_deref()
                .and_then(album_sort)
                .unwrap_or(AlbumSort::RecentlyAdded),
            track_sort: stored
                .track_sort
                .as_deref()
                .and_then(track_sort)
                .unwrap_or(TrackBrowseSort::Artist),
            seed: seed(),
        }
    }

    /// The album sort in force: one only offered alongside a filter or a
    /// search falls back when that is gone, as `AlbumSort::offered` says.
    pub fn effective_album_sort(&self) -> AlbumSort {
        let offered = album_sorts(
            self.filter.recent,
            self.filter.downloaded,
            !self.search.is_empty(),
        );
        if offered.contains(&self.album_sort) {
            self.album_sort
        } else {
            AlbumSort::RecentlyAdded
        }
    }

    pub fn effective_track_sort(&self) -> TrackBrowseSort {
        if track_sorts(self.filter.recent, !self.search.is_empty()).contains(&self.track_sort) {
            self.track_sort
        } else {
            TrackBrowseSort::Artist
        }
    }

    pub fn search(&self) -> Option<String> {
        let trimmed = self.search.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    }

    /// How many filters are on, the year range counting as one: what the
    /// filter control shows, because a narrowed listing otherwise looks like
    /// a missing library.
    pub fn active_filters(&self) -> usize {
        let f = &self.filter;
        [
            f.favourites,
            f.recent,
            f.downloaded,
            f.lossless,
            f.codec.is_some(),
            f.genre.is_some(),
            f.year_from.is_some() || f.year_to.is_some(),
        ]
        .into_iter()
        .filter(|on| *on)
        .count()
    }

    pub fn is_narrowed(&self) -> bool {
        self.active_filters() > 0 || self.search().is_some()
    }

    pub fn update(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut Self)) {
        change(self);
        self.save(cx);
        cx.notify();
    }

    pub fn reshuffle(&mut self, cx: &mut Context<Self>) {
        self.seed = seed();
        cx.notify();
    }

    fn save(&self, cx: &mut Context<Self>) {
        let f = &self.filter;
        let stored = Stored {
            favourites: f.favourites,
            recent: f.recent,
            downloaded: f.downloaded,
            lossless: f.lossless,
            codec: f.codec.clone(),
            year_from: f.year_from,
            year_to: f.year_to,
            genre: f.genre.clone(),
            album_sort: Some(album_sort_key(self.album_sort).into()),
            track_sort: Some(track_sort_key(self.track_sort).into()),
        };
        cx.background_executor()
            .spawn(async move {
                let Ok(bytes) = serde_json::to_vec_pretty(&stored) else {
                    return;
                };
                let path = path();
                let tmp = path.with_extension("json.tmp");
                if std::fs::write(&tmp, bytes).is_ok() && std::fs::rename(&tmp, &path).is_err() {
                    std::fs::remove_file(&tmp).ok();
                }
            })
            .detach();
    }
}

fn path() -> PathBuf {
    koan_ffi::config_dir().join("desktop-ui.json")
}

fn seed() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as i64)
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Stored {
    favourites: bool,
    recent: bool,
    downloaded: bool,
    lossless: bool,
    codec: Option<String>,
    year_from: Option<i32>,
    year_to: Option<i32>,
    genre: Option<String>,
    album_sort: Option<String>,
    track_sort: Option<String>,
}

/// The sorts to offer. Last Played only means something while the Recently
/// Played filter is on, Most Downloaded while the Downloaded one is, and Best
/// Match while a search narrows the listing.
pub fn album_sorts(recent: bool, downloaded: bool, searching: bool) -> Vec<AlbumSort> {
    let mut out = Vec::new();
    if searching {
        out.push(AlbumSort::BestMatch);
    }
    if recent {
        out.push(AlbumSort::LastPlayed);
    }
    if downloaded {
        out.push(AlbumSort::Downloaded);
    }
    out.extend([
        AlbumSort::RecentlyAdded,
        AlbumSort::Title,
        AlbumSort::Artist,
        AlbumSort::Year,
        AlbumSort::YearAsc,
        AlbumSort::Random,
    ]);
    out
}

pub fn track_sorts(recent: bool, searching: bool) -> Vec<TrackBrowseSort> {
    let mut out = Vec::new();
    if searching {
        out.push(TrackBrowseSort::BestMatch);
    }
    if recent {
        out.push(TrackBrowseSort::LastPlayed);
    }
    out.extend([
        TrackBrowseSort::Artist,
        TrackBrowseSort::Title,
        TrackBrowseSort::Album,
        TrackBrowseSort::Duration,
    ]);
    out
}

pub fn album_sort_label(sort: AlbumSort) -> &'static str {
    match sort {
        AlbumSort::RecentlyAdded => "recently added",
        AlbumSort::Title => "title",
        AlbumSort::Artist => "artist",
        AlbumSort::Year => "year, newest",
        AlbumSort::YearAsc => "year, oldest",
        AlbumSort::Random => "random",
        AlbumSort::LastPlayed => "last played",
        AlbumSort::Downloaded => "most downloaded",
        AlbumSort::BestMatch => "best match",
    }
}

pub fn track_sort_label(sort: TrackBrowseSort) -> &'static str {
    match sort {
        TrackBrowseSort::Artist => "artist",
        TrackBrowseSort::Title => "title",
        TrackBrowseSort::Album => "album",
        TrackBrowseSort::Duration => "duration",
        TrackBrowseSort::LastPlayed => "last played",
        TrackBrowseSort::BestMatch => "best match",
    }
}

/// Stable names for the stored file, so a preference survives the enum
/// gaining or reordering cases.
fn album_sort_key(sort: AlbumSort) -> &'static str {
    match sort {
        AlbumSort::RecentlyAdded => "recentlyAdded",
        AlbumSort::Title => "title",
        AlbumSort::Artist => "artist",
        AlbumSort::Year => "year",
        AlbumSort::YearAsc => "yearAsc",
        AlbumSort::Random => "random",
        AlbumSort::LastPlayed => "lastPlayed",
        AlbumSort::Downloaded => "downloaded",
        AlbumSort::BestMatch => "bestMatch",
    }
}

fn album_sort(key: &str) -> Option<AlbumSort> {
    album_sorts(true, true, true)
        .into_iter()
        .find(|s| album_sort_key(*s) == key)
}

fn track_sort_key(sort: TrackBrowseSort) -> &'static str {
    match sort {
        TrackBrowseSort::Artist => "artist",
        TrackBrowseSort::Title => "title",
        TrackBrowseSort::Album => "album",
        TrackBrowseSort::Duration => "duration",
        TrackBrowseSort::LastPlayed => "lastPlayed",
        TrackBrowseSort::BestMatch => "bestMatch",
    }
}

fn track_sort(key: &str) -> Option<TrackBrowseSort> {
    track_sorts(true, true)
        .into_iter()
        .find(|s| track_sort_key(*s) == key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sort_reads_back_from_its_stored_name() {
        for sort in album_sorts(true, true, true) {
            assert_eq!(album_sort(album_sort_key(sort)), Some(sort));
        }
        for sort in track_sorts(true, true) {
            assert_eq!(track_sort(track_sort_key(sort)), Some(sort));
        }
        assert_eq!(album_sort("nonsense"), None);
    }

    #[test]
    fn a_sort_offered_only_with_a_filter_falls_back_without_it() {
        let mut browse = Browse {
            filter: BrowseFilter::default(),
            search: String::new(),
            album_sort: AlbumSort::LastPlayed,
            track_sort: TrackBrowseSort::BestMatch,
            seed: 0,
        };
        assert_eq!(browse.effective_album_sort(), AlbumSort::RecentlyAdded);
        assert_eq!(browse.effective_track_sort(), TrackBrowseSort::Artist);
        browse.filter.recent = true;
        browse.search = "x".into();
        assert_eq!(browse.effective_album_sort(), AlbumSort::LastPlayed);
        assert_eq!(browse.effective_track_sort(), TrackBrowseSort::BestMatch);
    }
}
