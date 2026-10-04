//! Batched loaders for the parent → child edges of the schema.
//!
//! Without these, a field like `Album.trackCount` runs one query per album and
//! `Track.isFavourite` runs a full `favourites` scan per track. Each loader
//! collapses a whole selection set's worth of keys into one statement.

use std::collections::HashMap;

use async_graphql::dataloader::Loader;
use koan_core::db::queries::batch::{AlbumStats, ArtistStats};
use koan_core::db::queries::{self, AlbumRow, TrackRow, UidKind};

use super::{DbHandle, blocking, internal_error};

macro_rules! id_key {
    ($($name:ident),* $(,)?) => {
        $(
            #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
            pub(super) struct $name(pub i64);
        )*
    };
}

id_key!(
    ArtistAlbums,
    ArtistTracks,
    ArtistStatsOf,
    AlbumTracks,
    AlbumStatsOf
);

/// A row's uid: what every id field publishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct UidOf(pub UidKind, pub i64);

/// The row a uid names.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct RowOf(pub UidKind, pub String);

/// Favourite lookup keyed by whose favourites and the track.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct FavouriteTrack(pub i64, pub i64);

pub(super) struct DbLoader {
    handle: DbHandle,
}

impl DbLoader {
    pub(super) fn new(handle: DbHandle) -> Self {
        Self { handle }
    }

    /// Every loader body is the same shape: take the keys, run one blocking
    /// query with a pooled connection, hand back a keyed map.
    async fn batch<K, V, F>(&self, keys: &[K], f: F) -> Result<HashMap<K, V>, async_graphql::Error>
    where
        K: Clone + Send + 'static,
        V: Send + 'static,
        F: FnOnce(
                &koan_core::db::connection::Database,
                Vec<K>,
            ) -> async_graphql::Result<HashMap<K, V>>
            + Send
            + 'static,
        HashMap<K, V>: Send,
    {
        let handle = self.handle.clone();
        handle.note_batch();
        let keys = keys.to_vec();
        blocking(move || {
            let db = handle.acquire()?;
            f(&db, keys)
        })
        .await
    }
}

impl Loader<ArtistAlbums> for DbLoader {
    type Value = Vec<AlbumRow>;
    type Error = async_graphql::Error;

    async fn load(
        &self,
        keys: &[ArtistAlbums],
    ) -> Result<HashMap<ArtistAlbums, Self::Value>, Self::Error> {
        self.batch(keys, |db, keys| {
            let ids: Vec<i64> = keys.iter().map(|k| k.0).collect();
            let mut by_artist = queries::batch::albums_for_artists(&db.conn, &ids)
                .map_err(|e| internal_error("db", e))?;
            Ok(keys
                .into_iter()
                .map(|k| {
                    let albums = by_artist.remove(&k.0).unwrap_or_default();
                    (k, albums)
                })
                .collect())
        })
        .await
    }
}

impl Loader<ArtistTracks> for DbLoader {
    type Value = Vec<TrackRow>;
    type Error = async_graphql::Error;

    async fn load(
        &self,
        keys: &[ArtistTracks],
    ) -> Result<HashMap<ArtistTracks, Self::Value>, Self::Error> {
        self.batch(keys, |db, keys| {
            let ids: Vec<i64> = keys.iter().map(|k| k.0).collect();
            let mut by_artist = queries::batch::tracks_for_artists(&db.conn, &ids)
                .map_err(|e| internal_error("db", e))?;
            Ok(keys
                .into_iter()
                .map(|k| {
                    let tracks = by_artist.remove(&k.0).unwrap_or_default();
                    (k, tracks)
                })
                .collect())
        })
        .await
    }
}

impl Loader<ArtistStatsOf> for DbLoader {
    type Value = ArtistStats;
    type Error = async_graphql::Error;

    async fn load(
        &self,
        keys: &[ArtistStatsOf],
    ) -> Result<HashMap<ArtistStatsOf, Self::Value>, Self::Error> {
        self.batch(keys, |db, keys| {
            let ids: Vec<i64> = keys.iter().map(|k| k.0).collect();
            let stats = queries::batch::artist_stats(&db.conn, &ids)
                .map_err(|e| internal_error("db", e))?;
            Ok(keys
                .into_iter()
                .map(|k| {
                    let s = stats.get(&k.0).copied().unwrap_or_default();
                    (k, s)
                })
                .collect())
        })
        .await
    }
}

impl Loader<AlbumTracks> for DbLoader {
    type Value = Vec<TrackRow>;
    type Error = async_graphql::Error;

    async fn load(
        &self,
        keys: &[AlbumTracks],
    ) -> Result<HashMap<AlbumTracks, Self::Value>, Self::Error> {
        self.batch(keys, |db, keys| {
            let ids: Vec<i64> = keys.iter().map(|k| k.0).collect();
            let mut by_album = queries::batch::tracks_for_albums(&db.conn, &ids)
                .map_err(|e| internal_error("db", e))?;
            Ok(keys
                .into_iter()
                .map(|k| {
                    let tracks = by_album.remove(&k.0).unwrap_or_default();
                    (k, tracks)
                })
                .collect())
        })
        .await
    }
}

impl Loader<AlbumStatsOf> for DbLoader {
    type Value = AlbumStats;
    type Error = async_graphql::Error;

    async fn load(
        &self,
        keys: &[AlbumStatsOf],
    ) -> Result<HashMap<AlbumStatsOf, Self::Value>, Self::Error> {
        self.batch(keys, |db, keys| {
            let ids: Vec<i64> = keys.iter().map(|k| k.0).collect();
            let stats =
                queries::batch::album_stats(&db.conn, &ids).map_err(|e| internal_error("db", e))?;
            Ok(keys
                .into_iter()
                .map(|k| {
                    let s = stats.get(&k.0).copied().unwrap_or_default();
                    (k, s)
                })
                .collect())
        })
        .await
    }
}

impl Loader<FavouriteTrack> for DbLoader {
    type Value = bool;
    type Error = async_graphql::Error;

    async fn load(
        &self,
        keys: &[FavouriteTrack],
    ) -> Result<HashMap<FavouriteTrack, Self::Value>, Self::Error> {
        self.batch(keys, |db, keys| {
            // One request is one user, so this is one query in practice.
            let mut by_user: HashMap<i64, Vec<i64>> = HashMap::new();
            for k in &keys {
                by_user.entry(k.0).or_default().push(k.1);
            }
            let mut starred = std::collections::HashSet::new();
            for (user, ids) in by_user {
                for id in queries::batch::favourite_track_ids(&db.conn, user, &ids)
                    .map_err(|e| internal_error("db", e))?
                {
                    starred.insert((user, id));
                }
            }
            Ok(keys
                .into_iter()
                .map(|k| {
                    let hit = starred.contains(&(k.0, k.1));
                    (k, hit)
                })
                .collect())
        })
        .await
    }
}

impl Loader<UidOf> for DbLoader {
    type Value = String;
    type Error = async_graphql::Error;

    async fn load(&self, keys: &[UidOf]) -> Result<HashMap<UidOf, Self::Value>, Self::Error> {
        self.batch(keys, |db, keys| {
            let mut out = HashMap::new();
            for kind in [
                UidKind::Artist,
                UidKind::Album,
                UidKind::Track,
                UidKind::Playlist,
            ] {
                let ids = keys.iter().filter(|k| k.0 == kind).map(|k| k.1);
                let uids =
                    queries::uids_for(&db.conn, kind, ids).map_err(|e| internal_error("db", e))?;
                out.extend(uids.into_iter().map(|(id, uid)| (UidOf(kind, id), uid)));
            }
            Ok(out)
        })
        .await
    }
}

impl Loader<RowOf> for DbLoader {
    type Value = i64;
    type Error = async_graphql::Error;

    async fn load(&self, keys: &[RowOf]) -> Result<HashMap<RowOf, Self::Value>, Self::Error> {
        self.batch(keys, |db, keys| {
            let mut out = HashMap::new();
            for key in keys {
                if let Some(id) = queries::id_for_uid(&db.conn, key.0, &key.1)
                    .map_err(|e| internal_error("db", e))?
                {
                    out.insert(key, id);
                }
            }
            Ok(out)
        })
        .await
    }
}
