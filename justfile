# koan — bit-perfect macOS music player

# Build release binary
build:
    cargo build --release

# Build + run CLI in release mode
cli *ARGS:
    cargo run --release -p koan-cli -- {{ARGS}}

# Run tests + clippy
#
# KOAN_NO_KEYCHAIN: a test binary is unsigned and rebuilt under a new hash every
# compile, so a keychain ACL can never match it — every run would prompt for the
# login password, and "Always Allow" would grant it to a binary that is about to
# stop existing.
#
# Set per-recipe, not at the top of the file. A top-level `export` reaches every
# recipe, so `just macos-run` launched the app with its credential store switched
# off: the keychain is where the remote password lives, and without it koan has
# no server — nothing plays, nothing downloads and no artwork loads.
check:
    KOAN_NO_KEYCHAIN=1 cargo test --all-targets
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
#
# One slice, for the machine doing the building. The app used to ship as a
# universal binary: two cross builds, lipo'd together, which was most of the
# release job's fifteen minutes and most of the disk it needed. `macos-verify`
# is what holds this honest — it fails if the assembled app is not the
# architecture asked for.
macos-ffi:
    #!/usr/bin/env bash
    set -euo pipefail
    # Match what SwiftPM links against. Without it cargo builds for the host's
    # OS and every link is a page of "built for newer macOS version" warnings.
    export MACOSX_DEPLOYMENT_TARGET=26.0
    cargo build --release -p koan-ffi
    lib=target/release/libkoan_ffi.a
    dylib=target/release/libkoan_ffi.dylib
    # Stage the archive somewhere holding nothing else, and link against that.
    #
    # `-lkoan_ffi` over a directory containing both a .a and a .dylib picks the
    # .dylib, and cargo leaves one next to the archive. The release DMG shipped
    # an app that referenced
    # `/Users/runner/work/koan/koan/target/release/deps/libkoan_ffi.dylib` and
    # could not launch anywhere. A directory with one file in it cannot produce
    # that outcome.
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
    for lib in target/swift-link/libkoan_ffi.a; do
        if [ -f "$lib" ] && [ -f "$product" ] && [ "$lib" -nt "$product" ]; then
            echo "koan-ffi is newer than the built app — forcing a relink"
            rm -f "$product"
        fi
    done
    cd {{app_dir}} && swift build -c release

# Assemble kōan.app.
macos-bundle: macos-build
    #!/usr/bin/env bash
    set -euo pipefail
    version=$(grep '^version' Cargo.toml | head -1 | cut -d'"' -f2)
    app="{{app_dir}}/.build/pkg/kōan.app"
    rm -rf "$app"
    mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
    cp {{app_dir}}/.build/release/Koan "$app/Contents/MacOS/koan-app"
    echo "app binary: $(lipo -archs "$app/Contents/MacOS/koan-app")"
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
            --platform macosx --minimum-deployment-target 26.0 \
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
        <key>CFBundleName</key><string>kōan</string>
        <key>CFBundleDisplayName</key><string>kōan</string>
        <key>CFBundlePackageType</key><string>APPL</string>
        <key>CFBundleShortVersionString</key><string>${version}</string>
        <key>CFBundleVersion</key><string>${version}</string>
        <key>CFBundleIconFile</key><string>AppIcon</string>
        ${accent}
        <key>LSMinimumSystemVersion</key><string>26.0</string>
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

# Create the self-signed certificate that dev builds sign with.
#
# Ad-hoc signing derives the app's identity from the binary's own hash, so every
# rebuild is a different application to macOS. Keychain items and TCC grants are
# both keyed on that identity, which is why the app asks for keychain access
# again after every build and forgets its permission to read removable volumes.
#
# A stable certificate fixes both. It does nothing for Gatekeeper — a self-signed
# certificate is no more trusted than ad-hoc, and only Developer ID plus
# notarisation clears that — so this is for development, not distribution.
#
# The prompt this removes is the legacy keychain ACL dialog, which authenticates
# with the login password and cannot use Touch ID — biometrics belong to the
# data-protection keychain, a different API an app opts into, and which asks on
# every read by design. "Always Allow" binds the item to the signing identity, so
# it holds only while that identity is stable, which is what this provides.
#
# Run once, then export KOAN_SIGN_IDENTITY="koan development".
macos-signing-cert:
    #!/usr/bin/env bash
    set -euo pipefail
    name="koan development"
    if security find-identity -v -p codesigning | grep -q "$name"; then
        echo "already have a '$name' identity"
        echo "export KOAN_SIGN_IDENTITY=\"$name\""
        exit 0
    fi

    dir=$(mktemp -d)
    trap 'rm -rf "$dir"' EXIT

    # codeSigning EKU is what lets codesign treat this as an identity.
    openssl req -x509 -newkey rsa:2048 -nodes -days 3650 \
        -keyout "$dir/key.pem" -out "$dir/cert.pem" \
        -subj "/CN=$name" \
        -addext "basicConstraints=critical,CA:false" \
        -addext "keyUsage=critical,digitalSignature" \
        -addext "extendedKeyUsage=critical,codeSigning" 2>/dev/null

    # SHA-1/3DES and a non-empty passphrase, because Apple's `security import`
    # reads neither OpenSSL 3's defaults nor an empty-password PKCS#12 — both
    # fail as "MAC verification failed (wrong password?)", which is not what is
    # wrong. The passphrase protects a file that exists for one command.
    openssl pkcs12 -export -inkey "$dir/key.pem" -in "$dir/cert.pem" \
        -out "$dir/identity.p12" -passout pass:koan \
        -keypbe PBE-SHA1-3DES -certpbe PBE-SHA1-3DES -macalg sha1 2>/dev/null

    # -A lets any tool use the private key without asking, which is the whole
    # point: being asked is what this recipe exists to stop.
    security import "$dir/identity.p12" -k ~/Library/Keychains/login.keychain-db \
        -P koan -T /usr/bin/codesign -A

    # Without this the certificate imports but is not a *code-signing* identity,
    # and `security find-identity -p codesigning` still reports none. User
    # domain, code signing only — no sudo, and no bearing on any other trust.
    security add-trusted-cert -r trustRoot -p codeSign \
        -k ~/Library/Keychains/login.keychain-db "$dir/cert.pem"

    echo
    echo "created. add this to your shell profile:"
    echo "    export KOAN_SIGN_IDENTITY=\"$name\""

# Check a built kōan.app is actually shippable.
#
# Two ways the bundle has gone out broken, both of which built and signed
# cleanly and neither of which showed until someone downloaded it:
#   - linked against cargo's dylib by absolute path, so it died in dyld on any
#     machine but the one that built it
#   - arm64 only, from a build that had compiled x86_64 as well
#
# ARCHES is what the binary must contain, e.g. "arm64 x86_64".
macos-verify *ARCHES:
    #!/usr/bin/env bash
    set -euo pipefail
    bin="{{app_dir}}/.build/pkg/kōan.app/Contents/MacOS/koan-app"
    [ -f "$bin" ] || { echo "no app bundle at $bin"; exit 1; }

    if otool -L "$bin" | grep -q koan_ffi; then
        echo "app links koan_ffi dynamically — it will not run off this machine:"
        otool -L "$bin" | grep koan_ffi
        exit 1
    fi

    have=$(lipo -archs "$bin")
    for want in {{ARCHES}}; do
        case " $have " in
            *" $want "*) ;;
            *) echo "app binary is [$have], missing $want"; exit 1 ;;
        esac
    done
    echo "app binary: [$have], statically linked"

# Build and launch the app bundle.
#
# Quits any running instance first — `open` on a live app just focuses it, so
# without this you get the old binary back and none of your changes.
macos-run: macos-bundle
    #!/usr/bin/env bash
    set -euo pipefail
    osascript -e 'quit app "kōan"' 2>/dev/null || true
    # Wait for it to actually go before replacing it.
    for _ in $(seq 20); do
        pgrep -qf 'kōan.app/Contents/MacOS/koan-app' || break
        sleep 0.2
    done
    pkill -f 'kōan.app/Contents/MacOS/koan-app' 2>/dev/null || true
    open {{app_dir}}/.build/pkg/kōan.app
    echo "launched $(date +%H:%M:%S)"

# Run the app from the terminal: logs on stderr, and no local library folders,
# so macOS stops asking for disk and removable-volume access on every launch.
# Env vars only reach the process this way — `open` does not pass them on.
macos-dev *ARGS: macos-bundle
    #!/usr/bin/env bash
    set -euo pipefail
    osascript -e 'quit app "kōan"' 2>/dev/null || true
    KOAN_LIBRARY__FOLDERS='[]' \
    RUST_LOG="${RUST_LOG:-info}" \
        {{app_dir}}/.build/pkg/kōan.app/Contents/MacOS/koan-app {{ARGS}}

# Package the app as a DMG for release.
macos-dmg: macos-bundle
    #!/usr/bin/env bash
    set -euo pipefail
    out={{app_dir}}/.build/pkg
    rm -f "$out/Koan.dmg"
    hdiutil create -volname "koan" -srcfolder "$out/kōan.app" -ov -format UDZO "$out/Koan.dmg"
    echo "built $out/Koan.dmg"

# Run the macOS app's tests.
macos-test: macos-ffi
    cd {{app_dir}} && KOAN_NO_KEYCHAIN=1 swift test
