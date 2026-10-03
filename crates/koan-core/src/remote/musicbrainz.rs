//! MusicBrainz API client — artist search, Wikidata links, release credits.
//!
//! Uses `musicbrainz.org/ws/2/` with JSON format. No API key required.
//! Rate limit: 1 request per second (enforced by caller, not this module).

use serde::Deserialize;
use thiserror::Error;

const MB_BASE: &str = "https://musicbrainz.org/ws/2";
const USER_AGENT: &str = concat!(
    "koan/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/radiosilence/koan)"
);

#[derive(Debug, Error)]
pub enum MusicBrainzError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("artist not found")]
    NotFound,
    #[error("rate limited")]
    RateLimited,
}

/// Search result from MusicBrainz artist search.
#[derive(Debug, Clone)]
pub struct ArtistSearchResult {
    pub name: String,
    pub mbid: String,
    pub score: u32,
}

// --- Deserialization types ---

#[derive(Debug, Deserialize)]
struct ArtistSearchResponse {
    #[serde(default)]
    artists: Vec<MbArtist>,
}

#[derive(Debug, Deserialize)]
struct MbArtist {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    score: Option<u32>,
    #[serde(default)]
    relations: Vec<MbRelation>,
}

#[derive(Debug, Deserialize)]
struct MbRelation {
    #[serde(rename = "type")]
    relation_type: Option<String>,
    #[serde(default)]
    url: Option<MbUrl>,
}

#[derive(Debug, Deserialize)]
struct MbUrl {
    #[serde(default)]
    resource: String,
}

#[derive(Debug, Deserialize)]
struct MbRelease {
    #[serde(rename = "artist-credit", default)]
    artist_credit: Vec<MbCredit>,
}

#[derive(Debug, Deserialize)]
struct MbCredit {
    artist: MbRelatedArtist,
}

#[derive(Debug, Deserialize)]
struct MbRelatedArtist {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
}

/// Search for an artist by name. Returns up to `limit` results sorted by match score.
pub fn search_artist(
    http: &reqwest::blocking::Client,
    artist_name: &str,
    limit: usize,
) -> Result<Vec<ArtistSearchResult>, MusicBrainzError> {
    // Quoted, so a name of several words is searched as a phrase. Unquoted,
    // `artist:Azure Ray` is Azure or Ray, and Ray Charles scores highest.
    let query = format!("artist:\"{}\"", artist_name.replace(['\\', '"'], ""));
    let resp = http
        .get(format!("{MB_BASE}/artist"))
        .query(&[
            ("query", query.as_str()),
            ("fmt", "json"),
            ("limit", &limit.to_string()),
        ])
        .header("User-Agent", USER_AGENT)
        .send()?;

    if resp.status().as_u16() == 503 {
        return Err(MusicBrainzError::RateLimited);
    }
    if !resp.status().is_success() {
        return Err(MusicBrainzError::NotFound);
    }

    let data: ArtistSearchResponse = resp.json()?;
    Ok(data
        .artists
        .into_iter()
        .map(|a| ArtistSearchResult {
            name: a.name,
            mbid: a.id,
            score: a.score.unwrap_or(0),
        })
        .collect())
}

/// The Wikidata item an artist is linked to, as its id (`Q2358013`).
pub fn wikidata_id(
    http: &reqwest::blocking::Client,
    artist_mbid: &str,
) -> Result<Option<String>, MusicBrainzError> {
    let url = format!("{MB_BASE}/artist/{artist_mbid}?inc=url-rels&fmt=json");
    let resp = http.get(&url).header("User-Agent", USER_AGENT).send()?;
    match resp.status().as_u16() {
        503 => return Err(MusicBrainzError::RateLimited),
        404 => return Err(MusicBrainzError::NotFound),
        _ => {}
    }
    let artist: MbArtist = resp.error_for_status()?.json()?;
    Ok(artist
        .relations
        .into_iter()
        .filter(|rel| rel.relation_type.as_deref() == Some("wikidata"))
        .find_map(|rel| {
            let resource = rel.url?.resource;
            let id = resource.rsplit('/').next()?;
            id.starts_with('Q').then(|| id.to_string())
        }))
}

/// The artists a release is credited to, as `(name, mbid)`.
pub fn release_artists(
    http: &reqwest::blocking::Client,
    release_mbid: &str,
) -> Result<Vec<(String, String)>, MusicBrainzError> {
    let url = format!("{MB_BASE}/release/{release_mbid}?inc=artist-credits&fmt=json");
    let resp = http.get(&url).header("User-Agent", USER_AGENT).send()?;
    match resp.status().as_u16() {
        503 => return Err(MusicBrainzError::RateLimited),
        404 => return Err(MusicBrainzError::NotFound),
        _ => {}
    }
    let release: MbRelease = resp.error_for_status()?.json()?;
    Ok(release
        .artist_credit
        .into_iter()
        .map(|credit| (credit.artist.name, credit.artist.id))
        .collect())
}

/// An HTTP client carrying the User-Agent MusicBrainz requires.
pub fn default_client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .unwrap_or_else(|_| reqwest::blocking::Client::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_search_response() {
        let json = r#"{
            "artists": [
                {"id": "f22942a1-6f70-4f48-866e-238cb2308fbd", "name": "Aphex Twin", "score": 100},
                {"id": "abc-123", "name": "Aphex Twin Tribute", "score": 60}
            ]
        }"#;
        let resp: ArtistSearchResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.artists.len(), 2);
        assert_eq!(resp.artists[0].name, "Aphex Twin");
        assert_eq!(resp.artists[0].score, Some(100));
    }
}
