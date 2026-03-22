//! Lightweight GraphQL client for connecting to a `koan serve` instance.
//!
//! Uses blocking reqwest — call from a background thread when used from the TUI.

use serde_json::Value;

/// A GraphQL client that talks to a koan server.
#[derive(Debug, Clone)]
pub struct GraphQLClient {
    url: String,
    http: reqwest::blocking::Client,
}

impl GraphQLClient {
    pub fn new(server_url: &str) -> Self {
        let url = format!("{}/graphql", server_url.trim_end_matches('/'));
        Self {
            url,
            http: reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("failed to build HTTP client"),
        }
    }

    /// Execute a raw GraphQL query/mutation.
    pub fn execute(&self, query: &str, variables: Option<Value>) -> Result<Value, GraphQLError> {
        let mut body = serde_json::json!({ "query": query });
        if let Some(vars) = variables {
            body["variables"] = vars;
        }

        let resp: Value = self
            .http
            .post(&self.url)
            .json(&body)
            .send()
            .map_err(|e| GraphQLError::Http(e.to_string()))?
            .json()
            .map_err(|e| GraphQLError::Http(e.to_string()))?;

        if let Some(errors) = resp.get("errors")
            && let Some(arr) = errors.as_array()
            && !arr.is_empty()
        {
            let msg = arr[0]
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error");
            return Err(GraphQLError::Query(msg.to_string()));
        }

        Ok(resp.get("data").cloned().unwrap_or(Value::Null))
    }

    /// Get the stream URL for a track (for audio playback).
    pub fn stream_url(&self, track_id: i64) -> String {
        let base = self.url.trim_end_matches("/graphql");
        format!("{}/rest/stream?id={}", base, track_id)
    }

    // -----------------------------------------------------------------------
    // Typed helpers
    // -----------------------------------------------------------------------

    pub fn now_playing(&self) -> Result<NowPlaying, GraphQLError> {
        let data = self.execute(
            "{ nowPlaying { state positionMs durationMs queueItemId \
             track { title artist album codec sampleRate bitDepth channels durationMs } } }",
            None,
        )?;
        let np = &data["nowPlaying"];
        Ok(NowPlaying {
            state: np["state"].as_str().unwrap_or("STOPPED").to_string(),
            position_ms: np["positionMs"].as_u64().unwrap_or(0),
            duration_ms: np["durationMs"].as_u64(),
            queue_item_id: np["queueItemId"].as_str().map(String::from),
            track: np.get("track").and_then(|t| {
                if t.is_null() {
                    return None;
                }
                Some(NowPlayingTrack {
                    title: t["title"].as_str().unwrap_or("").to_string(),
                    artist: t["artist"].as_str().unwrap_or("").to_string(),
                    album: t["album"].as_str().unwrap_or("").to_string(),
                    codec: t["codec"].as_str().unwrap_or("").to_string(),
                    sample_rate: t["sampleRate"].as_u64().unwrap_or(0) as u32,
                    bit_depth: t["bitDepth"].as_u64().unwrap_or(0) as u16,
                    channels: t["channels"].as_u64().unwrap_or(0) as u16,
                    duration_ms: t["durationMs"].as_u64().unwrap_or(0),
                })
            }),
        })
    }

    pub fn queue(&self) -> Result<Vec<QueueEntry>, GraphQLError> {
        let data = self.execute(
            "{ queue { queueItemId title artist album codec trackNumber disc durationMs isCurrent } }",
            None,
        )?;
        let entries = data["queue"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|e| QueueEntry {
                        queue_item_id: e["queueItemId"].as_str().unwrap_or("").to_string(),
                        title: e["title"].as_str().unwrap_or("").to_string(),
                        artist: e["artist"].as_str().unwrap_or("").to_string(),
                        album: e["album"].as_str().unwrap_or("").to_string(),
                        codec: e["codec"].as_str().map(String::from),
                        track_number: e["trackNumber"].as_i64(),
                        disc: e["disc"].as_i64(),
                        duration_ms: e["durationMs"].as_u64(),
                        is_current: e["isCurrent"].as_bool().unwrap_or(false),
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(entries)
    }

    pub fn search(&self, query: &str, limit: u32) -> Result<Vec<TrackResult>, GraphQLError> {
        let data = self.execute(
            "query($search: String!, $first: Int) { tracks(search: $search, first: $first) { edges { node { id title artist album albumId artistId disc trackNumber durationMs codec genre source } } } }",
            Some(serde_json::json!({ "search": query, "first": limit })),
        )?;
        parse_track_edges(&data["tracks"])
    }

    pub fn artists(&self) -> Result<Vec<ArtistResult>, GraphQLError> {
        let data = self.execute("{ artists { edges { node { id name } } } }", None)?;
        let edges = data["artists"]["edges"].as_array();
        Ok(edges
            .map(|arr| {
                arr.iter()
                    .map(|e| {
                        let n = &e["node"];
                        ArtistResult {
                            id: n["id"].as_i64().unwrap_or(0),
                            name: n["name"].as_str().unwrap_or("").to_string(),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    pub fn albums_for_artist(&self, artist_id: i64) -> Result<Vec<AlbumResult>, GraphQLError> {
        let data = self.execute(
            "query($artistId: Int!) { albums(artistId: $artistId) { edges { node { id title artistName date codec } } } }",
            Some(serde_json::json!({ "artistId": artist_id })),
        )?;
        parse_album_edges(&data["albums"])
    }

    pub fn tracks_for_album(&self, album_id: i64) -> Result<Vec<TrackResult>, GraphQLError> {
        let data = self.execute(
            "query($albumId: Int!) { tracks(albumId: $albumId) { edges { node { id title artist album albumId artistId disc trackNumber durationMs codec genre source } } } }",
            Some(serde_json::json!({ "albumId": album_id })),
        )?;
        parse_track_edges(&data["tracks"])
    }

    pub fn fuzzy_search(
        &self,
        query: &str,
        kind: &str,
        limit: u32,
    ) -> Result<Vec<FuzzyMatch>, GraphQLError> {
        let data = self.execute(
            "query($query: String!, $kind: FuzzySearchKind!, $limit: Int) { fuzzySearch(query: $query, kind: $kind, limit: $limit) { id name rank kind } }",
            Some(serde_json::json!({ "query": query, "kind": kind, "limit": limit })),
        )?;
        Ok(data["fuzzySearch"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|e| FuzzyMatch {
                        id: e["id"].as_i64().unwrap_or(0),
                        name: e["name"].as_str().unwrap_or("").to_string(),
                        rank: e["rank"].as_i64().unwrap_or(0) as i32,
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    // -- Mutations --

    pub fn pause(&self) -> Result<(), GraphQLError> {
        self.execute("mutation { pause { ok } }", None)?;
        Ok(())
    }

    pub fn resume(&self) -> Result<(), GraphQLError> {
        self.execute("mutation { resume { ok } }", None)?;
        Ok(())
    }

    pub fn stop(&self) -> Result<(), GraphQLError> {
        self.execute("mutation { stop { ok } }", None)?;
        Ok(())
    }

    pub fn next(&self) -> Result<(), GraphQLError> {
        self.execute("mutation { next { ok } }", None)?;
        Ok(())
    }

    pub fn previous(&self) -> Result<(), GraphQLError> {
        self.execute("mutation { previous { ok } }", None)?;
        Ok(())
    }

    pub fn seek(&self, position_ms: u64) -> Result<(), GraphQLError> {
        self.execute(
            "mutation($positionMs: Int!) { seek(positionMs: $positionMs) { ok } }",
            Some(serde_json::json!({ "positionMs": position_ms })),
        )?;
        Ok(())
    }

    pub fn play(&self, queue_item_id: &str) -> Result<(), GraphQLError> {
        self.execute(
            "mutation($queueItemId: String!) { play(queueItemId: $queueItemId) { ok } }",
            Some(serde_json::json!({ "queueItemId": queue_item_id })),
        )?;
        Ok(())
    }

    pub fn add_to_queue(&self, track_ids: &[i64]) -> Result<Vec<String>, GraphQLError> {
        let data = self.execute(
            "mutation($trackIds: [Int!]!) { addToQueue(trackIds: $trackIds) { ok addedCount queueItemIds } }",
            Some(serde_json::json!({ "trackIds": track_ids })),
        )?;
        Ok(data["addToQueue"]["queueItemIds"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default())
    }

    pub fn replace_queue(&self, track_ids: &[i64]) -> Result<Vec<String>, GraphQLError> {
        let data = self.execute(
            "mutation($trackIds: [Int!]!) { replaceQueue(trackIds: $trackIds) { ok addedCount queueItemIds } }",
            Some(serde_json::json!({ "trackIds": track_ids })),
        )?;
        Ok(data["replaceQueue"]["queueItemIds"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default())
    }

    pub fn clear_queue(&self) -> Result<(), GraphQLError> {
        self.execute("mutation { clearQueue { ok } }", None)?;
        Ok(())
    }

    pub fn favourite(&self, track_id: i64) -> Result<(), GraphQLError> {
        self.execute(
            "mutation($trackId: Int!) { favourite(trackId: $trackId) { id } }",
            Some(serde_json::json!({ "trackId": track_id })),
        )?;
        Ok(())
    }

    pub fn unfavourite(&self, track_id: i64) -> Result<(), GraphQLError> {
        self.execute(
            "mutation($trackId: Int!) { unfavourite(trackId: $trackId) { id } }",
            Some(serde_json::json!({ "trackId": track_id })),
        )?;
        Ok(())
    }

    pub fn save_snapshot(&self, name: &str) -> Result<(), GraphQLError> {
        self.execute(
            "mutation($name: String!) { saveSnapshot(name: $name) { ok } }",
            Some(serde_json::json!({ "name": name })),
        )?;
        Ok(())
    }

    pub fn restore_snapshot(&self, name: &str) -> Result<(), GraphQLError> {
        self.execute(
            "mutation($name: String!) { restoreSnapshot(name: $name) { ok } }",
            Some(serde_json::json!({ "name": name })),
        )?;
        Ok(())
    }

    pub fn enable_radio(&self) -> Result<(), GraphQLError> {
        self.execute("mutation { enableRadio { ok } }", None)?;
        Ok(())
    }

    pub fn disable_radio(&self) -> Result<(), GraphQLError> {
        self.execute("mutation { disableRadio { ok } }", None)?;
        Ok(())
    }

    pub fn library_stats(&self) -> Result<Value, GraphQLError> {
        self.execute(
            "{ libraryStats { totalTracks totalArtists totalAlbums localTracks remoteTracks cachedTracks } }",
            None,
        )
    }

    /// Server URL (without /graphql path).
    pub fn server_url(&self) -> &str {
        self.url.trim_end_matches("/graphql")
    }
}

// ---------------------------------------------------------------------------
// Result types
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum GraphQLError {
    #[error("http error: {0}")]
    Http(String),
    #[error("query error: {0}")]
    Query(String),
}

#[derive(Debug, Clone)]
pub struct NowPlaying {
    pub state: String,
    pub position_ms: u64,
    pub duration_ms: Option<u64>,
    pub queue_item_id: Option<String>,
    pub track: Option<NowPlayingTrack>,
}

#[derive(Debug, Clone)]
pub struct NowPlayingTrack {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub codec: String,
    pub sample_rate: u32,
    pub bit_depth: u16,
    pub channels: u16,
    pub duration_ms: u64,
}

#[derive(Debug, Clone)]
pub struct QueueEntry {
    pub queue_item_id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub codec: Option<String>,
    pub track_number: Option<i64>,
    pub disc: Option<i64>,
    pub duration_ms: Option<u64>,
    pub is_current: bool,
}

#[derive(Debug, Clone)]
pub struct TrackResult {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_id: Option<i64>,
    pub artist_id: Option<i64>,
    pub disc: Option<i32>,
    pub track_number: Option<i32>,
    pub duration_ms: Option<i64>,
    pub codec: Option<String>,
    pub genre: Option<String>,
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct ArtistResult {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct AlbumResult {
    pub id: i64,
    pub title: String,
    pub artist_name: String,
    pub date: Option<String>,
    pub codec: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FuzzyMatch {
    pub id: i64,
    pub name: String,
    pub rank: i32,
}

// ---------------------------------------------------------------------------
// Parse helpers
// ---------------------------------------------------------------------------

fn parse_track_edges(connection: &Value) -> Result<Vec<TrackResult>, GraphQLError> {
    Ok(connection["edges"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|e| {
                    let n = &e["node"];
                    TrackResult {
                        id: n["id"].as_i64().unwrap_or(0),
                        title: n["title"].as_str().unwrap_or("").to_string(),
                        artist: n["artist"].as_str().unwrap_or("").to_string(),
                        album: n["album"].as_str().unwrap_or("").to_string(),
                        album_id: n["albumId"].as_i64(),
                        artist_id: n["artistId"].as_i64(),
                        disc: n["disc"].as_i64().map(|v| v as i32),
                        track_number: n["trackNumber"].as_i64().map(|v| v as i32),
                        duration_ms: n["durationMs"].as_i64(),
                        codec: n["codec"].as_str().map(String::from),
                        genre: n["genre"].as_str().map(String::from),
                        source: n["source"].as_str().unwrap_or("local").to_string(),
                    }
                })
                .collect()
        })
        .unwrap_or_default())
}

fn parse_album_edges(connection: &Value) -> Result<Vec<AlbumResult>, GraphQLError> {
    Ok(connection["edges"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|e| {
                    let n = &e["node"];
                    AlbumResult {
                        id: n["id"].as_i64().unwrap_or(0),
                        title: n["title"].as_str().unwrap_or("").to_string(),
                        artist_name: n["artistName"].as_str().unwrap_or("").to_string(),
                        date: n["date"].as_str().map(String::from),
                        codec: n["codec"].as_str().map(String::from),
                    }
                })
                .collect()
        })
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_constructs_url() {
        let c = GraphQLClient::new("http://localhost:4000");
        assert_eq!(c.url, "http://localhost:4000/graphql");
    }

    #[test]
    fn client_trailing_slash() {
        let c = GraphQLClient::new("http://localhost:4000/");
        assert_eq!(c.url, "http://localhost:4000/graphql");
    }

    #[test]
    fn stream_url_format() {
        let c = GraphQLClient::new("http://localhost:4000");
        assert_eq!(c.stream_url(42), "http://localhost:4000/rest/stream?id=42");
    }
}
