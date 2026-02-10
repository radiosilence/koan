uniffi::setup_scaffolding!();

// -- UniFFI exports (control plane) --

#[uniffi::export]
fn koan_version() -> String {
    "0.1.0".to_string()
}

// -- C FFI exports (audio data plane) --
// These bypass UniFFI for zero-overhead access from Swift.

mod audio_ffi;
