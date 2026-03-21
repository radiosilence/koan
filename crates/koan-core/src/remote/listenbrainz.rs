//! ListenBrainz Labs API client — similar artists via ML-based similarity.
//!
//! Uses `labs.api.listenbrainz.org` which requires no API key.
//! Rate limits are dynamic and generous.

use serde::Deserialize;
use thiserror::Error;

const LABS_BASE: &str = "https://labs.api.listenbrainz.org/similar-artists/json";

#[derive(Debug, Error)]
pub enum ListenBrainzError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("no results for artist")]
    NoResults,
}

/// A similar artist result from ListenBrainz.
#[derive(Debug, Clone)]
pub struct SimilarArtist {
    pub name: String,
    pub mbid: String,
    pub score: f64,
}

/// Raw response row from the labs API.
#[derive(Debug, Deserialize)]
struct LabsRow {
    #[serde(default)]
    artist_credit_name: Option<String>,
    #[serde(default)]
    artist_mbid: Option<String>,
    #[serde(default)]
    score: Option<f64>,
}

/// Fetch similar artists for an artist MBID from ListenBrainz Labs.
///
/// Returns up to `limit` similar artists sorted by score descending.
pub fn get_similar_artists(
    http: &reqwest::blocking::Client,
    artist_mbid: &str,
    limit: usize,
) -> Result<Vec<SimilarArtist>, ListenBrainzError> {
    let url = format!("{}?artist_mbids={}", LABS_BASE, artist_mbid);
    let resp: Vec<Vec<LabsRow>> = http.get(&url).send()?.json()?;

    let mut results = Vec::new();
    for row_set in &resp {
        for row in row_set {
            if let (Some(name), Some(mbid), Some(score)) =
                (&row.artist_credit_name, &row.artist_mbid, row.score)
            {
                // Skip the seed artist itself.
                if mbid != artist_mbid {
                    results.push(SimilarArtist {
                        name: name.clone(),
                        mbid: mbid.clone(),
                        score,
                    });
                }
            }
        }
    }

    if results.is_empty() {
        return Err(ListenBrainzError::NoResults);
    }

    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results.truncate(limit);
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_labs_row() {
        let json = r#"{"artist_credit_name": "Boards of Canada", "artist_mbid": "69158f97-4c07-4c4e-baf8-4e4ab1ed666e", "score": 0.85, "reference_mbid": "f22942a1-6f70-4f48-866e-238cb2308fbd"}"#;
        let row: LabsRow = serde_json::from_str(json).unwrap();
        assert_eq!(row.artist_credit_name.as_deref(), Some("Boards of Canada"));
        assert!((row.score.unwrap() - 0.85).abs() < f64::EPSILON);
    }

    #[test]
    fn test_parse_empty_row() {
        let json = r#"{}"#;
        let row: LabsRow = serde_json::from_str(json).unwrap();
        assert!(row.artist_credit_name.is_none());
        assert!(row.score.is_none());
    }
}
