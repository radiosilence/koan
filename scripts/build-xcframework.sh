#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."

FRAMEWORK_DIR="swift/KoanRust.xcframework"
GENERATED_DIR="swift/Generated"
BUILD_DIR="target/release"

rm -rf "$FRAMEWORK_DIR"

TEMP_DIR=$(mktemp -d)
HEADERS_DIR="$TEMP_DIR/Headers"
mkdir -p "$HEADERS_DIR"

# Copy headers
cp "$GENERATED_DIR/koan_audio_ffi.h" "$HEADERS_DIR/"
if [ -f "$GENERATED_DIR/KoanFFIFFI.h" ]; then
    cp "$GENERATED_DIR/KoanFFIFFI.h" "$HEADERS_DIR/"
fi

# Module map exposing both FFI layers
cat > "$HEADERS_DIR/module.modulemap" << 'EOF'
module KoanRust {
    header "koan_audio_ffi.h"
    header "KoanFFIFFI.h"
    export *
}
EOF

# Single static library — both UniFFI and C FFI live in koan-ffi
echo "==> Creating xcframework..."
xcodebuild -create-xcframework \
    -library "$BUILD_DIR/libkoan_ffi.a" \
    -headers "$HEADERS_DIR" \
    -output "$FRAMEWORK_DIR"

rm -rf "$TEMP_DIR"
echo "==> xcframework created at $FRAMEWORK_DIR"
