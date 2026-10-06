//! The library's shelves: favourites, recently played, what is downloaded and
//! a search. Each is
//! one narrowing, applied alike to the album, artist and track listings, so a
//! shelf is not a list of its own but the library's own listings with a filter
//! on. A shelf page shows the first few of each (`summary`), and its "See all"
//! opens the same listing with the same filter: the two read one query, so
//! they cannot disagree about what is on the shelf or in what order.
//!
//! What each shelf holds, how far back it reaches and how it is ordered is
//! decided here and nowhere else. Every front end asks for a shelf by name.
//!
//! Adding a shelf means a variant of [`Shelf`] and its
//! narrowing in each of [`Shelf::albums`], [`Shelf::artists`] and
//! [`Shelf::tracks`]; the summary and every listing follow from those.

use crate::db::connection::DbError;
use crate::db::queries::{
    self, AlbumFilter, AlbumOrder, AlbumQuery, AlbumRow, ArtistOrder, ArtistQuery, ArtistRow,
    PlayedSince, TrackFilter, TrackOrder, TrackRow,
};

/// How far back Recently played reaches.
pub const RECENT_DAYS: i64 = 30;

/// How many of each a shelf page shows before "See all": a cloud of artists,
/// about one row of records, a short list of tracks.
pub const PREVIEW_ARTISTS: u32 = 12;
pub const PREVIEW_ALBUMS: u32 = 8;
pub const PREVIEW_TRACKS: u32 = 10;

/// The clock the shelves read, in unix seconds.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shelf<'a> {
    /// What the account has favourited.
    Favourites,
    /// What the account played in the last `RECENT_DAYS`, the most recently
    /// played first, each once.
    Recent,
    /// What matches `q`: names for records and artists, the full-text index
    /// for tracks, the closest matches first. When nothing of a kind holds
    /// `q` as typed, that kind's closest fuzzy matches, so a typo still finds
    /// what was meant.
    Search(&'a str),
    /// What can play on this device, downloaded or in the library. Records
    /// count with any track here, fully there first, and each carries how
    /// much of it is.
    Downloaded,
}

impl<'a> Shelf<'a> {
    /// The shelf's records, for `user` at `now`. Page it with `limit` and
    /// `offset`; narrow it further with the album browser's own filters.
    pub fn albums(self, user: i64, now: i64) -> AlbumQuery<'a> {
        match self {
            Shelf::Favourites => AlbumQuery {
                favourites_of: Some(user),
                order: AlbumOrder::ArtistThenDate,
                ..Default::default()
            },
            Shelf::Recent => AlbumQuery {
                played: Some(recent(user, now)),
                order: AlbumOrder::LastPlayed,
                ..Default::default()
            },
            Shelf::Search(q) => AlbumQuery {
                search: Some(q),
                order: AlbumOrder::Relevance,
                ..Default::default()
            },
            Shelf::Downloaded => AlbumQuery {
                filter: AlbumFilter {
                    on_device: true,
                    ..Default::default()
                },
                order: AlbumOrder::Downloaded,
                ..Default::default()
            },
        }
    }

    /// The shelf's artists: album artists, as the artist browser lists them.
    pub fn artists(self, user: i64, now: i64) -> ArtistQuery<'a> {
        match self {
            Shelf::Favourites => ArtistQuery {
                favourites_of: Some(user),
                order: ArtistOrder::Name,
                ..Default::default()
            },
            Shelf::Recent => ArtistQuery {
                played: Some(recent(user, now)),
                order: ArtistOrder::LastPlayed,
                ..Default::default()
            },
            Shelf::Search(q) => ArtistQuery {
                search: Some(q),
                order: ArtistOrder::Relevance,
                ..Default::default()
            },
            Shelf::Downloaded => ArtistQuery {
                filter: AlbumFilter {
                    on_device: true,
                    ..Default::default()
                },
                order: ArtistOrder::Name,
                ..Default::default()
            },
        }
    }

    /// The shelf's tracks, and the order they read in (`descending` as
    /// `filter_tracks` takes it).
    pub fn tracks(self, user: i64, now: i64) -> Tracks {
        match self {
            Shelf::Favourites => Tracks {
                filter: TrackFilter {
                    favourites_of: Some(user),
                    ..Default::default()
                },
                order: TrackOrder::ArtistAlbumDiscTrack,
                descending: false,
            },
            Shelf::Recent => Tracks {
                filter: TrackFilter {
                    played: Some(recent(user, now)),
                    ..Default::default()
                },
                order: TrackOrder::LastPlayed,
                descending: true,
            },
            Shelf::Search(q) => Tracks {
                filter: TrackFilter {
                    search: Some(q.to_owned()),
                    ..Default::default()
                },
                order: TrackOrder::Relevance,
                descending: false,
            },
            Shelf::Downloaded => Tracks {
                filter: TrackFilter {
                    on_device: true,
                    ..Default::default()
                },
                order: TrackOrder::ArtistAlbumDiscTrack,
                descending: false,
            },
        }
    }
}

fn recent(user: i64, now: i64) -> PlayedSince {
    PlayedSince {
        user,
        since: now - RECENT_DAYS * 24 * 60 * 60,
    }
}

/// A shelf's tracks: the filter, and the order they read in.
#[derive(Debug, Clone)]
pub struct Tracks {
    pub filter: TrackFilter,
    pub order: TrackOrder,
    pub descending: bool,
}

impl Tracks {
    pub fn page(
        &self,
        conn: &rusqlite::Connection,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<TrackRow>, DbError> {
        queries::filter_tracks(
            conn,
            &self.filter,
            self.order,
            self.descending,
            limit,
            offset,
        )
    }

    pub fn count(&self, conn: &rusqlite::Connection) -> Result<u64, DbError> {
        queries::count_tracks(conn, &self.filter)
    }
}

/// The first few of one kind on a shelf, and how many there are in all.
#[derive(Debug, Clone)]
pub struct Section<T> {
    pub preview: Vec<T>,
    pub total: u64,
}

/// A shelf page: the first few artists, records and tracks, with their totals.
#[derive(Debug, Clone)]
pub struct Summary {
    pub artists: Section<ArtistRow>,
    pub albums: Section<AlbumRow>,
    pub tracks: Section<TrackRow>,
}

impl Summary {
    pub fn is_empty(&self) -> bool {
        self.artists.total == 0 && self.albums.total == 0 && self.tracks.total == 0
    }
}

/// What a shelf page shows: the head of each of the shelf's listings, narrowed
/// to what can play here when `on_device` (offline).
pub fn summary(
    conn: &rusqlite::Connection,
    shelf: Shelf,
    user: i64,
    now: i64,
    on_device: bool,
) -> Result<Summary, DbError> {
    let mut artists = shelf.artists(user, now);
    let mut albums = shelf.albums(user, now);
    let mut tracks = shelf.tracks(user, now);
    artists.filter.on_device |= on_device;
    albums.filter.on_device |= on_device;
    tracks.filter.on_device |= on_device;
    Ok(Summary {
        artists: Section {
            preview: queries::list_artists(
                conn,
                &ArtistQuery {
                    limit: Some(PREVIEW_ARTISTS),
                    without_track_counts: true,
                    ..artists
                },
            )?,
            total: queries::count_artists(conn, &artists)?,
        },
        albums: Section {
            preview: queries::list_albums(
                conn,
                &AlbumQuery {
                    limit: Some(PREVIEW_ALBUMS),
                    ..albums
                },
            )?,
            total: queries::count_albums(conn, &albums)?,
        },
        tracks: Section {
            preview: tracks.page(conn, PREVIEW_TRACKS, 0)?,
            total: tracks.count(conn)?,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::Database;
    use crate::db::queries::{
        add_favourite, record_play_at, sample_meta, set_favourite_album, set_favourite_artist,
        upsert_track,
    };

    const NOW: i64 = 1_800_000_000;
    const DAY: i64 = 24 * 60 * 60;

    fn db() -> Database {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        Database { conn }
    }

    /// Three records by three artists, two tracks each. Returns the tracks.
    fn library(db: &Database) -> Vec<TrackRow> {
        for (artist, album) in [("Ayla", "Moss"), ("Bryn", "Tide"), ("Cato", "Ember")] {
            for (n, title) in ["One", "Two"].iter().enumerate() {
                let mut m = sample_meta(&format!("{album} {title}"), artist, album);
                m.track_number = Some(n as i32 + 1);
                upsert_track(&db.conn, &m).unwrap();
            }
        }
        queries::filter_tracks(
            &db.conn,
            &TrackFilter::default(),
            TrackOrder::Title,
            false,
            100,
            0,
        )
        .unwrap()
    }

    fn by_title<'a>(tracks: &'a [TrackRow], title: &str) -> &'a TrackRow {
        tracks.iter().find(|t| t.title == title).unwrap()
    }

    #[test]
    fn recently_played_is_the_window_newest_first_each_once() {
        let db = db();
        let tracks = library(&db);
        let user = queries::LOCAL_USER;
        let play = |title: &str, ago: i64| {
            record_play_at(
                &db.conn,
                user,
                by_title(&tracks, title).id,
                NOW - ago,
                None,
                "t",
            )
            .unwrap();
        };
        play("Moss One", 3 * DAY);
        play("Tide Two", 2 * DAY);
        play("Moss Two", DAY);
        play("Moss One", 60);
        play("Ember One", 40 * DAY);

        let s = summary(&db.conn, Shelf::Recent, user, NOW, false).unwrap();
        let titles: Vec<_> = s.tracks.preview.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(
            titles,
            ["Moss One", "Moss Two", "Tide Two"],
            "newest play first, once each"
        );
        assert_eq!(
            s.tracks.total, 3,
            "the play from 40 days ago is outside the window"
        );
        let albums: Vec<_> = s.albums.preview.iter().map(|a| a.title.as_str()).collect();
        assert_eq!(albums, ["Moss", "Tide"]);
        let artists: Vec<_> = s.artists.preview.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(artists, ["Ayla", "Bryn"]);
        assert_eq!((s.albums.total, s.artists.total), (2, 2));
    }

    #[test]
    fn favourites_are_the_accounts_own() {
        let db = db();
        let tracks = library(&db);
        let user = queries::LOCAL_USER;
        let moss = by_title(&tracks, "Moss One");
        add_favourite(&db.conn, user, by_title(&tracks, "Tide Two").id).unwrap();
        add_favourite(&db.conn, user, moss.id).unwrap();
        set_favourite_album(&db.conn, user, moss.album_id.unwrap(), true).unwrap();
        set_favourite_artist(&db.conn, user, moss.artist_id.unwrap(), true).unwrap();

        let s = summary(&db.conn, Shelf::Favourites, user, NOW, false).unwrap();
        let titles: Vec<_> = s.tracks.preview.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, ["Moss One", "Tide Two"], "by artist, then record");
        assert_eq!(s.albums.preview.len(), 1);
        assert_eq!(s.artists.preview[0].name, "Ayla");
        assert_eq!((s.tracks.total, s.albums.total, s.artists.total), (2, 1, 1));
    }

    #[test]
    fn a_search_shelf_narrows_each_listing_by_the_query() {
        let db = db();
        library(&db);
        let s = summary(
            &db.conn,
            Shelf::Search("tide"),
            queries::LOCAL_USER,
            NOW,
            false,
        )
        .unwrap();
        assert_eq!(s.albums.preview[0].title, "Tide");
        assert_eq!(s.tracks.total, 2);
        assert!(s.artists.preview.is_empty(), "no artist is called tide");
    }

    #[test]
    fn downloaded_is_what_can_play_here_fully_there_first() {
        let db = db();
        library(&db);
        db.conn
            .execute("UPDATE tracks SET path = NULL", [])
            .unwrap();
        db.conn
            .execute(
                "UPDATE tracks SET cached_path = '/cache/' || title
                  WHERE title IN ('Moss One', 'Tide One', 'Tide Two')",
                [],
            )
            .unwrap();
        let s = summary(&db.conn, Shelf::Downloaded, queries::LOCAL_USER, NOW, false).unwrap();
        let albums: Vec<_> = s
            .albums
            .preview
            .iter()
            .map(|a| (a.title.as_str(), a.on_device.map(|d| (d.have, d.total))))
            .collect();
        assert_eq!(albums, [("Tide", Some((2, 2))), ("Moss", Some((1, 2)))]);
        let artists: Vec<_> = s.artists.preview.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(artists, ["Ayla", "Bryn"]);
        assert_eq!(s.tracks.total, 3);
    }

    #[test]
    fn a_preview_is_the_head_of_the_listing_its_total_counts() {
        let db = db();
        let tracks = library(&db);
        let user = queries::LOCAL_USER;
        for (i, t) in tracks.iter().enumerate() {
            add_favourite(&db.conn, user, t.id).unwrap();
            record_play_at(&db.conn, user, t.id, NOW - i as i64 * 60, None, "t").unwrap();
        }
        for shelf in [Shelf::Favourites, Shelf::Recent, Shelf::Downloaded] {
            let t = shelf.tracks(user, NOW);
            let all = t.page(&db.conn, 1000, 0).unwrap();
            let s = summary(&db.conn, shelf, user, NOW, false).unwrap();
            assert_eq!(s.tracks.total, all.len() as u64, "{shelf:?}");
            assert_eq!(
                s.tracks.preview.iter().map(|t| t.id).collect::<Vec<_>>(),
                all.iter()
                    .take(PREVIEW_TRACKS as usize)
                    .map(|t| t.id)
                    .collect::<Vec<_>>(),
                "{shelf:?}: the preview is the listing's head"
            );
            let albums = queries::list_albums(&db.conn, &shelf.albums(user, NOW)).unwrap();
            assert_eq!(s.albums.total, albums.len() as u64);
            assert_eq!(
                s.albums.preview.iter().map(|a| a.id).collect::<Vec<_>>(),
                albums
                    .iter()
                    .take(PREVIEW_ALBUMS as usize)
                    .map(|a| a.id)
                    .collect::<Vec<_>>()
            );
        }
    }

    /// Artists whose names hold "krew" at the start, at a later word, inside a
    /// word, and others that hold only its letters, scattered.
    fn krew_library(db: &Database) {
        for (artist, album) in [
            ("Okrewa", "Inside"),
            ("The Krew", "Word"),
            ("Krewella", "Get Wet"),
            ("Kreayshawn", "Somethin' 'Bout Kreay"),
            ("Kryptic Minds", "One of Us"),
            ("Fckn Crew", "Crew Cuts"),
            ("Black Country, New Road", "Ants From Up There"),
            ("DJ Brisk", "Rewind"),
        ] {
            for title in ["Alive", "Live for the Night"] {
                upsert_track(
                    &db.conn,
                    &sample_meta(&format!("{album} {title}"), artist, album),
                )
                .unwrap();
            }
        }
    }

    /// Every kind's count is its whole listing's length, and its preview the
    /// listing's head.
    fn assert_agrees(db: &Database, q: &str) -> Summary {
        let user = queries::LOCAL_USER;
        let shelf = Shelf::Search(q);
        let s = summary(&db.conn, shelf, user, NOW, false).unwrap();
        let ids = |v: Vec<i64>, n: u32| v.into_iter().take(n as usize).collect::<Vec<_>>();

        let artists = queries::list_artists(&db.conn, &shelf.artists(user, NOW)).unwrap();
        assert_eq!(s.artists.total, artists.len() as u64, "{q}: artists");
        assert_eq!(
            s.artists.preview.iter().map(|a| a.id).collect::<Vec<_>>(),
            ids(artists.iter().map(|a| a.id).collect(), PREVIEW_ARTISTS),
        );
        let albums = queries::list_albums(&db.conn, &shelf.albums(user, NOW)).unwrap();
        assert_eq!(s.albums.total, albums.len() as u64, "{q}: albums");
        assert_eq!(
            s.albums.preview.iter().map(|a| a.id).collect::<Vec<_>>(),
            ids(albums.iter().map(|a| a.id).collect(), PREVIEW_ALBUMS),
        );
        let tracks = shelf.tracks(user, NOW).page(&db.conn, 1000, 0).unwrap();
        assert_eq!(s.tracks.total, tracks.len() as u64, "{q}: tracks");
        assert_eq!(
            s.tracks.preview.iter().map(|t| t.id).collect::<Vec<_>>(),
            ids(tracks.iter().map(|t| t.id).collect(), PREVIEW_TRACKS),
        );
        s
    }

    #[test]
    fn a_search_lists_the_closest_matches_first_and_no_scattered_ones() {
        let db = db();
        krew_library(&db);
        let s = assert_agrees(&db, "Krew");
        let artists: Vec<_> = s.artists.preview.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(
            artists,
            ["Krewella", "The Krew", "Okrewa"],
            "the name's start, then a later word's, then anywhere; nothing scattered"
        );
        let albums: Vec<_> = s.albums.preview.iter().map(|a| a.title.as_str()).collect();
        assert_eq!(albums, ["Get Wet", "Word", "Inside"]);
        assert_eq!(s.tracks.preview[0].artist_name, "Krewella");
        assert!(
            s.tracks
                .preview
                .iter()
                .all(|t| t.artist_name != "Black Country, New Road")
        );
    }

    #[test]
    fn a_typo_falls_back_to_the_closest_fuzzy_matches() {
        let db = db();
        krew_library(&db);
        for typo in ["Krewela", "krwella"] {
            let s = assert_agrees(&db, typo);
            let artists: Vec<_> = s.artists.preview.iter().map(|a| a.name.as_str()).collect();
            assert_eq!(artists[0], "Krewella", "{typo}");
            assert!(
                !artists.contains(&"Black Country, New Road"),
                "{typo}: {artists:?}"
            );
            assert_eq!(s.albums.preview[0].title, "Get Wet", "{typo}");
            assert!(s.tracks.total > 0, "{typo}");
            assert!(
                s.tracks.preview.iter().all(|t| t.artist_name == "Krewella"),
                "{typo}"
            );
        }
    }

    #[test]
    fn a_search_reads_nucleos_operators_as_characters() {
        let db = db();
        krew_library(&db);
        for q in ["!!!", "^x"] {
            let s = assert_agrees(&db, q);
            assert!(s.is_empty(), "{q}: nothing holds these characters");
        }
    }

    #[test]
    fn a_fallback_sees_what_was_written_since_the_last_search() {
        let db = db();
        krew_library(&db);
        let before = summary(
            &db.conn,
            Shelf::Search("Polr Bear"),
            queries::LOCAL_USER,
            NOW,
            false,
        )
        .unwrap();
        assert!(before.artists.preview.is_empty());
        upsert_track(
            &db.conn,
            &sample_meta("Fluffy", "Polar Bear", "Held On The Tips Of Fingers"),
        )
        .unwrap();
        let s = assert_agrees(&db, "Polr Bear");
        assert_eq!(s.artists.preview[0].name, "Polar Bear");
    }
}
