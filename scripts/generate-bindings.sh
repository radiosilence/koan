#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."

GENERATED_DIR="swift/Generated"
mkdir -p "$GENERATED_DIR"

echo "==> Generating UniFFI Swift bindings..."
cargo run -p koan-ffi --bin uniffi-bindgen generate \
    --library target/release/libkoan_ffi.a \
    --language swift \
    --out-dir "$GENERATED_DIR"

echo "==> Generating C headers via cbindgen..."
cbindgen --config crates/koan-ffi/cbindgen.toml \
    --crate koan-ffi \
    --output "$GENERATED_DIR/koan_audio_ffi.h"

echo "==> Bindings generated in $GENERATED_DIR"
