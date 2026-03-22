//! Acoustic feature extraction using bliss-audio.
//!
//! Extracts a feature vector per track (tempo, timbre, chroma,
//! spectral features) for acoustic similarity search. bliss-audio v2
//! produces 23 dimensions; we use whatever the current bliss version
//! exports as NUMBER_FEATURES.

use std::path::Path;

use thiserror::Error;

/// Number of dimensions in the acoustic feature vector (matches bliss-audio).
pub const EMBEDDING_DIMS: usize = bliss_audio::NUMBER_FEATURES;

#[derive(Debug, Error)]
pub enum AnalysisError {
    #[error("bliss analysis failed: {0}")]
    Bliss(String),
}

/// Analyze a track and return its acoustic feature vector.
///
/// Uses bliss-audio's Symphonia-based decoder to extract tempo, timbre,
/// chroma, and spectral features. Takes ~0.4s per track on average.
pub fn analyze_track(path: &Path) -> Result<Vec<f32>, AnalysisError> {
    use bliss_audio::decoder::Decoder as _;
    use bliss_audio::decoder::symphonia::SymphoniaDecoder;

    let song =
        SymphoniaDecoder::song_from_path(path).map_err(|e| AnalysisError::Bliss(e.to_string()))?;
    Ok(song.analysis.as_vec())
}

/// Serialize a float vector to bytes for BLOB storage. Little-endian f32.
pub fn embedding_to_bytes(embedding: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(embedding.len() * 4);
    for &val in embedding {
        bytes.extend_from_slice(&val.to_le_bytes());
    }
    bytes
}

/// Deserialize bytes from BLOB storage back to a float vector.
pub fn bytes_to_embedding(bytes: &[u8]) -> Option<Vec<f32>> {
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let count = bytes.len() / 4;
    let mut embedding = Vec::with_capacity(count);
    for chunk in bytes.chunks_exact(4) {
        embedding.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
    }
    Some(embedding)
}

/// Compute euclidean distance between two embedding vectors.
/// Vectors must be the same length.
pub fn euclidean_distance(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (x - y) * (x - y))
        .sum::<f32>()
        .sqrt()
}

/// Compute the centroid (mean) of multiple embedding vectors.
pub fn centroid(embeddings: &[Vec<f32>]) -> Vec<f32> {
    if embeddings.is_empty() {
        return vec![0.0; EMBEDDING_DIMS];
    }
    let dims = embeddings[0].len();
    let mut result = vec![0.0f32; dims];
    let count = embeddings.len() as f32;
    for emb in embeddings {
        for (i, &val) in emb.iter().enumerate() {
            result[i] += val;
        }
    }
    for val in &mut result {
        *val /= count;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedding_serialization_roundtrip() {
        let embedding: Vec<f32> = (0..EMBEDDING_DIMS)
            .map(|i| (i as f32) * 1.5 - 10.0)
            .collect();
        let bytes = embedding_to_bytes(&embedding);
        let recovered = bytes_to_embedding(&bytes).unwrap();
        assert_eq!(embedding, recovered);
    }

    #[test]
    fn bytes_to_embedding_wrong_length() {
        // Not a multiple of 4
        assert!(bytes_to_embedding(&[0u8; 10]).is_none());
        // Empty is valid (0 floats)
        assert_eq!(bytes_to_embedding(&[]).unwrap().len(), 0);
    }

    #[test]
    fn euclidean_distance_identical() {
        let a = vec![1.0f32; EMBEDDING_DIMS];
        assert!((euclidean_distance(&a, &a) - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn euclidean_distance_known() {
        let mut a = vec![0.0f32; EMBEDDING_DIMS];
        let mut b = vec![0.0f32; EMBEDDING_DIMS];
        a[0] = 3.0;
        b[0] = 0.0;
        a[1] = 0.0;
        b[1] = 4.0;
        // sqrt(9 + 16) = 5.0
        assert!((euclidean_distance(&a, &b) - 5.0).abs() < 1e-6);
    }

    #[test]
    fn centroid_single() {
        let emb = vec![42.0f32; EMBEDDING_DIMS];
        let result = centroid(&[emb.clone()]);
        assert_eq!(result, emb);
    }

    #[test]
    fn centroid_multiple() {
        let a = vec![2.0f32; EMBEDDING_DIMS];
        let b = vec![4.0f32; EMBEDDING_DIMS];
        let result = centroid(&[a, b]);
        for val in result {
            assert!((val - 3.0).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn centroid_empty() {
        let result = centroid(&[]);
        assert_eq!(result.len(), EMBEDDING_DIMS);
        for val in result {
            assert!((val - 0.0).abs() < f32::EPSILON);
        }
    }
}
