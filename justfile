# koan — bit-perfect macOS music player

# Build release binary
build:
    cargo build --release

# Build + run CLI in release mode
cli *ARGS:
    cargo run --release -p koan-cli -- {{ARGS}}

# Run tests + clippy
check:
    cargo test --all-targets
    cargo clippy --all-targets -- -D warnings

# Format
fmt:
    cargo fmt

# Install dev build to ~/.local/bin/koan-dev
install-dev:
    cargo build --release
    mkdir -p ~/.local/bin
    cp target/release/koan ~/.local/bin/koan-dev
    @echo "Installed to ~/.local/bin/koan-dev"

# Watch for changes and rebuild dev binary
watch-dev:
    cargo watch -s 'cargo build --release && cp target/release/koan ~/.local/bin/koan-dev && echo "✓ koan-dev updated"'

# Clean build artifacts
clean:
    cargo clean

# --- macOS app ---------------------------------------------------------------
# The SwiftUI app links koan-core through the koan-ffi static library, so the
# Rust side must be built and its bindings regenerated before `swift build`.

bundle_id := "cc.blit.koan"
app_dir := "apps/macos"

# Build the FFI static library and regenerate the Swift bindings.
macos-ffi *TARGETS:
    #!/usr/bin/env bash
    set -euo pipefail
    targets="{{TARGETS}}"
    if [ -z "$targets" ]; then
        cargo build --release -p koan-ffi
        lib=target/release/libkoan_ffi.a
        dylib=target/release/libkoan_ffi.dylib
    else
        for t in $targets; do
            cargo build --release -p koan-ffi --target "$t"
        done
        mkdir -p target/universal/release
        lipo -create $(for t in $targets; do echo "target/$t/release/libkoan_ffi.a"; done) \
             -output target/universal/release/libkoan_ffi.a
        lib=target/universal/release/libkoan_ffi.a
        dylib=target/$(echo $targets | cut -d' ' -f1)/release/libkoan_ffi.dylib
    fi
    # Bindings are generated from the dylib's embedded metadata, not the sources.
    cargo run --release -q -p koan-ffi --bin uniffi-bindgen -- \
        generate --library "$dylib" --language swift --out-dir target/uniffi
    # These directories hold only generated files, so git doesn't carry them.
    mkdir -p {{app_dir}}/Sources/KoanFFI {{app_dir}}/Sources/koan_ffiFFI
    cp target/uniffi/koan_ffi.swift {{app_dir}}/Sources/KoanFFI/
    cp target/uniffi/koan_ffiFFI.h {{app_dir}}/Sources/koan_ffiFFI/
    echo "koan-ffi ready: $lib"

# Compile the SwiftUI app.
macos-build: macos-ffi
    cd {{app_dir}} && swift build -c release

# Assemble Koan.app.
macos-bundle: macos-build
    #!/usr/bin/env bash
    set -euo pipefail
    version=$(grep '^version' Cargo.toml | head -1 | cut -d'"' -f2)
    app="{{app_dir}}/.build/pkg/Koan.app"
    rm -rf "$app"
    mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
    cp {{app_dir}}/.build/release/Koan "$app/Contents/MacOS/koan-app"
    [ -f {{app_dir}}/Resources/AppIcon.icns ] && cp {{app_dir}}/Resources/AppIcon.icns "$app/Contents/Resources/" || true
    cat > "$app/Contents/Info.plist" <<PLIST
    <?xml version="1.0" encoding="UTF-8"?>
    <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
    <plist version="1.0">
    <dict>
        <key>CFBundleExecutable</key><string>koan-app</string>
        <key>CFBundleIdentifier</key><string>{{bundle_id}}</string>
        <key>CFBundleName</key><string>Koan</string>
        <key>CFBundleDisplayName</key><string>koan</string>
        <key>CFBundlePackageType</key><string>APPL</string>
        <key>CFBundleShortVersionString</key><string>${version}</string>
        <key>CFBundleVersion</key><string>${version}</string>
        <key>CFBundleIconFile</key><string>AppIcon</string>
        <key>LSMinimumSystemVersion</key><string>14.0</string>
        <key>NSHighResolutionCapable</key><true/>
        <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
    </dict>
    </plist>
    PLIST
    # Ad-hoc signature — the bundle links a Rust static library, and an unsigned
    # binary trips Gatekeeper harder than an unsigned bundle does.
    codesign --force --deep --sign - "$app"
    echo "built $app"

# Build and launch the app bundle.
macos-run: macos-bundle
    open {{app_dir}}/.build/pkg/Koan.app

# Package the app as a DMG for release.
macos-dmg: macos-bundle
    #!/usr/bin/env bash
    set -euo pipefail
    out={{app_dir}}/.build/pkg
    rm -f "$out/Koan.dmg"
    hdiutil create -volname "koan" -srcfolder "$out/Koan.app" -ov -format UDZO "$out/Koan.dmg"
    echo "built $out/Koan.dmg"

# Run the macOS app's tests.
macos-test: macos-ffi
    cd {{app_dir}} && swift test
