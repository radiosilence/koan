- **bliss-audio no longer needs the aubio C library**; upstream replaced it with a Rust implementation, so `bliss-audio-aubio-rs` and its bindgen build are gone. The feature vector is unchanged (`FeaturesVersion::Version2`), so stored embeddings stay valid, though decoded values may shift marginally after bliss's Symphonia/Rubato update.

