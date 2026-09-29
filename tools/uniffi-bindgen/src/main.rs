//! koan-ffi's bindings generator: `just ffi-bindings`.
//!
//! A crate of its own so that running it builds uniffi and nothing else. As a
//! binary of koan-ffi it built the whole engine first, for the host, even when
//! the library it reads was built for iOS.
fn main() {
    uniffi::uniffi_bindgen_main()
}
