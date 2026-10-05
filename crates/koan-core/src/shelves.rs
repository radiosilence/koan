//! The library's shelves: favourites, recently played and a search. Each is
//! one narrowing, applied alike to the album, artist and track listings, so a
//! shelf is not a list of its own but the library's own listings with a filter
//! on. A shelf page shows the first few of each (`summary`), and its "See all"
//! opens the same listing with the same filter: the two read one query, so
//! they cannot disagree about what is on the shelf or in what order.
//!
//! What each shelf holds, how far back it reaches and how it is ordered is
//! decided here and nowhere else. Every front end asks for a shelf by name.
//!
//! Adding a shelf (Downloaded is next) means a variant of [`Shelf`] and its
//! narrowing in each of [`Shelf::albums`], [`Shelf::artists`] and
//! [`Shelf::tracks`]; the summary and every listing follow from those.

use crate::db::connection::DbError;
use crate::db::queries::{
    self, AlbumOrder, AlbumQuery, AlbumRow, ArtistOrder, ArtistQuery, ArtistRow, PlayedSince,
    TrackFilter, TrackOrder, TrackRow,
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
    /// for tracks.
    Search(&'a str),
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
                order: AlbumOrder::RecentlyAdded,
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

/// What a shelf page shows: the head of each of the shelf's listings.
pub fn summary(
    conn: &rusqlite::Connection,
    shelf: Shelf,
    user: i64,
    now: i64,
) -> Result<Summary, DbError> {
    let artists = shelf.artists(user, now);
    let albums = shelf.albums(user, now);
    let tracks = shelf.tracks(user, now);
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

        let s = summary(&db.conn, Shelf::Recent, user, NOW).unwrap();
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

        let s = summary(&db.conn, Shelf::Favourites, user, NOW).unwrap();
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
        let s = summary(&db.conn, Shelf::Search("tide"), queries::LOCAL_USER, NOW).unwrap();
        assert_eq!(s.albums.preview[0].title, "Tide");
        assert_eq!(s.tracks.total, 2);
        assert!(s.artists.preview.is_empty(), "no artist is called tide");
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
        for shelf in [Shelf::Favourites, Shelf::Recent] {
            let t = shelf.tracks(user, NOW);
            let all = t.page(&db.conn, 1000, 0).unwrap();
            let s = summary(&db.conn, shelf, user, NOW).unwrap();
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
}
