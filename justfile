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
    # Signing identity. Ad-hoc ("-") derives the identity from the binary's own
    # hash, so every rebuild is a different app to macOS and any TCC grant —
    # removable volumes, files and folders — has to be given again. Set
    # KOAN_SIGN_IDENTITY to a stable certificate (a self-signed one in your
    # login keychain is enough) and the grants stick across rebuilds.
    codesign --force --deep --sign "${KOAN_SIGN_IDENTITY:--}" "$app"
    echo "built $app"

# Build and launch the app bundle.
#
# Quits any running instance first — `open` on a live app just focuses it, so
# without this you get the old binary back and none of your changes.
macos-run: macos-bundle
    #!/usr/bin/env bash
    set -euo pipefail
    osascript -e 'quit app "Koan"' 2>/dev/null || true
    # Wait for it to actually go before replacing it.
    for _ in $(seq 20); do
        pgrep -qf 'Koan.app/Contents/MacOS/koan-app' || break
        sleep 0.2
    done
    pkill -f 'Koan.app/Contents/MacOS/koan-app' 2>/dev/null || true
    open {{app_dir}}/.build/pkg/Koan.app
    echo "launched $(date +%H:%M:%S)"

# Run the app from the terminal: logs on stderr, and no local library folders,
# so macOS stops asking for disk and removable-volume access on every launch.
# Env vars only reach the process this way — `open` does not pass them on.
macos-dev *ARGS: macos-bundle
    #!/usr/bin/env bash
    set -euo pipefail
    osascript -e 'quit app "Koan"' 2>/dev/null || true
    KOAN_LIBRARY__FOLDERS='[]' \
    RUST_LOG="${RUST_LOG:-info}" \
        {{app_dir}}/.build/pkg/Koan.app/Contents/MacOS/koan-app {{ARGS}}

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
