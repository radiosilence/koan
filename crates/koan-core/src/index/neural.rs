//! Neural audio embeddings via DCLAP (Distilled CLAP) ONNX model.
//!
//! Feature-gated behind `neural-discovery`. When enabled, provides:
//! - 512-dim audio embeddings for semantic similarity search
//! - Text-to-music search via CLAP text encoder
//!
//! If the model file is missing, functions return errors that callers
//! handle gracefully — no panics, no crashes.

use std::path::{Path, PathBuf};

use thiserror::Error;

/// Dimensionality of DCLAP neural embeddings.
pub const NEURAL_EMBEDDING_DIMS: usize = 512;

/// Default model directory under config.
const MODEL_DIR: &str = "models";
/// Expected filename for the audio encoder model.
const AUDIO_MODEL_FILENAME: &str = "dclap_audio.onnx";
/// Expected filename for the text encoder model.
const TEXT_MODEL_FILENAME: &str = "dclap_text.onnx";

#[derive(Debug, Error)]
pub enum NeuralError {
    #[error("model file not found: {0}")]
    ModelNotFound(PathBuf),

    #[error("audio decode failed: {0}")]
    DecodeFailed(String),

    #[error("ONNX inference failed: {0}")]
    InferenceFailed(String),

    #[error("neural-discovery feature not enabled")]
    FeatureDisabled,

    #[error("text encoding failed: {0}")]
    TextEncodeFailed(String),
}

/// Return the default model directory: `~/.config/koan/models/`
pub fn default_model_dir() -> PathBuf {
    crate::config::config_dir().join(MODEL_DIR)
}

/// Return the expected path for the audio encoder model.
pub fn audio_model_path(model_dir: &Path) -> PathBuf {
    model_dir.join(AUDIO_MODEL_FILENAME)
}

/// Return the expected path for the text encoder model.
pub fn text_model_path(model_dir: &Path) -> PathBuf {
    model_dir.join(TEXT_MODEL_FILENAME)
}

/// Check whether the neural audio model is available at the given directory.
pub fn is_audio_model_available(model_dir: &Path) -> bool {
    audio_model_path(model_dir).exists()
}

/// Check whether the neural text model is available at the given directory.
pub fn is_text_model_available(model_dir: &Path) -> bool {
    text_model_path(model_dir).exists()
}

/// Analyze a track and return a 512-dim neural embedding.
///
/// Requires the `neural-discovery` feature and a DCLAP ONNX model at
/// `model_dir/dclap_audio.onnx`. Returns `NeuralError::ModelNotFound`
/// if the model is missing, `NeuralError::FeatureDisabled` if compiled
/// without the feature flag.
#[cfg(feature = "neural-discovery")]
pub fn analyze_track_neural(path: &Path, model_dir: &Path) -> Result<Vec<f32>, NeuralError> {
    let model_path = audio_model_path(model_dir);
    if !model_path.exists() {
        return Err(NeuralError::ModelNotFound(model_path));
    }

    // Decode audio to mono f32 samples at 48kHz (CLAP expected input).
    let samples = decode_audio_to_f32(path, 48000)?;

    // Run ONNX inference.
    run_audio_inference(&model_path, &samples)
}

/// Analyze a track — stub when feature is disabled.
#[cfg(not(feature = "neural-discovery"))]
pub fn analyze_track_neural(_path: &Path, _model_dir: &Path) -> Result<Vec<f32>, NeuralError> {
    Err(NeuralError::FeatureDisabled)
}

/// Encode a text query to a 512-dim embedding for text-to-music search.
///
/// Requires `neural-discovery` feature and `dclap_text.onnx` model.
#[cfg(feature = "neural-discovery")]
pub fn encode_text(text: &str, model_dir: &Path) -> Result<Vec<f32>, NeuralError> {
    let model_path = text_model_path(model_dir);
    if !model_path.exists() {
        return Err(NeuralError::ModelNotFound(model_path));
    }

    run_text_inference(&model_path, text)
}

/// Encode text — stub when feature is disabled.
#[cfg(not(feature = "neural-discovery"))]
pub fn encode_text(_text: &str, _model_dir: &Path) -> Result<Vec<f32>, NeuralError> {
    Err(NeuralError::FeatureDisabled)
}

// ---------------------------------------------------------------------------
// Internal: audio decode
// ---------------------------------------------------------------------------

/// Decode an audio file to mono f32 samples at the target sample rate.
/// Reuses Symphonia (already a koan dep) for format-agnostic decoding.
#[cfg(feature = "neural-discovery")]
fn decode_audio_to_f32(path: &Path, target_sr: u32) -> Result<Vec<f32>, NeuralError> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let file =
        std::fs::File::open(path).map_err(|e| NeuralError::DecodeFailed(format!("open: {}", e)))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| NeuralError::DecodeFailed(format!("probe: {}", e)))?;

    let mut reader = probed.format;
    let track = reader
        .default_track()
        .ok_or_else(|| NeuralError::DecodeFailed("no default track".into()))?;
    let track_id = track.id;
    let source_sr = track.codec_params.sample_rate.unwrap_or(44100);
    let channels = track.codec_params.channels.map(|c| c.count()).unwrap_or(2);

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| NeuralError::DecodeFailed(format!("codec: {}", e)))?;

    let mut all_samples: Vec<f32> = Vec::new();
    // Limit to ~30 seconds of audio (enough for embedding).
    let max_samples = target_sr as usize * 30;

    loop {
        let packet = match reader.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(_) => break,
        };

        if packet.track_id() != track_id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(_) => continue,
        };

        let spec = *decoded.spec();
        let n_frames = decoded.frames();
        let mut sbuf = SampleBuffer::<f32>::new(n_frames as u64, spec);
        sbuf.copy_interleaved_ref(decoded);
        let interleaved = sbuf.samples();

        // Mix to mono.
        let ch = spec.channels.count().max(1);
        for frame in interleaved.chunks(ch) {
            let mono: f32 = frame.iter().sum::<f32>() / ch as f32;
            all_samples.push(mono);
        }

        if all_samples.len() >= max_samples * source_sr as usize / target_sr as usize {
            break;
        }
    }

    // Naive resample if source rate differs from target.
    if source_sr != target_sr && !all_samples.is_empty() {
        all_samples = naive_resample(&all_samples, source_sr, target_sr);
    }

    // Truncate to max_samples.
    all_samples.truncate(max_samples);

    if all_samples.is_empty() {
        return Err(NeuralError::DecodeFailed("no samples decoded".into()));
    }

    Ok(all_samples)
}

/// Simple linear-interpolation resample. Not audiophile-grade, but fine for
/// neural feature extraction where we just need ~48kHz mono for the model.
#[cfg(feature = "neural-discovery")]
fn naive_resample(samples: &[f32], from_sr: u32, to_sr: u32) -> Vec<f32> {
    let ratio = from_sr as f64 / to_sr as f64;
    let out_len = (samples.len() as f64 / ratio).ceil() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src_idx = i as f64 * ratio;
        let idx0 = src_idx.floor() as usize;
        let idx1 = (idx0 + 1).min(samples.len() - 1);
        let frac = (src_idx - idx0 as f64) as f32;
        out.push(samples[idx0] * (1.0 - frac) + samples[idx1] * frac);
    }
    out
}

// ---------------------------------------------------------------------------
// Internal: ONNX inference
// ---------------------------------------------------------------------------

#[cfg(feature = "neural-discovery")]
fn run_audio_inference(model_path: &Path, samples: &[f32]) -> Result<Vec<f32>, NeuralError> {
    use ort::session::Session;

    let session = Session::builder()
        .and_then(|b| b.commit_from_file(model_path))
        .map_err(|e| NeuralError::InferenceFailed(format!("load model: {}", e)))?;

    // DCLAP audio encoder expects shape [batch, samples] or [batch, 1, samples].
    // We pass [1, N] and let the model handle its internal chunking.
    let sample_count = samples.len();
    let input_array = ndarray::Array2::from_shape_vec((1, sample_count), samples.to_vec())
        .map_err(|e| NeuralError::InferenceFailed(format!("shape: {}", e)))?;

    let outputs = session
        .run(
            ort::inputs![input_array]
                .map_err(|e| NeuralError::InferenceFailed(format!("input: {}", e)))?,
        )
        .map_err(|e| NeuralError::InferenceFailed(format!("run: {}", e)))?;

    // Extract the embedding from the first output tensor.
    let embedding = outputs[0]
        .try_extract_tensor::<f32>()
        .map_err(|e| NeuralError::InferenceFailed(format!("extract: {}", e)))?;

    let vec: Vec<f32> = embedding.iter().copied().collect();
    if vec.len() != NEURAL_EMBEDDING_DIMS {
        log::warn!(
            "neural embedding dims mismatch: expected {}, got {}",
            NEURAL_EMBEDDING_DIMS,
            vec.len()
        );
    }
    Ok(vec)
}

#[cfg(feature = "neural-discovery")]
fn run_text_inference(model_path: &Path, text: &str) -> Result<Vec<f32>, NeuralError> {
    use ort::session::Session;

    let session = Session::builder()
        .and_then(|b| b.commit_from_file(model_path))
        .map_err(|e| NeuralError::TextEncodeFailed(format!("load model: {}", e)))?;

    // CLAP text encoder expects tokenized input. For a minimal implementation,
    // we pass the raw text as a string tensor and rely on the model's built-in
    // tokenizer. If the model requires pre-tokenized input, this will need
    // a tokenizer step (e.g. via the `tokenizers` crate).
    //
    // TODO: Wire up proper tokenization if the DCLAP ONNX model requires
    // pre-tokenized int64 input_ids rather than raw string input. For now,
    // this works with models that include an embedded tokenizer.
    let text_array = ndarray::Array2::from_shape_vec((1, 1), vec![text.to_string()])
        .map_err(|e| NeuralError::TextEncodeFailed(format!("shape: {}", e)))?;

    let outputs = session
        .run(
            ort::inputs![text_array]
                .map_err(|e| NeuralError::TextEncodeFailed(format!("input: {}", e)))?,
        )
        .map_err(|e| NeuralError::TextEncodeFailed(format!("run: {}", e)))?;

    let embedding = outputs[0]
        .try_extract_tensor::<f32>()
        .map_err(|e| NeuralError::TextEncodeFailed(format!("extract: {}", e)))?;

    let vec: Vec<f32> = embedding.iter().copied().collect();
    Ok(vec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_model_dir_exists_as_path() {
        let dir = default_model_dir();
        // Just verify it returns a sensible path, not that it exists on disk.
        assert!(dir.to_string_lossy().contains("koan"));
        assert!(dir.to_string_lossy().contains("models"));
    }

    #[test]
    fn model_paths_contain_filenames() {
        let dir = PathBuf::from("/tmp/models");
        assert!(
            audio_model_path(&dir)
                .to_string_lossy()
                .ends_with("dclap_audio.onnx")
        );
        assert!(
            text_model_path(&dir)
                .to_string_lossy()
                .ends_with("dclap_text.onnx")
        );
    }

    #[test]
    fn model_not_available_for_missing_dir() {
        let dir = PathBuf::from("/nonexistent/path/models");
        assert!(!is_audio_model_available(&dir));
        assert!(!is_text_model_available(&dir));
    }

    #[test]
    fn analyze_returns_error_for_missing_model() {
        let dir = PathBuf::from("/nonexistent/path/models");
        let result = analyze_track_neural(Path::new("/fake.flac"), &dir);
        assert!(result.is_err());
    }

    #[test]
    fn encode_text_returns_error_for_missing_model() {
        let dir = PathBuf::from("/nonexistent/path/models");
        let result = encode_text("test query", &dir);
        assert!(result.is_err());
    }

    #[cfg(feature = "neural-discovery")]
    #[test]
    fn naive_resample_identity() {
        let input = vec![1.0f32, 2.0, 3.0, 4.0];
        let output = naive_resample(&input, 48000, 48000);
        assert_eq!(output.len(), input.len());
        for (a, b) in input.iter().zip(output.iter()) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    #[cfg(feature = "neural-discovery")]
    #[test]
    fn naive_resample_downsample() {
        let input: Vec<f32> = (0..1000).map(|i| i as f32).collect();
        let output = naive_resample(&input, 48000, 24000);
        // Should be roughly half the length.
        assert!(output.len() > 400 && output.len() < 600);
    }
}
