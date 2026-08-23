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

# Regenerate AppIcon.icns from AppIcon.svg.
#
# The .icns is committed so a build needs no render tooling; run this after
# editing the SVG. Needs rsvg-convert (brew install librsvg).
macos-icon:
    #!/usr/bin/env bash
    set -euo pipefail
    set="{{app_dir}}/Resources/AppIcon.iconset"
    rm -rf "$set" && mkdir -p "$set"
    # Each macOS icon slot, and the pixel size it wants.
    for spec in "16 icon_16x16" "32 icon_16x16@2x" "32 icon_32x32" "64 icon_32x32@2x" \
                "128 icon_128x128" "256 icon_128x128@2x" "256 icon_256x256" \
                "512 icon_256x256@2x" "512 icon_512x512" "1024 icon_512x512@2x"; do
        px="${spec%% *}"; name="${spec##* }"
        rsvg-convert -w "$px" -h "$px" {{app_dir}}/Resources/AppIcon.svg -o "$set/$name.png"
    done
    iconutil -c icns "$set" -o {{app_dir}}/Resources/AppIcon.icns
    echo "built {{app_dir}}/Resources/AppIcon.icns"

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
    # Stage the archive somewhere holding nothing else, and link against that.
    #
    # `-lkoan_ffi` over a directory containing both a .a and a .dylib picks the
    # .dylib, and cargo leaves one next to the archive. The release DMG shipped
    # an app that referenced
    # `/Users/runner/work/koan/koan/target/release/deps/libkoan_ffi.dylib` and
    # could not launch anywhere, and it was arm64-only because the host dylib
    # won over the universal archive. A directory with one file in it cannot
    # produce either outcome.
    rm -rf target/swift-link
    mkdir -p target/swift-link
    cp "$lib" target/swift-link/libkoan_ffi.a

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
    #!/usr/bin/env bash
    set -euo pipefail
    # SwiftPM links libkoan_ffi.a through a systemLibrary target and linker
    # flags, so it has no idea the library is an input: a Rust change with no
    # Swift change leaves the previous binary in place and the app silently runs
    # the old engine. Dropping the product when the library is newer forces the
    # relink.
    product={{app_dir}}/.build/release/Koan
    for lib in target/release/libkoan_ffi.a target/universal/release/libkoan_ffi.a; do
        if [ -f "$lib" ] && [ -f "$product" ] && [ "$lib" -nt "$product" ]; then
            echo "koan-ffi is newer than the built app — forcing a relink"
            rm -f "$product"
        fi
    done
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
    # The accent colour. macOS paints list selection, focus rings and controls
    # from the app's accent, and reads it from a compiled asset catalog — there
    # is no way to set it from SwiftUI, which is why `.tint` leaves sidebar
    # selection stubbornly blue. actool ships with Xcode proper, not the command
    # line tools, so a machine without it gets a working app with the system
    # accent rather than a failed build.
    if /usr/bin/actool --version >/dev/null 2>&1; then
        /usr/bin/actool {{app_dir}}/Resources/Assets.xcassets \
            --compile "$app/Contents/Resources" \
            --platform macosx --minimum-deployment-target 14.0 \
            --output-partial-info-plist /dev/null >/dev/null
        accent='<key>NSAccentColorName</key><string>AccentColor</string>'
    else
        echo "note: actool unavailable (needs full Xcode) — building with the system accent"
        accent=''
    fi
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
        ${accent}
        <key>LSMinimumSystemVersion</key><string>14.0</string>
        <key>NSHighResolutionCapable</key><true/>
        <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
        <key>UTExportedTypeDeclarations</key>
        <array>
            <dict>
                <key>UTTypeIdentifier</key><string>cc.blit.koan.playable</string>
                <key>UTTypeDescription</key><string>koan playable</string>
                <key>UTTypeConformsTo</key>
                <array><string>public.data</string></array>
                <key>UTTypeTagSpecification</key><dict/>
            </dict>
        </array>
    </dict>
    </plist>
    PLIST
    # Signing identity. Ad-hoc ("-") derives the identity from the binary's own
    # hash, so every rebuild is a different app to macOS and any TCC grant —
    # removable volumes, files and folders — has to be given again. Set
    # KOAN_SIGN_IDENTITY to a stable certificate (a self-signed one in your
    # login keychain is enough) and the grants stick across rebuilds.
    #
    # It does nothing for Gatekeeper. A downloaded app is refused unless it is
    # signed with a Developer ID certificate *and* notarised by Apple, which
    # needs a paid developer account — a self-signed certificate is no more
    # trusted than ad-hoc. Direct downloads clear the quarantine flag by hand;
    # the Homebrew cask does it in a postflight.
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
