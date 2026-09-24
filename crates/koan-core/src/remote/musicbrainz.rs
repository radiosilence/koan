//! MusicBrainz API client — artist search and relationship graph.
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

/// An artist relationship from MusicBrainz.
#[derive(Debug, Clone)]
pub struct ArtistRelation {
    /// Name of the related artist.
    pub name: String,
    /// MBID of the related artist.
    pub mbid: String,
    /// Relationship type: "member of band", "collaboration", "associated act", etc.
    pub relation_type: String,
    /// Simplified category: "member", "collaborator", "associated".
    pub category: RelationCategory,
}

/// Simplified relationship categories for scoring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RelationCategory {
    Member,
    Collaborator,
    Associated,
}

impl RelationCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Member => "member",
            Self::Collaborator => "collaborator",
            Self::Associated => "associated",
        }
    }
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
    artist: Option<MbRelatedArtist>,
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

/// Build an HTTP client with the required MusicBrainz User-Agent.
fn mb_client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .unwrap_or_else(|_| reqwest::blocking::Client::new())
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

/// Look up an artist's MBID by exact name match. Returns the best match if score >= 90.
pub fn lookup_artist_mbid(
    http: &reqwest::blocking::Client,
    artist_name: &str,
) -> Result<Option<String>, MusicBrainzError> {
    let results = search_artist(http, artist_name, 3)?;
    Ok(results.into_iter().find(|r| r.score >= 90).map(|r| r.mbid))
}

/// Fetch artist relationships (collaborators, band members, associated acts) by MBID.
pub fn get_artist_relations(
    http: &reqwest::blocking::Client,
    artist_mbid: &str,
) -> Result<Vec<ArtistRelation>, MusicBrainzError> {
    let url = format!(
        "{}/artist/{}?inc=artist-rels&fmt=json",
        MB_BASE, artist_mbid
    );
    let resp = http.get(&url).header("User-Agent", USER_AGENT).send()?;

    if resp.status().as_u16() == 503 {
        return Err(MusicBrainzError::RateLimited);
    }
    if resp.status().as_u16() == 404 {
        return Err(MusicBrainzError::NotFound);
    }

    let artist: MbArtist = resp.json()?;
    let mut relations = Vec::new();

    for rel in artist.relations {
        if let (Some(rel_type), Some(related)) = (rel.relation_type, rel.artist) {
            if related.id == artist_mbid {
                continue; // Skip self-references.
            }
            let category = categorize_relation(&rel_type);
            relations.push(ArtistRelation {
                name: related.name,
                mbid: related.id,
                relation_type: rel_type,
                category,
            });
        }
    }

    Ok(relations)
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

/// Categorize a MusicBrainz relation type string into a simplified category.
fn categorize_relation(rel_type: &str) -> RelationCategory {
    let lower = rel_type.to_lowercase();
    if lower.contains("member") || lower.contains("part of") {
        RelationCategory::Member
    } else if lower.contains("collaborat")
        || lower.contains("remix")
        || lower.contains("producer")
        || lower.contains("performing")
        || lower.contains("instrument")
        || lower.contains("vocal")
    {
        RelationCategory::Collaborator
    } else {
        RelationCategory::Associated
    }
}

/// Create a default HTTP client suitable for MusicBrainz API calls.
pub fn default_client() -> reqwest::blocking::Client {
    mb_client()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_categorize_relation_member() {
        assert_eq!(
            categorize_relation("member of band"),
            RelationCategory::Member
        );
        assert_eq!(categorize_relation("is part of"), RelationCategory::Member);
    }

    #[test]
    fn test_categorize_relation_collaborator() {
        assert_eq!(
            categorize_relation("collaboration"),
            RelationCategory::Collaborator
        );
        assert_eq!(categorize_relation("remix"), RelationCategory::Collaborator);
        assert_eq!(
            categorize_relation("producer"),
            RelationCategory::Collaborator
        );
    }

    #[test]
    fn test_categorize_relation_associated() {
        assert_eq!(categorize_relation("tribute"), RelationCategory::Associated);
        assert_eq!(
            categorize_relation("support act"),
            RelationCategory::Associated
        );
    }

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

    #[test]
    fn test_deserialize_artist_with_relations() {
        let json = r#"{
            "id": "f22942a1-6f70-4f48-866e-238cb2308fbd",
            "name": "Aphex Twin",
            "relations": [
                {
                    "type": "collaboration",
                    "artist": {"id": "abc-123", "name": "µ-Ziq"}
                },
                {
                    "type": "member of band",
                    "artist": {"id": "def-456", "name": "Universal Indicator"}
                }
            ]
        }"#;
        let artist: MbArtist = serde_json::from_str(json).unwrap();
        assert_eq!(artist.relations.len(), 2);
        assert_eq!(
            artist.relations[0].relation_type.as_deref(),
            Some("collaboration")
        );
        assert_eq!(artist.relations[0].artist.as_ref().unwrap().name, "µ-Ziq");
    }
}
