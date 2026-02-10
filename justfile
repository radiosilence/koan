# koan — bit-perfect macOS music player

# Build everything (Rust → bindings → xcframework → Swift)
build: build-rust generate-bindings build-xcframework build-swift

# Rust workspace release build
build-rust:
    cargo build --release --workspace

# Debug build (faster iteration)
build-debug:
    cargo build --workspace

# Generate Swift bindings (UniFFI + cbindgen)
generate-bindings:
    mkdir -p swift/Generated
    cargo run -p koan-ffi --bin uniffi-bindgen generate \
        --library target/release/libkoan_ffi.a \
        --language swift \
        --out-dir swift/Generated
    cbindgen --config crates/koan-ffi/cbindgen.toml \
        --crate koan-ffi \
        --output swift/Generated/koan_audio_ffi.h

# Create xcframework for Swift package
build-xcframework:
    #!/usr/bin/env bash
    set -euo pipefail
    rm -rf swift/KoanRust.xcframework
    TEMP=$(mktemp -d)
    HEADERS="$TEMP/Headers"
    mkdir -p "$HEADERS"
    cp swift/Generated/koan_audio_ffi.h "$HEADERS/"
    [ -f swift/Generated/KoanFFIFFI.h ] && cp swift/Generated/KoanFFIFFI.h "$HEADERS/"
    cat > "$HEADERS/module.modulemap" << 'EOF'
    module KoanRust {
        header "koan_audio_ffi.h"
        header "KoanFFIFFI.h"
        export *
    }
    EOF
    xcodebuild -create-xcframework \
        -library target/release/libkoan_ffi.a \
        -headers "$HEADERS" \
        -output swift/KoanRust.xcframework
    rm -rf "$TEMP"

# Build Swift package
build-swift:
    cd swift && swift build

# Run the Swift app
run: build
    cd swift && swift run

# Build + run CLI in release mode
cli *ARGS:
    cargo run --release -p koan-cli -- {{ARGS}}

# Run tests + clippy
check:
    cargo test --workspace
    cargo clippy --workspace -- -D warnings

# Format
fmt:
    cargo fmt

# Clean all build artifacts
clean:
    cargo clean
    rm -rf swift/.build
    rm -rf swift/KoanRust.xcframework
    rm -rf swift/Generated
