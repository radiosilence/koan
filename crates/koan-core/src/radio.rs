//! Radio mode: multi-signal discovery from local library + metadata APIs.
//!
//! Uses multiple similarity axes to pick tracks that feel like a coherent journey:
//! - ListenBrainz similar artists (ML-based, no API key)
//! - MusicBrainz relationships (collaborators, band members, associated acts)
//! - Subsonic getSimilarSongs2 (when remote is configured)
//! - Genre/era matching from local metadata
//! - Play history for recency scoring (surface buried gems)
//!
//! The seed *drifts* — recent plays are weighted more heavily than the initial track,
//! so the radio evolves through your library instead of orbiting one point.

use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;

use crate::config::RadioConfig;
use crate::db::queries;
use crate::remote::client::SubsonicClient;
use crate::remote::listenbrainz;
use crate::remote::musicbrainz;

/// Which similarity axis led to a candidate being selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SimilarityAxis {
    ListenBrainz,
    MusicBrainz,
    Subsonic,
    GenreEra,
    SameArtist,
    Random,
}

/// A candidate track with its scoring breakdown.
#[derive(Debug)]
struct Candidate {
    track_id: i64,
    #[allow(dead_code)]
    artist_id: Option<i64>,
    path: Option<String>,
    #[allow(dead_code)]
    genre: Option<String>,
    year: Option<i32>,
    #[allow(dead_code)]
    duration_ms: Option<i64>,
    /// Similarity axes that contributed to this candidate.
    axes: HashSet<SimilarityAxis>,
    /// Base similarity score (0.0..1.0).
    base_score: f64,
}

/// Context extracted from the current queue and play history to guide radio picks.
#[derive(Debug, Default)]
pub struct RadioContext {
    /// Artist IDs from the seed window, with recency weight (more recent = higher).
    pub seed_artists: HashMap<i64, f64>,
    /// Paths already in the queue (to avoid duplicates).
    pub queued_paths: HashSet<String>,
    /// Track IDs in the recent play history exclusion window.
    pub excluded_track_ids: HashSet<i64>,
    /// The currently playing track's remote_id (for Subsonic similar songs).
    pub current_remote_id: Option<String>,
    /// The currently playing track's artist name (for top songs fallback).
    pub current_artist_name: Option<String>,
    /// Genres from the seed window.
    pub seed_genres: HashSet<String>,
    /// Average year of seed tracks (for era matching).
    pub seed_avg_year: Option<i32>,
}

impl RadioContext {
    /// Build context from queue items and play history.
    ///
    /// `queue_items`: (artist_id, path) pairs from the current queue.
    /// `seed_window`: number of recent tracks to use as seeds.
    /// `history_window`: number of recent track IDs to exclude.
    pub fn build(
        conn: &Connection,
        queue_items: &[(Option<i64>, Option<String>)],
        seed_window: usize,
        history_window: usize,
    ) -> Self {
        let mut ctx = Self::default();

        // Add queued paths for duplicate prevention.
        for (_aid, path) in queue_items {
            if let Some(p) = path {
                ctx.queued_paths.insert(p.clone());
            }
        }

        // Build seed from recent plays (drifting seed).
        let recent = queries::recent_track_ids(conn, seed_window).unwrap_or_default();
        let seed_count = recent.len().max(1) as f64;

        for (i, track_id) in recent.iter().enumerate() {
            if let Ok(Some(track)) = queries::get_track_row(conn, *track_id) {
                // More recent = higher weight (linear decay).
                let weight = (seed_count - i as f64) / seed_count;
                if let Some(aid) = track.artist_id {
                    let entry = ctx.seed_artists.entry(aid).or_insert(0.0);
                    *entry = entry.max(weight);
                }
                if let Some(ref genre) = track.genre {
                    ctx.seed_genres.insert(genre.clone());
                }
            }
        }

        // If no play history, fall back to queue artist weights (old behavior).
        if ctx.seed_artists.is_empty() {
            for (artist_id, _path) in queue_items {
                if let Some(aid) = artist_id {
                    *ctx.seed_artists.entry(*aid).or_default() += 1.0;
                }
            }
            // Normalise.
            let max = ctx.seed_artists.values().copied().fold(1.0_f64, f64::max);
            for v in ctx.seed_artists.values_mut() {
                *v /= max;
            }

            // Collect genres from queue.
            for (artist_id, _path) in queue_items {
                if let Some(aid) = artist_id
                    && let Ok(tracks) = queries::random_tracks_excluding(conn, &[], &[*aid], &[], 1)
                {
                    for t in tracks {
                        if let Some(ref g) = t.genre {
                            ctx.seed_genres.insert(g.clone());
                        }
                    }
                }
            }
        }

        // Build exclusion window from play history.
        let excluded = queries::recent_track_ids(conn, history_window).unwrap_or_default();
        ctx.excluded_track_ids = excluded.into_iter().collect();

        // Compute average year from seed tracks.
        let mut years: Vec<i32> = Vec::new();
        let seed_ids = queries::recent_track_ids(conn, seed_window).unwrap_or_default();
        for tid in &seed_ids {
            if let Ok(Some(track)) = queries::get_track_row(conn, *tid)
                && let Some(album_id) = track.album_id
                && let Ok(Some(album)) = queries::get_album(conn, album_id)
                && let Some(ref date) = album.date
                && let Ok(year) = date[..4.min(date.len())].parse::<i32>()
            {
                years.push(year);
            }
        }
        if !years.is_empty() {
            ctx.seed_avg_year = Some(years.iter().sum::<i32>() / years.len() as i32);
        }

        ctx
    }

    /// Legacy builder for backward compat — used by TUI when play history is empty.
    pub fn from_queue(items: &[(Option<i64>, Option<String>)]) -> Self {
        let mut ctx = Self::default();
        for (artist_id, path) in items {
            if let Some(aid) = artist_id {
                *ctx.seed_artists.entry(*aid).or_default() += 1.0;
            }
            if let Some(p) = path {
                ctx.queued_paths.insert(p.clone());
            }
        }
        // Normalise.
        let max = ctx.seed_artists.values().copied().fold(1.0_f64, f64::max);
        if max > 0.0 {
            for v in ctx.seed_artists.values_mut() {
                *v /= max;
            }
        }
        ctx
    }
}

/// Pick tracks for radio mode. Returns track IDs to enqueue.
///
/// Multi-signal strategy with fallback chain:
/// 1. ListenBrainz similar artists -> local tracks
/// 2. MusicBrainz relationships -> local tracks by collaborators/associated acts
/// 3. Subsonic getSimilarSongs2 (if remote configured)
/// 4. Genre + era match -> local tracks with matching tags from similar decade
/// 5. Same-artist fallback
/// 6. Random from library (nuclear fallback)
pub fn pick_tracks(
    conn: &Connection,
    ctx: &RadioContext,
    client: Option<&SubsonicClient>,
    config: &RadioConfig,
) -> Vec<i64> {
    let count = config.batch_size;
    let mut candidates: Vec<Candidate> = Vec::new();

    log::info!(
        "radio: picking {} tracks (seed: {} artists, {} genres, {} excluded, remote_id={}, artist={})",
        count,
        ctx.seed_artists.len(),
        ctx.seed_genres.len(),
        ctx.excluded_track_ids.len(),
        ctx.current_remote_id.as_deref().unwrap_or("none"),
        ctx.current_artist_name.as_deref().unwrap_or("none"),
    );

    // --- Signal 1: ListenBrainz similar artists ---
    gather_listenbrainz_candidates(conn, ctx, &mut candidates);

    // --- Signal 2: MusicBrainz relationships ---
    gather_musicbrainz_candidates(conn, ctx, &mut candidates);

    // --- Signal 3: Subsonic similar songs ---
    if let Some(client) = client {
        gather_subsonic_candidates(conn, ctx, client, &mut candidates);
    }

    // --- Signal 4: Genre + era match ---
    gather_genre_era_candidates(conn, ctx, &mut candidates);

    // --- Signal 5: Same-artist tracks ---
    gather_same_artist_candidates(conn, ctx, &mut candidates);

    // --- Signal 6: Random library tracks ---
    gather_random_candidates(conn, ctx, &mut candidates);

    log::info!("radio: {} raw candidates before scoring", candidates.len());

    // Deduplicate by track_id, merging axes.
    let mut deduped: HashMap<i64, Candidate> = HashMap::new();
    for c in candidates {
        let entry = deduped.entry(c.track_id).or_insert(Candidate {
            track_id: c.track_id,
            artist_id: c.artist_id,
            path: c.path.clone(),
            genre: c.genre.clone(),
            year: c.year,
            duration_ms: c.duration_ms,
            axes: HashSet::new(),
            base_score: 0.0,
        });
        entry.axes.extend(c.axes.iter());
        entry.base_score = entry.base_score.max(c.base_score);
    }

    // Filter out excluded tracks and already-queued.
    let mut scored: Vec<(i64, f64)> = deduped
        .into_values()
        .filter(|c| !ctx.excluded_track_ids.contains(&c.track_id))
        .filter(|c| {
            c.path
                .as_ref()
                .is_none_or(|p| !ctx.queued_paths.contains(p))
        })
        .map(|c| {
            let score = compute_score(conn, &c, ctx, config);
            (c.track_id, score)
        })
        .collect();

    // Sort by score descending.
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    // Weighted random selection from top candidates for variety.
    let picks = weighted_select(&scored, count);

    log::info!("radio: picked {} tracks", picks.len());
    picks
}

/// Compute final score for a candidate.
fn compute_score(
    conn: &Connection,
    candidate: &Candidate,
    ctx: &RadioContext,
    config: &RadioConfig,
) -> f64 {
    let base = candidate.base_score;

    // Signal overlap bonus: tracks matching on 2+ axes score higher.
    let overlap_bonus = match candidate.axes.len() {
        0 | 1 => 1.0,
        2 => 1.5,
        3 => 2.0,
        _ => 2.5,
    };

    // Recency bonus: boost tracks that haven't been played recently or ever.
    let recency_bonus = compute_recency_bonus(conn, candidate.track_id, config.discovery_weight);

    // Era proximity bonus (if we have year data).
    let era_bonus = if let (Some(track_year), Some(seed_year)) = (candidate.year, ctx.seed_avg_year)
    {
        let diff = (track_year - seed_year).unsigned_abs();
        if diff <= 5 {
            1.3
        } else if diff <= 10 {
            1.1
        } else {
            1.0
        }
    } else {
        1.0
    };

    base * overlap_bonus * recency_bonus * era_bonus
}

/// Compute recency bonus for a track. Higher = more desirable.
/// Never-played tracks get the highest bonus.
fn compute_recency_bonus(conn: &Connection, track_id: i64, discovery_weight: f64) -> f64 {
    let last_played = queries::last_played_at(conn, track_id).unwrap_or(None);
    match last_played {
        None => {
            // Never played — big bonus, scaled by discovery_weight.
            1.0 + discovery_weight * 2.0
        }
        Some(ts) => {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            let days_ago = (now - ts) / 86400;
            if days_ago > 180 {
                1.0 + discovery_weight * 1.5 // "oh fuck, I forgot I owned this"
            } else if days_ago > 30 {
                1.0 + discovery_weight * 0.8
            } else if days_ago > 7 {
                1.0 + discovery_weight * 0.3
            } else {
                1.0 // Recently played — no bonus.
            }
        }
    }
}

/// Weighted random selection from scored candidates.
/// Takes the top N*3 candidates and selects N with probability proportional to score.
fn weighted_select(scored: &[(i64, f64)], count: usize) -> Vec<i64> {
    if scored.is_empty() {
        return vec![];
    }

    let pool_size = (count * 3).min(scored.len());
    let pool = &scored[..pool_size];

    // Simple selection from the top-scored pool.
    let mut selected = Vec::new();
    let mut used = HashSet::new();

    for (id, _score) in pool {
        if selected.len() >= count {
            break;
        }
        if used.insert(*id) {
            selected.push(*id);
        }
    }

    selected
}

// --- Signal gatherers ---

fn gather_listenbrainz_candidates(
    conn: &Connection,
    ctx: &RadioContext,
    candidates: &mut Vec<Candidate>,
) {
    let http = reqwest::blocking::Client::new();

    for (&artist_id, &weight) in ctx.seed_artists.iter().take(3) {
        // Get artist MBID from our DB.
        let mbid: Option<String> = conn
            .query_row(
                "SELECT mbid FROM artists WHERE id = ?1",
                rusqlite::params![artist_id],
                |row| row.get(0),
            )
            .ok()
            .flatten();

        let mbid = match mbid {
            Some(m) if !m.is_empty() => m,
            _ => {
                // Try to look up MBID via MusicBrainz search.
                let artist_name: Option<String> = conn
                    .query_row(
                        "SELECT name FROM artists WHERE id = ?1",
                        rusqlite::params![artist_id],
                        |row| row.get(0),
                    )
                    .ok();
                if let Some(name) = artist_name {
                    match musicbrainz::lookup_artist_mbid(&http, &name) {
                        Ok(Some(mbid)) => {
                            // Cache the MBID.
                            let _ = conn.execute(
                                "UPDATE artists SET mbid = ?1 WHERE id = ?2",
                                rusqlite::params![mbid, artist_id],
                            );
                            mbid
                        }
                        _ => continue,
                    }
                } else {
                    continue;
                }
            }
        };

        // Check if we have fresh ListenBrainz data cached.
        if queries::has_fresh_similar_artists_for_source(conn, artist_id, Some("listenbrainz"))
            .unwrap_or(false)
        {
            // Use cached data.
            add_cached_similar_candidates(
                conn,
                ctx,
                artist_id,
                weight,
                SimilarityAxis::ListenBrainz,
                candidates,
            );
            continue;
        }

        // Fetch from API.
        match listenbrainz::get_similar_artists(&http, &mbid, 20) {
            Ok(similar) => {
                log::info!(
                    "radio: listenbrainz returned {} similar for artist_id={}",
                    similar.len(),
                    artist_id
                );
                // Match to local artists and cache.
                let mut pairs: Vec<(i64, f64)> = Vec::new();
                for sa in &similar {
                    // Try to find by MBID first, then by name.
                    let local_id: Option<i64> = conn
                        .query_row(
                            "SELECT id FROM artists WHERE mbid = ?1",
                            rusqlite::params![sa.mbid],
                            |row| row.get(0),
                        )
                        .ok()
                        .or_else(|| {
                            conn.query_row(
                                "SELECT id FROM artists WHERE name = ?1 COLLATE NOCASE",
                                rusqlite::params![sa.name],
                                |row| row.get(0),
                            )
                            .ok()
                        });

                    if let Some(local_id) = local_id
                        && local_id != artist_id
                    {
                        pairs.push((local_id, sa.score));
                    }
                }

                if !pairs.is_empty() {
                    let _ = queries::save_similar_artists(conn, artist_id, &pairs, "listenbrainz");
                }

                // Add candidates from the matched local artists.
                add_local_artist_candidates(
                    conn,
                    ctx,
                    &pairs,
                    weight,
                    SimilarityAxis::ListenBrainz,
                    candidates,
                );
            }
            Err(e) => {
                log::debug!(
                    "radio: listenbrainz failed for artist_id={}: {}",
                    artist_id,
                    e
                );
                // Fall through — other signals will pick up the slack.
            }
        }
    }
}

fn gather_musicbrainz_candidates(
    conn: &Connection,
    ctx: &RadioContext,
    candidates: &mut Vec<Candidate>,
) {
    let http = musicbrainz::default_client();

    for (&artist_id, &weight) in ctx.seed_artists.iter().take(3) {
        // Check cache first.
        if queries::has_fresh_similar_artists_for_source(conn, artist_id, Some("musicbrainz"))
            .unwrap_or(false)
        {
            add_cached_similar_candidates(
                conn,
                ctx,
                artist_id,
                weight,
                SimilarityAxis::MusicBrainz,
                candidates,
            );
            continue;
        }

        let mbid: Option<String> = conn
            .query_row(
                "SELECT mbid FROM artists WHERE id = ?1",
                rusqlite::params![artist_id],
                |row| row.get(0),
            )
            .ok()
            .flatten();

        let Some(mbid) = mbid.filter(|m| !m.is_empty()) else {
            continue;
        };

        match musicbrainz::get_artist_relations(&http, &mbid) {
            Ok(relations) => {
                log::info!(
                    "radio: musicbrainz returned {} relations for artist_id={}",
                    relations.len(),
                    artist_id
                );

                let mut pairs: Vec<(i64, f64)> = Vec::new();

                for rel in &relations {
                    let local_id: Option<i64> = conn
                        .query_row(
                            "SELECT id FROM artists WHERE mbid = ?1",
                            rusqlite::params![rel.mbid],
                            |row| row.get(0),
                        )
                        .ok()
                        .or_else(|| {
                            conn.query_row(
                                "SELECT id FROM artists WHERE name = ?1 COLLATE NOCASE",
                                rusqlite::params![rel.name],
                                |row| row.get(0),
                            )
                            .ok()
                        });

                    if let Some(local_id) = local_id
                        && local_id != artist_id
                    {
                        // Score by relationship type.
                        let score = match rel.category {
                            musicbrainz::RelationCategory::Member => 0.8,
                            musicbrainz::RelationCategory::Collaborator => 0.7,
                            musicbrainz::RelationCategory::Associated => 0.5,
                        };
                        pairs.push((local_id, score));
                    }
                }

                if !pairs.is_empty() {
                    let _ = queries::save_similar_artists_with_rel(
                        conn,
                        artist_id,
                        &pairs,
                        "musicbrainz",
                        "collaborator",
                    );
                }

                add_local_artist_candidates(
                    conn,
                    ctx,
                    &pairs,
                    weight,
                    SimilarityAxis::MusicBrainz,
                    candidates,
                );
            }
            Err(e) => {
                log::debug!(
                    "radio: musicbrainz relations failed for artist_id={}: {}",
                    artist_id,
                    e
                );
            }
        }

        // Rate limit: sleep 1s between MusicBrainz requests.
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

fn gather_subsonic_candidates(
    conn: &Connection,
    ctx: &RadioContext,
    client: &SubsonicClient,
    candidates: &mut Vec<Candidate>,
) {
    if let Some(ref remote_id) = ctx.current_remote_id {
        match client.get_similar_songs(remote_id, 30) {
            Ok(songs) => {
                log::info!("radio: subsonic returned {} similar songs", songs.len());
                for (i, song) in songs.iter().enumerate() {
                    if let Some(track_id) = resolve_subsonic_song_to_track(conn, song) {
                        let score = (songs.len() as f64 - i as f64) / songs.len() as f64;
                        let track = queries::get_track_row(conn, track_id).ok().flatten();
                        candidates.push(Candidate {
                            track_id,
                            artist_id: track.as_ref().and_then(|t| t.artist_id),
                            path: track.as_ref().and_then(|t| t.path.clone()),
                            genre: track.as_ref().and_then(|t| t.genre.clone()),
                            year: None,
                            duration_ms: track.as_ref().and_then(|t| t.duration_ms),
                            axes: [SimilarityAxis::Subsonic].into_iter().collect(),
                            base_score: score * 0.9,
                        });
                    }
                }

                // Cache artist relationships from subsonic results.
                cache_subsonic_artist_relationships(conn, ctx, &songs);
            }
            Err(e) => {
                log::debug!("radio: subsonic similar songs failed: {}", e);
            }
        }
    }
}

fn gather_genre_era_candidates(
    conn: &Connection,
    ctx: &RadioContext,
    candidates: &mut Vec<Candidate>,
) {
    if ctx.seed_genres.is_empty() {
        return;
    }

    let genres: Vec<String> = ctx.seed_genres.iter().cloned().collect();
    let exclude: Vec<String> = ctx.queued_paths.iter().cloned().collect();

    match queries::random_tracks_excluding(conn, &exclude, &[], &genres, 30) {
        Ok(tracks) => {
            for track in tracks {
                let year = track
                    .album_id
                    .and_then(|aid| queries::get_album(conn, aid).ok().flatten())
                    .and_then(|a| {
                        a.date
                            .as_ref()
                            .and_then(|d| d[..4.min(d.len())].parse().ok())
                    });

                // Score higher if both genre AND era match.
                let genre_match = track
                    .genre
                    .as_ref()
                    .is_some_and(|g| ctx.seed_genres.contains(g));
                let era_match = match (year, ctx.seed_avg_year) {
                    (Some(y), Some(sy)) => (y as i64 - sy as i64).unsigned_abs() <= 10,
                    _ => false,
                };

                let base_score = match (genre_match, era_match) {
                    (true, true) => 0.6,
                    (true, false) => 0.3,
                    (false, true) => 0.2,
                    (false, false) => 0.1,
                };

                candidates.push(Candidate {
                    track_id: track.id,
                    artist_id: track.artist_id,
                    path: track.path.clone(),
                    genre: track.genre.clone(),
                    year,
                    duration_ms: track.duration_ms,
                    axes: [SimilarityAxis::GenreEra].into_iter().collect(),
                    base_score,
                });
            }
        }
        Err(e) => {
            log::debug!("radio: genre/era query failed: {}", e);
        }
    }
}

fn gather_same_artist_candidates(
    conn: &Connection,
    ctx: &RadioContext,
    candidates: &mut Vec<Candidate>,
) {
    let artist_ids: Vec<i64> = ctx.seed_artists.keys().copied().collect();
    if artist_ids.is_empty() {
        return;
    }

    let exclude: Vec<String> = ctx.queued_paths.iter().cloned().collect();
    match queries::random_tracks_excluding(conn, &exclude, &artist_ids, &[], 15) {
        Ok(tracks) => {
            for track in tracks {
                let weight = track
                    .artist_id
                    .and_then(|aid| ctx.seed_artists.get(&aid))
                    .copied()
                    .unwrap_or(0.3);

                candidates.push(Candidate {
                    track_id: track.id,
                    artist_id: track.artist_id,
                    path: track.path.clone(),
                    genre: track.genre.clone(),
                    year: None,
                    duration_ms: track.duration_ms,
                    axes: [SimilarityAxis::SameArtist].into_iter().collect(),
                    base_score: weight * 0.4, // Lower base — same-artist is the fallback.
                });
            }
        }
        Err(e) => {
            log::debug!("radio: same-artist query failed: {}", e);
        }
    }
}

fn gather_random_candidates(
    conn: &Connection,
    ctx: &RadioContext,
    candidates: &mut Vec<Candidate>,
) {
    let exclude: Vec<String> = ctx.queued_paths.iter().cloned().collect();
    match queries::random_tracks_excluding(conn, &exclude, &[], &[], 10) {
        Ok(tracks) => {
            for track in tracks {
                candidates.push(Candidate {
                    track_id: track.id,
                    artist_id: track.artist_id,
                    path: track.path.clone(),
                    genre: track.genre.clone(),
                    year: None,
                    duration_ms: track.duration_ms,
                    axes: [SimilarityAxis::Random].into_iter().collect(),
                    base_score: 0.05, // Nuclear fallback — still better than silence.
                });
            }
        }
        Err(e) => {
            log::debug!("radio: random fallback failed: {}", e);
        }
    }
}

// --- Helpers ---

/// Add candidates from cached similar artist data.
fn add_cached_similar_candidates(
    conn: &Connection,
    ctx: &RadioContext,
    artist_id: i64,
    seed_weight: f64,
    axis: SimilarityAxis,
    candidates: &mut Vec<Candidate>,
) {
    if let Ok(similar) = queries::get_similar_artists(conn, artist_id) {
        let pairs: Vec<(i64, f64)> = similar.into_iter().map(|(a, s)| (a.id, s)).collect();
        add_local_artist_candidates(conn, ctx, &pairs, seed_weight, axis, candidates);
    }
}

/// Add candidates from a list of (artist_id, similarity_score) pairs.
fn add_local_artist_candidates(
    conn: &Connection,
    ctx: &RadioContext,
    pairs: &[(i64, f64)],
    seed_weight: f64,
    axis: SimilarityAxis,
    candidates: &mut Vec<Candidate>,
) {
    for &(similar_artist_id, sim_score) in pairs.iter().take(10) {
        let exclude: Vec<String> = ctx.queued_paths.iter().cloned().collect();
        if let Ok(tracks) =
            queries::random_tracks_excluding(conn, &exclude, &[similar_artist_id], &[], 3)
        {
            for track in tracks {
                candidates.push(Candidate {
                    track_id: track.id,
                    artist_id: track.artist_id,
                    path: track.path.clone(),
                    genre: track.genre.clone(),
                    year: None,
                    duration_ms: track.duration_ms,
                    axes: [axis].into_iter().collect(),
                    base_score: sim_score * seed_weight * 0.8,
                });
            }
        }
    }
}

/// Resolve a SubsonicSong to a local track ID by remote_id.
fn resolve_subsonic_song_to_track(
    conn: &Connection,
    song: &crate::remote::client::SubsonicSong,
) -> Option<i64> {
    conn.query_row(
        "SELECT id FROM tracks WHERE remote_id = ?1",
        rusqlite::params![song.id],
        |row| row.get::<_, i64>(0),
    )
    .ok()
}

/// Extract and cache artist relationships from Subsonic similar songs response.
fn cache_subsonic_artist_relationships(
    conn: &Connection,
    ctx: &RadioContext,
    songs: &[crate::remote::client::SubsonicSong],
) {
    for &artist_id in ctx.seed_artists.keys().take(5) {
        let mut similar: HashMap<i64, f64> = HashMap::new();
        let total = songs.len() as f64;

        for (i, song) in songs.iter().enumerate() {
            if let Some(ref song_artist_id) = song.artist_id {
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
                    let score = (total - i as f64) / total;
                    let entry = similar.entry(local_id).or_insert(0.0);
                    *entry = entry.max(score);
                }
            }
        }

        if !similar.is_empty() {
            let pairs: Vec<(i64, f64)> = similar.into_iter().collect();
            let _ = queries::save_similar_artists(conn, artist_id, &pairs, "subsonic");
        }
    }
}

/// Populate the similar artists cache for a given artist using Subsonic.
/// Kept for backward compat with the TUI trigger.
pub fn fetch_and_cache_similar_artists(
    conn: &Connection,
    client: &SubsonicClient,
    artist_id: i64,
) -> Result<(), Box<dyn std::error::Error>> {
    if queries::has_fresh_similar_artists_for_source(conn, artist_id, Some("subsonic"))
        .unwrap_or(false)
    {
        return Ok(());
    }

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

    let songs = client.get_similar_songs(&track_remote_id, 50)?;
    let mut similar_artists: HashMap<i64, f64> = HashMap::new();
    let total = songs.len() as f64;

    for (i, song) in songs.iter().enumerate() {
        if let Some(ref song_artist_id) = song.artist_id {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::Database;
    use crate::db::queries::{get_or_create_artist, sample_meta, upsert_track};

    fn test_db() -> Database {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        Database { conn }
    }

    #[test]
    fn test_radio_context_from_queue() {
        let ctx = RadioContext::from_queue(&[
            (Some(1), Some("/a.flac".into())),
            (Some(2), Some("/b.flac".into())),
            (Some(1), Some("/c.flac".into())),
        ]);
        assert_eq!(ctx.seed_artists.len(), 2);
        assert!(ctx.seed_artists[&1] > ctx.seed_artists[&2]);
        assert_eq!(ctx.queued_paths.len(), 3);
    }

    #[test]
    fn test_recency_bonus_never_played() {
        let db = test_db();
        let mut meta = sample_meta("T1", "A1", "Al1");
        meta.path = Some("/music/T1.flac".into());
        upsert_track(&db.conn, &meta).unwrap();

        let track_id: i64 = db
            .conn
            .query_row("SELECT id FROM tracks LIMIT 1", [], |row| row.get(0))
            .unwrap();

        let bonus = compute_recency_bonus(&db.conn, track_id, 0.3);
        assert!(bonus > 1.0, "never-played should get a bonus");
    }

    #[test]
    fn test_recency_bonus_recently_played() {
        let db = test_db();
        let mut meta = sample_meta("T1", "A1", "Al1");
        meta.path = Some("/music/T1.flac".into());
        upsert_track(&db.conn, &meta).unwrap();

        let track_id: i64 = db
            .conn
            .query_row("SELECT id FROM tracks LIMIT 1", [], |row| row.get(0))
            .unwrap();

        queries::record_play(&db.conn, track_id, Some(240_000)).unwrap();

        let bonus = compute_recency_bonus(&db.conn, track_id, 0.3);
        assert!(
            (bonus - 1.0).abs() < f64::EPSILON,
            "recently played should get no bonus"
        );
    }

    #[test]
    fn test_weighted_select_empty() {
        assert!(weighted_select(&[], 5).is_empty());
    }

    #[test]
    fn test_weighted_select_fewer_than_requested() {
        let scored = vec![(1, 0.9), (2, 0.5)];
        let picks = weighted_select(&scored, 5);
        assert_eq!(picks.len(), 2);
    }

    #[test]
    fn test_signal_overlap_scoring() {
        let db = test_db();
        let config = RadioConfig::default();
        let ctx = RadioContext::default();

        // Candidate with 1 axis.
        let c1 = Candidate {
            track_id: 1,
            artist_id: None,
            path: None,
            genre: None,
            year: None,
            duration_ms: None,
            axes: [SimilarityAxis::ListenBrainz].into_iter().collect(),
            base_score: 0.5,
        };

        // Candidate with 3 axes.
        let c3 = Candidate {
            track_id: 2,
            artist_id: None,
            path: None,
            genre: None,
            year: None,
            duration_ms: None,
            axes: [
                SimilarityAxis::ListenBrainz,
                SimilarityAxis::MusicBrainz,
                SimilarityAxis::GenreEra,
            ]
            .into_iter()
            .collect(),
            base_score: 0.5,
        };

        let score1 = compute_score(&db.conn, &c1, &ctx, &config);
        let score3 = compute_score(&db.conn, &c3, &ctx, &config);

        assert!(
            score3 > score1,
            "multi-axis candidate should score higher: {} vs {}",
            score3,
            score1
        );
    }

    #[test]
    fn test_pick_tracks_empty_library() {
        let db = test_db();
        let ctx = RadioContext::from_queue(&[]);
        let config = RadioConfig::default();
        let picks = pick_tracks(&db.conn, &ctx, None, &config);
        assert!(picks.is_empty());
    }

    #[test]
    fn test_pick_tracks_with_library() {
        let db = test_db();

        // Populate library.
        for i in 0..20 {
            let mut meta = sample_meta(
                &format!("Track{}", i),
                &format!("Artist{}", i % 5),
                &format!("Album{}", i % 3),
            );
            meta.path = Some(format!("/music/Album{}/Track{}.flac", i % 3, i));
            meta.track_number = Some(i);
            upsert_track(&db.conn, &meta).unwrap();
        }

        let artist_id: i64 = db
            .conn
            .query_row("SELECT id FROM artists LIMIT 1", [], |row| row.get(0))
            .unwrap();

        let ctx = RadioContext::from_queue(&[(Some(artist_id), Some("/queued.flac".into()))]);
        let config = RadioConfig {
            batch_size: 5,
            ..RadioConfig::default()
        };

        let picks = pick_tracks(&db.conn, &ctx, None, &config);
        assert!(
            !picks.is_empty(),
            "should pick at least some tracks from a populated library"
        );
        assert!(picks.len() <= 5);
    }

    #[test]
    fn test_pick_tracks_excludes_history() {
        let db = test_db();

        // Insert a few tracks.
        for i in 0..5 {
            let mut meta = sample_meta(&format!("T{}", i), "Artist", "Album");
            meta.path = Some(format!("/music/T{}.flac", i));
            meta.track_number = Some(i);
            upsert_track(&db.conn, &meta).unwrap();
        }

        // Record all as recently played.
        let mut ids = Vec::new();
        for i in 0..5 {
            let id: i64 = db
                .conn
                .query_row(
                    "SELECT id FROM tracks WHERE path = ?1",
                    rusqlite::params![format!("/music/T{}.flac", i)],
                    |row| row.get(0),
                )
                .unwrap();
            queries::record_play(&db.conn, id, Some(240_000)).unwrap();
            ids.push(id);
        }

        let mut ctx = RadioContext::from_queue(&[]);
        ctx.excluded_track_ids = ids.into_iter().collect();

        let config = RadioConfig {
            batch_size: 5,
            ..RadioConfig::default()
        };

        let picks = pick_tracks(&db.conn, &ctx, None, &config);
        // All tracks are excluded, so nothing should be picked.
        assert!(
            picks.is_empty(),
            "all tracks in exclusion window, got picks"
        );
    }

    #[test]
    fn test_radio_context_build_with_no_history() {
        let db = test_db();

        for i in 0..5 {
            let _id = get_or_create_artist(&db.conn, &format!("Artist{}", i), None).unwrap();
        }

        let queue = vec![
            (Some(1_i64), Some("/a.flac".to_string())),
            (Some(2), Some("/b.flac".to_string())),
        ];
        let ctx = RadioContext::build(&db.conn, &queue, 5, 200);

        // Should fall back to queue weights since no play history.
        assert_eq!(ctx.seed_artists.len(), 2);
        assert_eq!(ctx.queued_paths.len(), 2);
    }
}
