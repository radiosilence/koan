//! Radio mode: automatic track selection based on what's currently playing.
//!
//! Uses a combination of signals to pick tracks that fit the current vibe:
//! - Subsonic getSimilarSongs2 (when remote is configured)
//! - Same/similar artist
//! - Same genre
//! - Cached similar-artist relationships (from Subsonic or Last.fm)

use std::collections::{HashMap, HashSet};

use rusqlite::Connection;

use crate::db::queries;
use crate::remote::client::SubsonicClient;

/// Context extracted from the current queue to guide radio picks.
#[derive(Debug, Default)]
pub struct RadioContext {
    /// Artist IDs in the queue, with play count (more = stronger signal).
    pub artist_counts: HashMap<i64, usize>,
    /// Genres in the queue, with count.
    pub genre_counts: HashMap<String, usize>,
    /// Paths already in the queue (to avoid duplicates).
    pub queued_paths: HashSet<String>,
    /// The currently playing track's remote_id (for Subsonic similar songs).
    pub current_remote_id: Option<String>,
    /// The currently playing track's artist name (for top songs fallback).
    pub current_artist_name: Option<String>,
}

impl RadioContext {
    /// Build context from a list of queue items.
    pub fn from_queue(items: &[(Option<i64>, Option<String>, Option<String>)]) -> Self {
        let mut ctx = Self::default();
        for (artist_id, genre, path) in items {
            if let Some(aid) = artist_id {
                *ctx.artist_counts.entry(*aid).or_default() += 1;
            }
            if let Some(g) = genre {
                for part in g.split(&[';', ',', '/'][..]) {
                    let trimmed = part.trim().to_lowercase();
                    if !trimmed.is_empty() {
                        *ctx.genre_counts.entry(trimmed).or_default() += 1;
                    }
                }
            }
            if let Some(p) = path {
                ctx.queued_paths.insert(p.clone());
            }
        }
        ctx
    }
}

/// Pick tracks for radio mode. Returns track IDs to enqueue.
///
/// Strategy:
/// 1. Try Subsonic getSimilarSongs2 if we have a remote and current track has a remote_id
/// 2. Try similar artists from cache (populated from Subsonic or Last.fm)
/// 3. Fall back to genre + artist matching from local library
pub fn pick_tracks(
    conn: &Connection,
    ctx: &RadioContext,
    client: Option<&SubsonicClient>,
    count: usize,
) -> Vec<i64> {
    let mut picks: Vec<i64> = Vec::new();

    // Strategy 1: Subsonic similar songs.
    if let Some(client) = client {
        if let Some(ref remote_id) = ctx.current_remote_id {
            picks.extend(pick_from_subsonic_similar(
                conn, client, remote_id, ctx, count,
            ));
        }
        // If we didn't get enough, try top songs for the current artist.
        if picks.len() < count
            && let Some(ref artist_name) = ctx.current_artist_name
        {
            picks.extend(pick_from_subsonic_top_songs(
                conn,
                client,
                artist_name,
                ctx,
                count - picks.len(),
                &picks,
            ));
        }
    }

    // Strategy 2: Similar artists from cache.
    if picks.len() < count {
        picks.extend(pick_from_similar_artists(
            conn,
            ctx,
            count - picks.len(),
            &picks,
        ));
    }

    // Strategy 3: Genre + artist matching from local library.
    if picks.len() < count {
        picks.extend(pick_from_local_library(
            conn,
            ctx,
            count - picks.len(),
            &picks,
        ));
    }

    picks
}

/// Check if a track is already queued by path.
fn is_queued(conn: &Connection, track_id: i64, queued_paths: &HashSet<String>) -> bool {
    let track = queries::get_track_row(conn, track_id).ok().flatten();
    if let Some(ref track) = track
        && let Some(ref path) = track.path
        && queued_paths.contains(path)
    {
        return true;
    }
    false
}

/// Try to get tracks from Subsonic getSimilarSongs2.
fn pick_from_subsonic_similar(
    conn: &Connection,
    client: &SubsonicClient,
    remote_id: &str,
    ctx: &RadioContext,
    count: usize,
) -> Vec<i64> {
    let songs = match client.get_similar_songs(remote_id, count * 3) {
        Ok(songs) => songs,
        Err(e) => {
            log::warn!("radio: getSimilarSongs2 failed: {}", e);
            return vec![];
        }
    };

    // Also cache the artist relationships we discover.
    cache_artist_relationships(conn, &songs);

    // Match Subsonic songs back to our local DB by remote_id.
    let mut picks = Vec::new();
    for song in &songs {
        if picks.len() >= count {
            break;
        }
        if let Some(track_id) = resolve_subsonic_song_to_track(conn, song) {
            if is_queued(conn, track_id, &ctx.queued_paths) {
                continue;
            }
            if !picks.contains(&track_id) {
                picks.push(track_id);
            }
        }
    }

    picks
}

/// Try Subsonic getTopSongs for the current artist.
fn pick_from_subsonic_top_songs(
    conn: &Connection,
    client: &SubsonicClient,
    artist_name: &str,
    ctx: &RadioContext,
    count: usize,
    already_picked: &[i64],
) -> Vec<i64> {
    let songs = match client.get_top_songs(artist_name, count * 3) {
        Ok(songs) => songs,
        Err(e) => {
            log::warn!("radio: getTopSongs failed: {}", e);
            return vec![];
        }
    };

    let mut picks = Vec::new();
    for song in &songs {
        if picks.len() >= count {
            break;
        }
        if let Some(track_id) = resolve_subsonic_song_to_track(conn, song) {
            if is_queued(conn, track_id, &ctx.queued_paths) {
                continue;
            }
            if !already_picked.contains(&track_id) && !picks.contains(&track_id) {
                picks.push(track_id);
            }
        }
    }

    picks
}

/// Pick tracks from similar artists cached in the DB.
fn pick_from_similar_artists(
    conn: &Connection,
    ctx: &RadioContext,
    count: usize,
    already_picked: &[i64],
) -> Vec<i64> {
    // Collect all similar artist IDs from the cache, weighted by how many
    // times their "parent" artist appears in the queue.
    let mut similar_artist_ids: Vec<(i64, f64)> = Vec::new();
    for (&artist_id, &queue_count) in &ctx.artist_counts {
        if let Ok(similar) = queries::get_similar_artists(conn, artist_id) {
            for (artist_row, score) in similar {
                // Weight by both the similarity score and how prominent
                // the parent artist is in the queue.
                let weighted = score * queue_count as f64;
                similar_artist_ids.push((artist_row.id, weighted));
            }
        }
    }

    if similar_artist_ids.is_empty() {
        return vec![];
    }

    // Sort by weighted score descending, take top artists.
    similar_artist_ids.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    similar_artist_ids.dedup_by_key(|x| x.0);

    // Get random tracks from these similar artists.
    let artist_ids: Vec<i64> = similar_artist_ids.iter().take(10).map(|x| x.0).collect();
    let exclude: Vec<String> = ctx.queued_paths.iter().cloned().collect();

    let genres: Vec<String> = ctx.genre_counts.keys().cloned().collect();
    match queries::random_tracks_excluding(conn, &exclude, &artist_ids, &genres, count * 3) {
        Ok(tracks) => tracks
            .into_iter()
            .filter(|t| !already_picked.contains(&t.id))
            .take(count)
            .map(|t| t.id)
            .collect(),
        Err(e) => {
            log::warn!("radio: similar artist query failed: {}", e);
            vec![]
        }
    }
}

/// Fall back to genre + artist matching from the local library.
fn pick_from_local_library(
    conn: &Connection,
    ctx: &RadioContext,
    count: usize,
    already_picked: &[i64],
) -> Vec<i64> {
    let artist_ids: Vec<i64> = ctx.artist_counts.keys().copied().collect();
    let genres: Vec<String> = ctx.genre_counts.keys().cloned().collect();
    let exclude: Vec<String> = ctx.queued_paths.iter().cloned().collect();

    match queries::random_tracks_excluding(conn, &exclude, &artist_ids, &genres, count * 3) {
        Ok(tracks) => tracks
            .into_iter()
            .filter(|t| !already_picked.contains(&t.id))
            .take(count)
            .map(|t| t.id)
            .collect(),
        Err(e) => {
            log::warn!("radio: local library query failed: {}", e);
            vec![]
        }
    }
}

/// Resolve a SubsonicSong to a local track ID by remote_id.
fn resolve_subsonic_song_to_track(
    conn: &Connection,
    song: &crate::remote::client::SubsonicSong,
) -> Option<i64> {
    let result = conn.query_row(
        "SELECT id FROM tracks WHERE remote_id = ?1",
        rusqlite::params![song.id],
        |row| row.get::<_, i64>(0),
    );
    result.ok()
}

/// Extract artist relationships from a Subsonic similar songs response
/// and cache them in the DB for future use.
fn cache_artist_relationships(conn: &Connection, songs: &[crate::remote::client::SubsonicSong]) {
    // For now, the similar_artists cache is populated by explicit API calls.
    // This function is a hook for future enrichment.
    let _ = (conn, songs);
}

/// Populate the similar artists cache for a given artist using Subsonic.
pub fn fetch_and_cache_similar_artists(
    conn: &Connection,
    client: &SubsonicClient,
    artist_id: i64,
) -> Result<(), Box<dyn std::error::Error>> {
    // Check if we already have fresh data.
    if queries::has_fresh_similar_artists(conn, artist_id).unwrap_or(false) {
        return Ok(());
    }

    // Get the artist's remote_id to find their songs.
    let remote_id: Option<Option<String>> = conn
        .query_row(
            "SELECT remote_id FROM artists WHERE id = ?1",
            rusqlite::params![artist_id],
            |row| row.get(0),
        )
        .ok();

    let Some(_remote_id) = remote_id.flatten() else {
        return Ok(());
    };

    // Get a representative track for this artist.
    let track_remote_id: Option<String> = conn
        .query_row(
            "SELECT remote_id FROM tracks WHERE artist_id = ?1 AND remote_id IS NOT NULL LIMIT 1",
            rusqlite::params![artist_id],
            |row| row.get(0),
        )
        .ok()
        .flatten();

    let Some(track_remote_id) = track_remote_id else {
        return Ok(());
    };

    // Get similar songs and extract unique artists.
    let songs = client.get_similar_songs(&track_remote_id, 50)?;
    let mut similar_artists: HashMap<i64, f64> = HashMap::new();
    let total = songs.len() as f64;

    for (i, song) in songs.iter().enumerate() {
        if let Some(ref song_artist_id) = song.artist_id {
            // Try to find this artist in our DB.
            let local_artist_id: Option<i64> = conn
                .query_row(
                    "SELECT id FROM artists WHERE remote_id = ?1",
                    rusqlite::params![song_artist_id],
                    |row| row.get(0),
                )
                .ok();

            if let Some(local_id) = local_artist_id
                && local_id != artist_id
            {
                // Score: higher-ranked songs = higher score.
                let score = (total - i as f64) / total;
                let entry = similar_artists.entry(local_id).or_insert(0.0);
                *entry = entry.max(score);
            }
        }
    }

    if !similar_artists.is_empty() {
        let pairs: Vec<(i64, f64)> = similar_artists.into_iter().collect();
        queries::save_similar_artists(conn, artist_id, &pairs, "subsonic")?;
    }

    Ok(())
}
