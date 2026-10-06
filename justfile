# koan — bit-perfect music player

# Build release binary
build:
    cargo build --release

# Build + run CLI in release mode
cli *ARGS:
    cargo run --release -p koan-cli -- {{ARGS}}

# Run tests + clippy
# Tests run against a config dir of their own. One that reaches past
# `isolate_config_for_tests` would otherwise open the real library and run the
# branch's migrations on it; here it writes into the canary and fails the check.
check:
    #!/usr/bin/env bash
    set -euo pipefail
    canary=$(mktemp -d)
    trap 'rm -rf "$canary"' EXIT
    KOAN_CONFIG_DIR="$canary" cargo test --all-targets
    if [ -n "$(ls -A "$canary")" ]; then
        echo "a test wrote to the config dir instead of isolating it:" >&2
        ls -A "$canary" >&2
        exit 1
    fi
    cargo clippy --all-targets -- -D warnings

# Format
fmt:
    cargo fmt

# Write CHANGELOG.md from the fragments in changelog.d/.
changelog *args:
    python3 scripts/changelog.py {{args}}

# Compile the web UI and share page stylesheets. The output is committed, since
# the server embeds it; CI fails a build where it is stale.
css:
    tailwindcss --input crates/koan-server/styles/ui.css --output crates/koan-server/assets/ui.css
    tailwindcss --input crates/koan-server/styles/share.css --output crates/koan-server/assets/share.css

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
# One slice, for the machine doing the building: a universal binary is two
# cross builds lipo'd together, most of a release job's time and disk.
# `macos-verify` fails if the assembled app is not the architecture asked for.
macos-ffi:
    #!/usr/bin/env bash
    set -euo pipefail
    # Match what SwiftPM links against. Without it cargo builds for the host's
    # OS and every link is a page of "built for newer macOS version" warnings.
    export MACOSX_DEPLOYMENT_TARGET=26.0
    cargo build --release -p koan-ffi
    lib=target/release/libkoan_ffi.a
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

    just ffi-bindings "$lib"
    echo "koan-ffi ready: $lib"

# Generate the Swift bindings from a built koan-ffi library: from the metadata
# embedded in it, not the sources, so any build of the engine serves, the iOS
# one included. The generator is a crate of its own and builds nothing else.
ffi-bindings lib:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo run -q -p uniffi-bindgen -- \
        generate --library "{{lib}}" --language swift --out-dir target/uniffi
    # These directories hold only generated files, so git doesn't carry them.
    mkdir -p {{app_dir}}/Sources/KoanFFI {{app_dir}}/Sources/koan_ffiFFI
    cp target/uniffi/koan_ffi.swift {{app_dir}}/Sources/KoanFFI/
    cp target/uniffi/koan_ffiFFI.h {{app_dir}}/Sources/koan_ffiFFI/

# Compile the SwiftUI app. With KOAN_APP_BINARY naming an app binary built
# already, that is used and nothing is compiled: the release packages the one
# CI's macOS App job built from the same commit rather than building it again.
macos-build:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ -n "${KOAN_APP_BINARY:-}" ]; then
        mkdir -p {{app_dir}}/.build/release
        cp "$KOAN_APP_BINARY" {{app_dir}}/.build/release/Koan
        echo "using the app binary built earlier: $KOAN_APP_BINARY"
        exit 0
    fi
    just macos-ffi
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
    # Geist Mono, for the kōan theme: the site's own file, read by Core Text as it is.
    cp site/public/geist-mono.woff2 "$app/Contents/Resources/"
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
        <key>CFBundleURLTypes</key>
        <array>
            <dict>
                <key>CFBundleURLName</key><string>{{bundle_id}}</string>
                <key>CFBundleURLSchemes</key><array><string>koan</string></array>
            </dict>
        </array>
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
    # trusted than ad-hoc.
    # A Developer ID signature is for distribution: notarisation requires the
    # hardened runtime and a secure timestamp, and only an Apple-issued
    # identity can get the timestamp, so a self-signed dev certificate keeps
    # the plain signature.
    id="${KOAN_SIGN_IDENTITY:--}"
    case "$id" in
        "Developer ID Application"*)
            codesign --force --options runtime --timestamp --sign "$id" "$app" ;;
        *)
            codesign --force --deep --sign "$id" "$app" ;;
    esac
    codesign --verify --strict --verbose=2 "$app"
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

# Check a built kōan.app is shippable.
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
    # Wait for it to exit before replacing it.
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
# The window opens with the app, an Applications link to drop it on, and a
# background drawn by `dmg/background.swift`. dmgbuild lays the window out by
# writing .DS_Store directly, so this needs no Finder session and runs in CI.
# It comes from uv where there is one, else pipx (preinstalled on GitHub's
# macOS runners).
#
# Package kōan.app into Koan.dmg.
macos-dmg: macos-bundle
    #!/usr/bin/env bash
    set -euo pipefail
    out={{app_dir}}/.build/pkg
    bg={{app_dir}}/.build/dmg
    mkdir -p "$bg"
    swift {{app_dir}}/dmg/background.swift {{app_dir}}/Resources/AppIcon.svg site/public/geist-mono.woff2 "$bg"
    tiffutil -cathidpicheck "$bg/background.png" "$bg/background@2x.png" -out "$bg/background.tiff" >/dev/null
    if command -v uvx >/dev/null; then dmgbuild=(uvx --from dmgbuild==1.6.7 dmgbuild)
    else dmgbuild=(pipx run --spec dmgbuild==1.6.7 dmgbuild); fi
    rm -f "$out/Koan.dmg"
    "${dmgbuild[@]}" -s {{app_dir}}/dmg/settings.py \
        -D app="$out/kōan.app" -D background="$bg/background.tiff" \
        -D volume_icon={{app_dir}}/Resources/AppIcon.icns \
        kōan "$out/Koan.dmg"
    echo "built $out/Koan.dmg"

# Needs KOAN_SIGN_IDENTITY (a "Developer ID Application" identity in the
# keychain) and an App Store Connect API key: APPLE_API_KEY_PATH (the .p8),
# APPLE_API_KEY_ID and APPLE_API_ISSUER_ID. The stapled ticket lets Gatekeeper
# open the DMG without asking and without the network.
#
# Sign the DMG with the Developer ID, notarise it with Apple, and staple the ticket.
macos-notarize: macos-dmg
    #!/usr/bin/env bash
    set -euo pipefail
    dmg={{app_dir}}/.build/pkg/Koan.dmg
    codesign --force --timestamp --sign "$KOAN_SIGN_IDENTITY" "$dmg"
    xcrun notarytool submit "$dmg" \
        --key "$APPLE_API_KEY_PATH" --key-id "$APPLE_API_KEY_ID" --issuer "$APPLE_API_ISSUER_ID" \
        --wait --timeout 30m
    xcrun stapler staple "$dmg"
    xcrun stapler validate "$dmg"
    spctl --assess --type open --context context:primary-signature --verbose=2 "$dmg"

# Run the macOS app's tests.
macos-test: macos-ffi
    cd {{app_dir}} && swift test

# --- iOS --------------------------------------------------------------------


ios_deployment_target := "26.0"
tv_deployment_target := "26.0"

# Styling that bypasses the kōan theme: a raw font, colour, label style or
# corner in the apps' views rather than a role from `Support/KoanTheme.swift`.
# The theme is the default, so each of these is a place it does not reach.
# A line that must stay raw says why with `// theme: raw` and is skipped.
theme-leaks:
    #!/usr/bin/env bash
    set -uo pipefail
    patterns=(
        '\.font\(\.(largeTitle|title|title2|title3|headline|body|callout|subheadline|footnote|caption|caption2)\b'
        '\.foreground(Style|Color)\(\.(primary|secondary|tertiary|quaternary)\)'
        'cornerRadius: [0-9]'
        'AnyShapeStyle\(\.(primary|secondary|tertiary)\)'
        'Color\.accentColor|Color\.koanAccent|\.controlAccentColor'
        'ContentUnavailableView\('
        'font: \.(caption|callout|body|subheadline|footnote|headline)\b'
        '\.shadow\(color: \.black\.opacity\([0-9]'
        'Color\((red|white|hue):'
        'NSFont\.(systemFont|preferredFont|monospacedSystemFont|monospacedDigitSystemFont)\('
        '[^.]\.(labelColor|secondaryLabelColor|tertiaryLabelColor)\b'
    )
    found=0
    for pattern in "${patterns[@]}"; do
        hits=$(grep -rnE "$pattern" apps/macos/Sources --include='*.swift' \
            | grep -v -e 'Support/KoanTheme.swift' -e '// theme: raw' -e 'role(\.' -e 'KoanTheme\.' -e 'Support/Graphics.swift' -e '\.pointSize')
        if [ -n "$hits" ]; then
            found=1
            echo "$hits"
        fi
    done
    [ "$found" = 0 ] && echo "no theme leaks" || { echo "theme leaks above"; exit 1; }

# Type-check the shared SwiftUI sources against the iOS SDK.
#
# The bindings are target-independent, so this needs `macos-ffi` and nothing
# else: no Rust iOS build, no simulator runtime, a few seconds in CI.
#
# The excluded files are the macOS shell — the scene root, the split view, the
# menu bar and the machinery that serves it. They have no iOS counterpart.
ios-typecheck: macos-ffi
    #!/usr/bin/env bash
    set -euo pipefail
    shell=(KoanApp Hotkeys TextFocus EditCommands MenuShortcuts ShortcutsSheet)
    find_args=()
    for f in "${shell[@]}"; do find_args+=(! -name "$f.swift"); done
    mod=$(mktemp -d)
    trap 'rm -rf "$mod"' EXIT
    ffi={{app_dir}}/Sources/koan_ffiFFI
    target=arm64-apple-ios{{ios_deployment_target}}-simulator
    # KoanFFI is its own module in the package, so it has to be built as one
    # before anything that imports it can be checked.
    xcrun -sdk iphonesimulator swiftc -target "$target" -swift-version 6 \
        -package-name koan \
        -emit-module -module-name KoanFFI -emit-module-path "$mod/KoanFFI.swiftmodule" \
        -Xcc -fmodule-map-file="$PWD/$ffi/module.modulemap" -I "$PWD/$ffi" \
        {{app_dir}}/Sources/KoanFFI/koan_ffi.swift
    # SIL, not just a typecheck: Swift 6's data-race checks run on SIL, and
    # `-typecheck` stops before them, so an archive can fail on a race it passed.
    xcrun -sdk iphonesimulator swiftc -target "$target" -swift-version 6 \
        -package-name koan \
        -wmo -emit-sil -o /dev/null -module-name Koan -I "$mod" \
        -Xcc -fmodule-map-file="$PWD/$ffi/module.modulemap" -I "$PWD/$ffi" \
        $(find {{app_dir}}/Sources/KoanIOS -name '*.swift') \
        $(find {{app_dir}}/Sources/Koan -name '*.swift' "${find_args[@]}")
    echo "the shared sources still build for iOS"


# Build the Rust engine for an iOS SDK and stage it for the Swift link.
#
# `iphonesimulator` or `iphoneos`, staged under the SDK's own name so the Xcode
# project can find the right one through `$(PLATFORM_NAME)`.
ios-ffi platform="iphonesimulator":
    #!/usr/bin/env bash
    set -euo pipefail
    case "{{platform}}" in
        iphonesimulator) triple=aarch64-apple-ios-sim ;;
        iphoneos) triple=aarch64-apple-ios ;;
        *) echo "unknown platform: {{platform}}" >&2; exit 1 ;;
    esac
    # Without this rustc targets arm64-apple-ios10.0.0 while every C dependency
    # compiled against the current SDK, and the link dies in a wall of "built
    # for newer iOS version". `macos-ffi` exports the macOS equivalent.
    export IPHONEOS_DEPLOYMENT_TARGET={{ios_deployment_target}}
    # A target directory per deployment target, because cargo does not count
    # that variable as a reason to rebuild: lowering it relinked objects built
    # for the old version, and the linker warned about every one of them.
    out=target/ios-{{ios_deployment_target}}
    cargo build --release -p koan-ffi --target "$triple" --target-dir "$out"
    rm -rf "target/ios-link/{{platform}}" && mkdir -p "target/ios-link/{{platform}}"
    cp "$out/$triple/release/libkoan_ffi.a" "target/ios-link/{{platform}}/"
    # From this build rather than the Mac's, so an iOS build needs no second
    # build of the engine for the host.
    just ffi-bindings "target/ios-link/{{platform}}/libkoan_ffi.a"
    echo "koan-ffi ready for {{platform}}"

# Assemble koan.app for the iOS simulator.
#
# By hand, exactly as `macos-bundle` does, and for the same reason: SwiftPM has
# no app product. A simulator bundle is the one case where that is enough — it
# needs no provisioning profile and no signature. A device build does, and that
# is what an Xcode project is for.
ios-bundle: macos-ffi ios-ffi
    #!/usr/bin/env bash
    set -euo pipefail
    app=target/ios-app/koan.app
    rm -rf "$app" && mkdir -p "$app"
    icon_plist=$(mktemp -t koan-icons.XXXXXX)
    trap 'rm -f "$icon_plist"' EXIT
    ffi={{app_dir}}/Sources/koan_ffiFFI
    target=arm64-apple-ios{{ios_deployment_target}}-simulator
    mod=target/ios-app/modules
    rm -rf "$mod" && mkdir -p "$mod"
    # KoanFFI is its own module in the package, so it is built as one here too —
    # compiling its source alongside the app would leave `import KoanFFI`
    # looking for a module that is being compiled into the same one.
    xcrun -sdk iphonesimulator swiftc -target "$target" -swift-version 6 -O \
        -package-name koan -module-name KoanFFI \
        -Xcc -fmodule-map-file="$PWD/$ffi/module.modulemap" -I "$PWD/$ffi" \
        -emit-module -emit-module-path "$mod/KoanFFI.swiftmodule" \
        -emit-library -static -o "$mod/libKoanFFI.a" \
        {{app_dir}}/Sources/KoanFFI/koan_ffi.swift
    xcrun -sdk iphonesimulator swiftc -target "$target" -swift-version 6 -O \
        -package-name koan \
        -I "$mod" -L "$mod" -lKoanFFI \
        -Xcc -fmodule-map-file="$PWD/$ffi/module.modulemap" -I "$PWD/$ffi" \
        -L "$PWD/target/ios-link/iphonesimulator" -lkoan_ffi \
        -framework AudioToolbox -framework AVFAudio -framework AVKit -framework MediaPlayer \
        -o "$app/koan" \
        $(find {{app_dir}}/Sources/KoanIOS -name '*.swift') \
        $(find {{app_dir}}/Sources/Koan -name '*.swift' \
            ! -name 'KoanApp.swift' ! -name 'Hotkeys.swift' \
            ! -name 'TextFocus.swift' ! -name 'EditCommands.swift' \
            ! -name 'MenuShortcuts.swift' ! -name 'ShortcutsSheet.swift')
    # The accent colour comes from the compiled catalog, exactly as on macOS —
    # `Color("AccentColor")` finds nothing without it and every tinted control
    # renders in nothing.
    if /usr/bin/actool --version >/dev/null 2>&1; then
        # `--app-icon` is not optional: without it actool compiles the colour
        # sets and silently leaves the icon out of the catalog entirely. Nor is
        # keeping the partial plist — it carries the CFBundleIcons that tell
        # SpringBoard which rendition to draw, and an icon in the catalog that
        # nothing names shows as an empty tile.
        /usr/bin/actool {{app_dir}}/Resources/Assets.xcassets \
            --compile "$app" --platform iphonesimulator \
            --minimum-deployment-target {{ios_deployment_target}} \
            --app-icon AppIcon --include-all-app-icons \
            --output-partial-info-plist "$icon_plist" >/dev/null
    else
        echo "note: actool unavailable (needs full Xcode) — building without an icon or accent"
    fi
    # iOS bundles are flat — no Contents/MacOS.
    cat > "$app/Info.plist" <<PLIST
    <?xml version="1.0" encoding="UTF-8"?>
    <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
    <plist version="1.0">
    <dict>
        <key>CFBundleExecutable</key><string>koan</string>
        <key>CFBundleIdentifier</key><string>{{bundle_id}}</string>
        <key>CFBundleName</key><string>kōan</string>
        <!-- The catalog holds the icon; this is what names it. Without it the
             home screen shows an empty tile and no error anywhere. -->
        <key>CFBundleIconName</key><string>AppIcon</string>
        <key>CFBundlePackageType</key><string>APPL</string>
        <key>CFBundleShortVersionString</key><string>0.0.0</string>
        <key>CFBundleVersion</key><string>1</string>
        <key>LSRequiresIPhoneOS</key><true/>
        <key>MinimumOSVersion</key><string>{{ios_deployment_target}}</string>
        <key>UIDeviceFamily</key><array><integer>1</integer><integer>2</integer></array>
        <key>UILaunchScreen</key><dict>
            <key>UIImageName</key><string>LaunchEnso</string>
            <key>UIColorName</key><string>LaunchBackground</string>
        </dict>
        <!-- Without this the process is suspended when the screen locks, and
             the audio thread with it. -->
        <key>UIBackgroundModes</key><array><string>audio</string></array>
        <key>CFBundleURLTypes</key>
        <array>
            <dict>
                <key>CFBundleURLName</key><string>{{bundle_id}}</string>
                <key>CFBundleURLSchemes</key><array><string>koan</string></array>
            </dict>
        </array>
        <key>NSBonjourServices</key><array><string>_koan._tcp</string></array>
        <key>NSLocalNetworkUsageDescription</key><string>koan finds other koan apps on your network to play music on.</string>
    </dict>
    </plist>
    PLIST
    # actool's keys, folded into the plist written above.
    if [ -s "$icon_plist" ]; then
        /usr/libexec/PlistBuddy -c "Merge $icon_plist" "$app/Info.plist" >/dev/null
    fi
    echo "built $app"

# Install and launch koan on a booted simulator.
ios-run: ios-bundle
    #!/usr/bin/env bash
    set -euo pipefail
    # A booted simulator if there is one, otherwise the first that can run the
    # deployment target — an older runtime installs the app and refuses to
    # launch it.
    device=$(xcrun simctl list devices available -j \
        | python3 -c 'import json,sys; want=int("{{ios_deployment_target}}".split(".")[0]); ds=[d for k,v in json.load(sys.stdin)["devices"].items() if "iOS-" in k and int(k.split("iOS-")[1].split("-")[0])>=want for d in v if d["isAvailable"]]; print(next((d["udid"] for d in ds if d["state"]=="Booted"), ds[0]["udid"]))')
    xcrun simctl boot "$device" 2>/dev/null || true
    xcrun simctl bootstatus "$device" -b
    xcrun simctl install "$device" target/ios-app/koan.app
    xcrun simctl launch --console-pty "$device" {{bundle_id}}

# Play a file through the real Player, on the simulator's real output.
#
# The check that matters for iOS: position only advances when RemoteIO's render
# callback drains the ring buffer, so a track that reaches its end has exercised
# decode, timeline and output together. `KOAN_STOP_AFTER_MS` forces the teardown
# instead of waiting for the queue to run out — the teardown path, shared with
# macOS, where a double free of CoreAudio's buffer list would show.
#
#     just ios-smoke ~/some/short.wav
ios-smoke FILE:
    #!/usr/bin/env bash
    set -euo pipefail
    export IPHONEOS_DEPLOYMENT_TARGET={{ios_deployment_target}}
    cargo build -q -p koan-core --example end_of_queue --target aarch64-apple-ios-sim
    device=$(xcrun simctl list devices booted -j \
        | python3 -c 'import json,sys; print([d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v][0])')
    bin=$PWD/target/aarch64-apple-ios-sim/debug/examples/end_of_queue
    # simctl only forwards environment prefixed for the child.
    echo "--- playing to the end of the queue"
    SIMCTL_CHILD_RUST_LOG=info xcrun simctl spawn "$device" "$bin" "{{FILE}}"
    echo "--- stopping mid-track"
    SIMCTL_CHILD_KOAN_STOP_AFTER_MS=1200 SIMCTL_CHILD_RUST_LOG=info \
        xcrun simctl spawn "$device" "$bin" "{{FILE}}"

# Generate the Xcode project the device build and TestFlight need.
#
# SwiftPM has no app product, which is fine for a simulator bundle and not for
# anything that has to be signed for a device. The project is generated from
# `apps/ios/project.yml` rather than checked in. The team defaults to empty,
# which builds but cannot sign. Links what `ios-ffi` staged and compiles the
# bindings it generated, so that runs first.
ios-project build="1":
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p target/ios-project
    KOAN_VERSION=$(grep '^version' Cargo.toml | head -1 | cut -d'"' -f2) \
    KOAN_BUILD={{build}} \
    KOAN_TEAM_ID=${APPLE_TEAM_ID:-} \
        xcodegen generate --quiet --spec apps/ios/project.yml
    echo "generated apps/ios/Koan.xcodeproj"

# Use the app as a listener does and check each step worked: browse, play,
# pause, skip, favourite, queue, Now Playing, lyrics, playlists, search, play on
# in the background, and seek into a track still downloading. What to run
# before a submission, on an iPhone and an iPad simulator signed in with
# `ios-signin`. Screenshots of each step land in target/ios-use.
ios-use device="koan-dev": (ios-ffi "iphonesimulator") ios-project
    #!/usr/bin/env bash
    set -euo pipefail
    out=target/ios-use
    rm -rf "$out" && mkdir -p "$out"
    udid=$(xcrun simctl list devices available | grep -F "{{device}} (" | head -1 | grep -oE '[0-9A-F-]{36}')
    xcrun simctl boot "$udid" 2>/dev/null || true
    xcrun simctl bootstatus "$udid" -b >/dev/null
    trap 'xcrun simctl shutdown "$udid"' EXIT
    status=0
    xcodebuild test -quiet \
        -project apps/ios/Koan.xcodeproj -scheme Koan \
        -destination "id=$udid" \
        -only-testing:KoanUITests/UseTests -only-testing:KoanUITests/SeekTests \
        -resultBundlePath "$out/use.xcresult" || status=$?
    xcrun xcresulttool export attachments --path "$out/use.xcresult" --output-path "$out" >/dev/null
    echo "screenshots in $out"
    exit $status

# Walk the app on a simulator and export a screenshot of every page.
#
# Runs `WalkTests` against whatever library that simulator holds, so sign it in
# to a server first. Screenshots land in target/ios-walk.
ios-walk device="koan-dev" search="gabriel": (ios-ffi "iphonesimulator") ios-project
    #!/usr/bin/env bash
    set -euo pipefail
    out=target/ios-walk
    rm -rf "$out" && mkdir -p "$out"
    udid=$(xcrun simctl list devices available | grep -F "{{device}} (" | head -1 | grep -oE '[0-9A-F-]{36}')
    xcrun simctl boot "$udid" 2>/dev/null || true
    xcrun simctl bootstatus "$udid" -b >/dev/null
    # Apple's clean status bar, so the screenshots can go anywhere.
    xcrun simctl status_bar "$udid" override --time 9:41 --dataNetwork wifi --wifiMode active \
        --wifiBars 3 --cellularMode active --cellularBars 4 --batteryState charged --batteryLevel 100
    # A booted simulator is a running copy of iOS; leave none behind.
    trap 'xcrun simctl status_bar "$udid" clear; xcrun simctl shutdown "$udid"' EXIT
    TEST_RUNNER_KOAN_WALK_SEARCH='{{search}}' TEST_RUNNER_KOAN_WALK_SETTLE="${KOAN_WALK_SETTLE:-0}" xcodebuild test -quiet \
        -project apps/ios/Koan.xcodeproj -scheme Koan \
        -destination "id=$udid" \
        -only-testing:KoanUITests/WalkTests \
        -resultBundlePath "$out/walk.xcresult" || true
    xcrun xcresulttool export attachments --path "$out/walk.xcresult" --output-path "$out"
    echo "screenshots in $out"

# Sign a simulator in to a server through Settings, as App Review does, and
# wait for its albums: the check that the review account works, and how a
# screenshot simulator gets a library. `just ios-signin koan-shots-iphone
# https://demo.navidrome.org demo demo`.
ios-signin device url user password: (ios-ffi "iphonesimulator") ios-project
    #!/usr/bin/env bash
    set -euo pipefail
    udid=$(xcrun simctl list devices available | grep -F "{{device}} (" | head -1 | grep -oE '[0-9A-F-]{36}')
    xcrun simctl boot "$udid" 2>/dev/null || true
    xcrun simctl bootstatus "$udid" -b >/dev/null
    trap 'xcrun simctl shutdown "$udid"' EXIT
    TEST_RUNNER_KOAN_SIGNIN_URL='{{url}}' \
    TEST_RUNNER_KOAN_SIGNIN_USER='{{user}}' \
    TEST_RUNNER_KOAN_SIGNIN_PASSWORD='{{password}}' \
        xcodebuild test -quiet \
            -project apps/ios/Koan.xcodeproj -scheme Koan \
            -destination "id=$udid" \
            -only-testing:KoanUITests/SignInTests
    echo "{{device}} is signed in to {{url}}"

# Open an invite link on a simulator and wait for its library, as a listener
# tapping it would. `koan auth invite` prints one.
ios-join device link: (ios-ffi "iphonesimulator") ios-project
    #!/usr/bin/env bash
    set -euo pipefail
    udid=$(xcrun simctl list devices available | grep -F "{{device}} (" | head -1 | grep -oE '[0-9A-F-]{36}')
    xcrun simctl boot "$udid" 2>/dev/null || true
    xcrun simctl bootstatus "$udid" -b >/dev/null
    TEST_RUNNER_KOAN_INVITE_LINK='{{link}}' \
        xcodebuild test -quiet \
            -project apps/ios/Koan.xcodeproj -scheme Koan \
            -destination "id=$udid" \
            -resultBundlePath target/ios-join.xcresult \
            -only-testing:KoanUITests/InviteTests
    echo "{{device}} joined through the invite"

# Build, install and launch on the iPhone plugged in (or on the same Wi-Fi).
#
# Signed with the free personal team unless APPLE_TEAM_ID says otherwise: it
# needs nothing but the Apple ID Xcode is signed in to, and installs expire
# after seven days. Installing over the app keeps its library and sign-in.
#
# Debug by default, because it builds incrementally; the engine is a release
# build either way, and the Swift is not where playback spends its time.
ios-phone config="Debug": (ios-ffi "iphoneos")
    #!/usr/bin/env bash
    set -euo pipefail
    phone=$(xcrun devicectl list devices | awk '/physical/ && /connected|available/' \
        | grep -oE '[0-9A-F]{8}-[0-9A-F]{16}' | head -1 || true)
    [ -n "$phone" ] || { echo "No iPhone found — plug it in, or unlock it if it is on Wi-Fi." >&2; exit 1; }
    APPLE_TEAM_ID=${APPLE_TEAM_ID:-2256Q92VF2} just ios-project
    # With the App Store Connect key, xcodebuild fetches a development profile
    # itself, one that carries the push entitlement; without it, it needs an
    # account signed in to Xcode.
    auth=(-allowProvisioningUpdates)
    if [ -n "${APPLE_API_KEY_PATH:-}" ]; then
        auth+=(-authenticationKeyPath "$APPLE_API_KEY_PATH" -authenticationKeyID "$APPLE_API_KEY_ID" -authenticationKeyIssuerID "$APPLE_API_ISSUER_ID")
    fi
    xcodebuild build -quiet \
        -project apps/ios/Koan.xcodeproj -scheme Koan -configuration {{config}} \
        -destination "id=$phone" -derivedDataPath target/ios-build \
        "${auth[@]}"
    xcrun devicectl device install app --device "$phone" \
        "target/ios-build/Build/Products/{{config}}-iphoneos/koan.app"
    xcrun devicectl device process launch --device "$phone" {{bundle_id}}

# Type-check the shared SwiftUI sources against the tvOS SDK: what
# `ios-typecheck` is for the phone. The excluded files are the Mac's shell and
# sidebar, and the phone's Live Activity, none of which tvOS has.
tv-typecheck: macos-ffi
    #!/usr/bin/env bash
    set -euo pipefail
    shell=(KoanApp Hotkeys TextFocus EditCommands MenuShortcuts ShortcutsSheet SidebarView)
    find_args=()
    for f in "${shell[@]}"; do find_args+=(! -name "$f.swift"); done
    mod=$(mktemp -d)
    trap 'rm -rf "$mod"' EXIT
    ffi={{app_dir}}/Sources/koan_ffiFFI
    target=arm64-apple-tvos{{tv_deployment_target}}-simulator
    xcrun -sdk appletvsimulator swiftc -target "$target" -swift-version 6 \
        -package-name koan \
        -emit-module -module-name KoanFFI -emit-module-path "$mod/KoanFFI.swiftmodule" \
        -Xcc -fmodule-map-file="$PWD/$ffi/module.modulemap" -I "$PWD/$ffi" \
        {{app_dir}}/Sources/KoanFFI/koan_ffi.swift
    xcrun -sdk appletvsimulator swiftc -target "$target" -swift-version 6 \
        -package-name koan \
        -wmo -emit-sil -o /dev/null -module-name Koan -I "$mod" \
        -Xcc -fmodule-map-file="$PWD/$ffi/module.modulemap" -I "$PWD/$ffi" \
        $(find {{app_dir}}/Sources/KoanIOS -name '*.swift' ! -name 'RemoteActivity*') \
        $(find {{app_dir}}/Sources/KoanTV -name '*.swift') \
        $(find {{app_dir}}/Sources/Koan -name '*.swift' "${find_args[@]}")
    echo "the shared sources still build for tvOS"

# Build the television app for the simulator, unsigned: the engine for tvOS,
# the bindings from it, and the Xcode project's KoanTV scheme. What CI runs.
# arm64 only: a generic simulator destination also builds x86_64, which the
# engine is not built for.
tv-build: (tv-ffi "appletvsimulator") ios-project
    xcodebuild build -quiet \
        -project apps/ios/Koan.xcodeproj -scheme KoanTV \
        -destination 'generic/platform=tvOS Simulator' \
        -derivedDataPath target/tv-build CODE_SIGNING_ALLOWED=NO ARCHS=arm64

# Build the Rust engine for a tvOS SDK and stage it for the Swift link.
#
# `appletvsimulator` or `appletvos`, staged under the SDK's own name so the
# Xcode project finds the right one through `$(PLATFORM_NAME)`.
tv-ffi platform="appletvsimulator":
    #!/usr/bin/env bash
    set -euo pipefail
    case "{{platform}}" in
        appletvsimulator) triple=aarch64-apple-tvos-sim ;;
        appletvos) triple=aarch64-apple-tvos ;;
        *) echo "unknown platform: {{platform}}" >&2; exit 1 ;;
    esac
    # As in `ios-ffi`: C dependencies compile against the current SDK, and
    # rustc must target the same version.
    export TVOS_DEPLOYMENT_TARGET={{tv_deployment_target}}
    out=target/tv-{{tv_deployment_target}}
    cargo build --release -p koan-ffi --target "$triple" --target-dir "$out"
    rm -rf "target/tv-link/{{platform}}" && mkdir -p "target/tv-link/{{platform}}"
    cp "$out/$triple/release/libkoan_ffi.a" "target/tv-link/{{platform}}/"
    just ffi-bindings "target/tv-link/{{platform}}/libkoan_ffi.a"
    echo "koan-ffi ready for {{platform}}"

# Build, install and launch koan on an Apple TV simulator.
#
# A booted Apple TV if there is one, otherwise the first on a runtime that can
# run the deployment target.
tv-run: (tv-ffi "appletvsimulator") ios-project
    #!/usr/bin/env bash
    set -euo pipefail
    sim=$(xcrun simctl list devices available -j \
        | python3 -c 'import json,sys; want=int("{{tv_deployment_target}}".split(".")[0]); ds=[d for k,v in json.load(sys.stdin)["devices"].items() if "tvOS-" in k and int(k.split("tvOS-")[1].split("-")[0])>=want for d in v if d["isAvailable"] and "Apple TV" in d["name"]]; print(next((d["udid"] for d in ds if d["state"]=="Booted"), ds[0]["udid"] if ds else ""))')
    [ -n "$sim" ] || { echo "No Apple TV simulator on tvOS {{tv_deployment_target}} or later." >&2; exit 1; }
    xcrun simctl boot "$sim" 2>/dev/null || true
    open -a Simulator
    xcrun simctl bootstatus "$sim" -b
    xcodebuild build -quiet \
        -project apps/ios/Koan.xcodeproj -scheme KoanTV -configuration Debug \
        -destination "id=$sim" -derivedDataPath target/tv-build
    xcrun simctl install "$sim" target/tv-build/Build/Products/Debug-appletvsimulator/koan.app
    xcrun simctl launch "$sim" {{bundle_id}}

# Build, install and launch on the Apple TV on the network.
#
# Signed as `ios-phone` is, with the personal team unless APPLE_TEAM_ID says
# otherwise. The TV must be paired in Xcode first.
tv-device config="Debug": (tv-ffi "appletvos")
    #!/usr/bin/env bash
    set -euo pipefail
    tv=$(xcrun devicectl list devices | awk '/Apple TV/ && /physical/' \
        | grep -oE '[0-9a-f]{40}|[0-9A-F]{8}-([0-9A-F]{4}-){3}[0-9A-F]{12}' | head -1 || true)
    [ -n "$tv" ] || { echo "No Apple TV found — pair it in Xcode, and wake it." >&2; exit 1; }
    APPLE_TEAM_ID=${APPLE_TEAM_ID:-2256Q92VF2} just ios-project
    auth=(-allowProvisioningUpdates)
    if [ -n "${APPLE_API_KEY_PATH:-}" ]; then
        auth+=(-authenticationKeyPath "$APPLE_API_KEY_PATH" -authenticationKeyID "$APPLE_API_KEY_ID" -authenticationKeyIssuerID "$APPLE_API_ISSUER_ID")
    fi
    xcodebuild build -quiet \
        -project apps/ios/Koan.xcodeproj -scheme KoanTV -configuration {{config}} \
        -destination "id=$tv" -derivedDataPath target/tv-build \
        "${auth[@]}"
    xcrun devicectl device install app --device "$tv" \
        "target/tv-build/Build/Products/{{config}}-appletvos/koan.app"
    xcrun devicectl device process launch --device "$tv" {{bundle_id}}

# Walk the television app with the remote on a simulator, screenshotting each
# page into target/tv-walk. Given a music folder, it serves that from a
# throwaway koan on a free port and signs in as its owner; otherwise it signs in
# with the account in the environment: KOAN_REMOTE__URL, KOAN_REMOTE__USERNAME
# and KOAN_REMOTE__API_KEY (or __PASSWORD). The app is reinstalled first, so
# nothing carries over from an earlier run.
tv-walk library="": (tv-ffi "appletvsimulator") ios-project
    #!/usr/bin/env bash
    set -euo pipefail
    out=target/tv-walk
    rm -rf "$out" && mkdir -p "$out"
    cleanup=()
    trap 'for c in "${cleanup[@]}"; do eval "$c"; done' EXIT
    if [ -n "{{library}}" ]; then
        url=$(just _demo-server "{{library}}" "$out")
        cleanup+=("just _demo-server-stop '$out'")
        export KOAN_REMOTE__ENABLED=true KOAN_REMOTE__URL=$url KOAN_REMOTE__USERNAME=owner \
            KOAN_REMOTE__API_KEY=$(cat "$out/server.key")
        unset KOAN_REMOTE__PASSWORD
        export KOAN_WALK_SEARCH=${KOAN_WALK_SEARCH:-Harbour}
    fi
    sim=$(xcrun simctl list devices available -j \
        | python3 -c 'import json,sys; ds=[d for k,v in json.load(sys.stdin)["devices"].items() if "tvOS-" in k for d in v if d["isAvailable"] and "Apple TV" in d["name"]]; print(next((d["udid"] for d in ds if d["state"]=="Booted"), ds[0]["udid"] if ds else ""))')
    [ -n "$sim" ] || { echo "No Apple TV simulator." >&2; exit 1; }
    xcrun simctl boot "$sim" 2>/dev/null || true
    xcrun simctl bootstatus "$sim" -b >/dev/null
    # A booted simulator is a running copy of tvOS; leave none behind.
    cleanup+=("xcrun simctl shutdown '$sim'")
    xcrun simctl uninstall "$sim" {{bundle_id}} 2>/dev/null || true
    for v in KOAN_REMOTE__ENABLED KOAN_REMOTE__URL KOAN_REMOTE__USERNAME KOAN_REMOTE__API_KEY KOAN_REMOTE__PASSWORD KOAN_WALK_SETTLE KOAN_WALK_SEARCH KOAN_WALK_PLAYLIST; do
        [ -n "${!v:-}" ] && export "TEST_RUNNER_$v=${!v}"
    done
    xcodebuild test -quiet \
        -project apps/ios/Koan.xcodeproj -scheme KoanTV \
        -destination "id=$sim" -derivedDataPath target/tv-build \
        -only-testing:KoanTVUITests/TVWalkTests \
        -resultBundlePath "$out/walk.xcresult" || true
    xcrun xcresulttool export attachments --path "$out/walk.xcresult" --output-path "$out"
    echo "screenshots in $out"

# Pair a signed-out television, end to end: `TVPairTests` asks for a code on
# the simulator and approves it as a phone would. `outcome` is `approve`,
# `decline` or `expire` (against a server whose codes last 20 seconds). The
# server and approver are KOAN_PAIR_SERVER, KOAN_PAIR_USER and
# KOAN_PAIR_PASSWORD, or a throwaway koan and its owner. Screenshots land in
# target/tv-pair-`outcome`.
tv-pair outcome="approve": (tv-ffi "appletvsimulator") ios-project
    #!/usr/bin/env bash
    set -euo pipefail
    out=target/tv-pair-{{outcome}}
    rm -rf "$out" && mkdir -p "$out"
    cleanup=()
    trap 'for c in "${cleanup[@]}"; do eval "$c"; done' EXIT
    server=${KOAN_PAIR_SERVER:-}
    user=${KOAN_PAIR_USER:-}
    password=${KOAN_PAIR_PASSWORD:-}
    if [ -z "$server" ] || [ "{{outcome}}" = expire ]; then
        [ "{{outcome}}" = expire ] && export KOAN_PAIR_TTL_SECS=20
        server=$(just _demo-server "" "$out")
        cleanup+=("just _demo-server-stop '$out'")
        user=owner
        password=$(cat "$out/server.password")
    fi
    sim=$(xcrun simctl list devices available -j \
        | python3 -c 'import json,sys; ds=[d for k,v in json.load(sys.stdin)["devices"].items() if "tvOS-" in k for d in v if d["isAvailable"] and "Apple TV" in d["name"]]; print(next((d["udid"] for d in ds if d["state"]=="Booted"), ds[0]["udid"] if ds else ""))')
    xcrun simctl boot "$sim" 2>/dev/null || true
    xcrun simctl bootstatus "$sim" -b >/dev/null
    cleanup+=("xcrun simctl shutdown '$sim'")
    # Signed out from the start: a fresh install holds no account.
    xcrun simctl uninstall "$sim" {{bundle_id}} 2>/dev/null || true
    TEST_RUNNER_KOAN_PAIR_SERVER=$server TEST_RUNNER_KOAN_PAIR_USER=$user \
    TEST_RUNNER_KOAN_PAIR_PASSWORD=$password TEST_RUNNER_KOAN_PAIR_OUTCOME={{outcome}} \
    xcodebuild test -quiet \
        -project apps/ios/Koan.xcodeproj -scheme KoanTV \
        -destination "id=$sim" -derivedDataPath target/tv-build \
        -only-testing:KoanTVUITests/TVPairTests \
        -resultBundlePath "$out/pair.xcresult" >"$out/test.log" 2>&1 || true
    xcrun xcresulttool export attachments --path "$out/pair.xcresult" --output-path "$out" >/dev/null
    [ -f "$out/server.log" ] && grep -E "pair:|link:" "$out/server.log" || true
    if xcrun xcresulttool get test-results summary --path "$out/pair.xcresult" 2>/dev/null \
        | python3 -c 'import json,sys; sys.exit(json.load(sys.stdin).get("result") != "Passed")'; then
        echo "pairing ({{outcome}}) passed; screenshots in $out"
    else
        echo "FAIL: pairing ({{outcome}}); see $out" >&2
        exit 1
    fi

# The television finds its server instead of being told it: the Mac announces
# a throwaway koan on the network as a signed-in kōan device does (`_koan._tcp`
# with the address in TXT `server`), and a signed-out TV simulator offers it,
# asks it for a code and is approved over the API. What a device announces is
# covered by the unit tests in remote/nearby.rs. Screenshots land in
# target/tv-discover.
tv-discover: (tv-ffi "appletvsimulator") ios-project
    #!/usr/bin/env bash
    set -euo pipefail
    out=target/tv-discover
    rm -rf "$out" && mkdir -p "$out"
    cleanup=()
    trap 'for c in "${cleanup[@]}"; do eval "$c"; done' EXIT
    tv=$(xcrun simctl list devices available -j \
        | python3 -c 'import json,sys; ds=[d for k,v in json.load(sys.stdin)["devices"].items() if "tvOS-" in k for d in v if d["isAvailable"] and "Apple TV" in d["name"]]; print(next((d["udid"] for d in ds if d["state"]=="Booted"), ds[0]["udid"] if ds else ""))')
    [ -n "$tv" ] || { echo "No Apple TV simulator." >&2; exit 1; }
    xcodebuild build-for-testing -quiet \
        -project apps/ios/Koan.xcodeproj -scheme KoanTV \
        -destination "id=$tv" -derivedDataPath target/tv-build
    # On the network rather than loopback, and named by the Mac's address
    # there, which the simulator reaches: a loopback address is never
    # announced.
    url=$(KOAN_GRAPHQL__BIND=0.0.0.0 just _demo-server "" "$out")
    cleanup+=("just _demo-server-stop '$out'")
    password=$(cat "$out/server.password")
    lan=$(ipconfig getifaddr en0 || ipconfig getifaddr en1)
    url=${url/127.0.0.1/$lan}
    # The port is never dialled: the TV only reads the announcement.
    dns-sd -R koan-tv-discover _koan._tcp local 9 \
        "id=$(uuidgen | tr A-Z a-z)" platform=macos "server=$url" >"$out/announce.log" 2>&1 &
    cleanup+=("kill $! 2>/dev/null || true")
    xcrun simctl boot "$tv" 2>/dev/null || true
    xcrun simctl bootstatus "$tv" -b >/dev/null
    cleanup+=("xcrun simctl shutdown '$tv'")
    xcrun simctl uninstall "$tv" {{bundle_id}} 2>/dev/null || true
    TEST_RUNNER_KOAN_PAIR_DISCOVER=1 TEST_RUNNER_KOAN_PAIR_SERVER=$url \
    TEST_RUNNER_KOAN_PAIR_USER=owner TEST_RUNNER_KOAN_PAIR_PASSWORD=$password \
    xcodebuild test-without-building -quiet \
        -project apps/ios/Koan.xcodeproj -scheme KoanTV \
        -destination "id=$tv" -derivedDataPath target/tv-build \
        -only-testing:KoanTVUITests/TVPairTests \
        -resultBundlePath "$out/tv.xcresult" >"$out/tv-test.log" 2>&1 || true
    xcrun xcresulttool export attachments --path "$out/tv.xcresult" --output-path "$out/tv" >/dev/null 2>&1 || true
    grep -E "pair:|link:" "$out/server.log" || true
    if xcrun xcresulttool get test-results summary --path "$out/tv.xcresult" 2>/dev/null \
        | python3 -c 'import json,sys; sys.exit(json.load(sys.stdin).get("result") != "Passed")'; then
        echo "discovered: the television found its server on the network and signed in (screenshots in $out)"
    else
        echo "FAIL: the television did not find its server or sign in; see $out" >&2
        exit 1
    fi

# Every other way onto the television, one route at a time from a fresh
# install: `TVSignInTests` with the route in KOAN_SIGNIN_ROUTE. Against
# KOAN_SIGNIN_SERVER, with KOAN_SIGNIN_USER and its KOAN_SIGNIN_PASSWORD and
# KOAN_SIGNIN_API_KEY, and an invite in KOAN_SIGNIN_INVITE. Screenshots land
# in target/tv-signin/`route`.
tv-signin routes="password apikey invite signout wrong-password unreachable menu revoked": (tv-ffi "appletvsimulator") ios-project
    #!/usr/bin/env bash
    set -euo pipefail
    out=target/tv-signin
    rm -rf "$out" && mkdir -p "$out"
    sim=$(xcrun simctl list devices available -j \
        | python3 -c 'import json,sys; ds=[d for k,v in json.load(sys.stdin)["devices"].items() if "tvOS-" in k for d in v if d["isAvailable"] and "Apple TV" in d["name"]]; print(next((d["udid"] for d in ds if d["state"]=="Booted"), ds[0]["udid"] if ds else ""))')
    xcodebuild build-for-testing -quiet \
        -project apps/ios/Koan.xcodeproj -scheme KoanTV \
        -destination "id=$sim" -derivedDataPath target/tv-build
    xcrun simctl boot "$sim" 2>/dev/null || true
    xcrun simctl bootstatus "$sim" -b >/dev/null
    trap 'xcrun simctl shutdown "$sim"' EXIT
    failed=()
    for route in {{routes}}; do
        server=${KOAN_SIGNIN_SERVER:-}
        user=${KOAN_SIGNIN_USER:-}
        secret=${KOAN_SIGNIN_PASSWORD:-}
        [ "$route" = apikey ] && secret=${KOAN_SIGNIN_API_KEY:-}
        # Revoking a key is done on a throwaway server, to a key of its own.
        if [ "$route" = revoked ]; then
            mkdir -p "$out/revoked"
            server=$(just _demo-server "" "$out/revoked")
            trap 'just _demo-server-stop "$out/revoked"; xcrun simctl shutdown "$sim"' EXIT
            user=owner
            secret=$(cat "$out/revoked/server.key")
        fi
        xcrun simctl uninstall "$sim" {{bundle_id}} 2>/dev/null || true
        TEST_RUNNER_KOAN_SIGNIN_ROUTE=$route \
        TEST_RUNNER_KOAN_SIGNIN_SERVER=$server \
        TEST_RUNNER_KOAN_SIGNIN_USER=$user \
        TEST_RUNNER_KOAN_SIGNIN_SECRET=$secret \
        TEST_RUNNER_KOAN_SIGNIN_INVITE=${KOAN_SIGNIN_INVITE:-} \
        xcodebuild test-without-building -quiet \
            -project apps/ios/Koan.xcodeproj -scheme KoanTV \
            -destination "id=$sim" -derivedDataPath target/tv-build \
            -only-testing:KoanTVUITests/TVSignInTests \
            -resultBundlePath "$out/$route.xcresult" >"$out/$route.log" 2>&1 || true
        mkdir -p "$out/$route"
        xcrun xcresulttool export attachments --path "$out/$route.xcresult" --output-path "$out/$route" >/dev/null 2>&1 || true
        # Which credential the television ended up holding, the secret masked:
        # a password sign-in to a koan server is traded for an API key.
        local_toml="$(xcrun simctl get_app_container "$sim" {{bundle_id}} data 2>/dev/null)/Library/Caches/koan-config/config.local.toml"
        if [ -f "$local_toml" ]; then
            grep -E '^[[:space:]]*(url|username|password|api_key)[[:space:]]*=' "$local_toml" \
                | sed -E 's/^([[:space:]]*(password|api_key)[[:space:]]*=).*/\1 <set>/' > "$out/$route/credential.txt" || true
            echo "$route holds: $(tr '\n' ' ' < "$out/$route/credential.txt")"
        fi
        if xcrun xcresulttool get test-results summary --path "$out/$route.xcresult" 2>/dev/null \
            | python3 -c 'import json,sys; sys.exit(json.load(sys.stdin).get("result") != "Passed")'; then
            echo "$route: passed"
        else
            echo "$route: FAILED"
            failed+=("$route")
        fi
    done
    echo "screenshots in $out"
    [ ${#failed[@]} -eq 0 ] || { echo "failed: ${failed[*]}" >&2; exit 1; }

# Pair the way a person does: the television shows its QR code, a phone reads
# it and its owner taps Allow. The code is read off the simulator's screen
# (apps/ios/tools/qrdecode.swift), and the link opened on an iPhone simulator
# signed in as the throwaway server's owner, through `koan://pair` since an
# unsigned build is not given the universal link. Screenshots of both screens
# land in target/tv-pair-qr. Two simulators, both shut down at the end.
tv-pair-qr: (tv-ffi "appletvsimulator") (ios-ffi "iphonesimulator") ios-project
    #!/usr/bin/env bash
    set -euo pipefail
    out=target/tv-pair-qr
    rm -rf "$out" && mkdir -p "$out"
    cleanup=()
    trap 'for c in "${cleanup[@]}"; do eval "$c"; done' EXIT
    pick() {
        xcrun simctl list devices available -j | python3 -c 'import json,sys; want=sys.argv[1]; ds=[d for k,v in json.load(sys.stdin)["devices"].items() if want+"-" in k for d in v if d["isAvailable"]]; print(next((d["udid"] for d in ds if d["state"]=="Booted"), ds[0]["udid"] if ds else ""))' "$1"
    }
    tv=$(pick tvOS)
    phone=$(pick iOS)
    [ -n "$tv" ] && [ -n "$phone" ] || { echo "Needs an Apple TV and an iPhone simulator." >&2; exit 1; }
    # Both test bundles first: once the code is on screen it has ten minutes.
    xcodebuild build-for-testing -quiet \
        -project apps/ios/Koan.xcodeproj -scheme KoanTV \
        -destination "id=$tv" -derivedDataPath target/tv-build
    xcodebuild build-for-testing -quiet \
        -project apps/ios/Koan.xcodeproj -scheme Koan \
        -destination "id=$phone" -derivedDataPath target/ios-build
    url=$(just _demo-server "" "$out")
    cleanup+=("just _demo-server-stop '$out'")
    password=$(cat "$out/server.password")
    for sim in "$tv" "$phone"; do
        xcrun simctl boot "$sim" 2>/dev/null || true
        xcrun simctl bootstatus "$sim" -b >/dev/null
        cleanup+=("xcrun simctl shutdown '$sim'")
        xcrun simctl uninstall "$sim" {{bundle_id}} 2>/dev/null || true
    done
    # The television asks for a code and waits for someone to allow it.
    TEST_RUNNER_KOAN_PAIR_SERVER=$url xcodebuild test-without-building -quiet \
        -project apps/ios/Koan.xcodeproj -scheme KoanTV \
        -destination "id=$tv" -derivedDataPath target/tv-build \
        -only-testing:KoanTVUITests/TVPairTests \
        -resultBundlePath "$out/tv.xcresult" >"$out/tv-test.log" 2>&1 &
    tv_test=$!
    link=""
    for _ in $(seq 120); do
        xcrun simctl io "$tv" screenshot "$out/tv-screen.png" >/dev/null 2>&1 || true
        link=$(swift apps/ios/tools/qrdecode.swift "$out/tv-screen.png" 2>/dev/null | head -1 || true)
        [ -n "$link" ] && break
        sleep 3
    done
    [ -n "$link" ] || { echo "FAIL: no QR code on the television" >&2; exit 1; }
    mv "$out/tv-screen.png" "$out/tv-qr.png"
    echo "read from the television: $link"
    case "$link" in https://koan.rocks/pair*'#'*) ;; *) echo "FAIL: not a pairing link" >&2; exit 1 ;; esac
    TEST_RUNNER_KOAN_PAIR_LINK="koan://pair#${link#*#}" \
    TEST_RUNNER_KOAN_REMOTE__ENABLED=true TEST_RUNNER_KOAN_REMOTE__URL=$url \
    TEST_RUNNER_KOAN_REMOTE__USERNAME=owner TEST_RUNNER_KOAN_REMOTE__API_KEY="$(cat "$out/server.key")" \
    xcodebuild test-without-building -quiet \
        -project apps/ios/Koan.xcodeproj -scheme Koan \
        -destination "id=$phone" -derivedDataPath target/ios-build \
        -only-testing:KoanUITests/PairApproveTests \
        -resultBundlePath "$out/phone.xcresult" >"$out/phone-test.log" 2>&1 || true
    wait "$tv_test" || true
    for r in tv phone; do
        [ -d "$out/$r.xcresult" ] && xcrun xcresulttool export attachments \
            --path "$out/$r.xcresult" --output-path "$out/$r" >/dev/null
    done
    grep -E "pair:|link:" "$out/server.log" || true
    if xcrun xcresulttool get test-results summary --path "$out/tv.xcresult" 2>/dev/null \
        | python3 -c 'import json,sys; sys.exit(json.load(sys.stdin).get("result") != "Passed")'; then
        echo "paired: the television signed in from the phone's approval (screenshots in $out)"
    else
        echo "FAIL: the television did not sign in; see $out" >&2
        exit 1
    fi

# A throwaway koan for the simulator recipes: `library` (or nothing) served on
# a free port from a configuration of its own, with an owner account whose
# password lands in `out`/server.password and an API key in server.key. Prints
# the server's address.
# `_demo-server-stop` ends it.
_demo-server library out:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo build -q -p koan-cli
    dir=$(mktemp -d)
    port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
    password=demo-$RANDOM$RANDOM
    [ -z "{{library}}" ] || printf '[library]\nfolders = ["%s"]\n' "$(cd "{{library}}" && pwd)" > "$dir/config.toml"
    KOAN_CONFIG_DIR=$dir KOAN_USERNAME=owner KOAN_PASSWORD=$password target/debug/koan auth setup >/dev/null
    [ -z "{{library}}" ] || KOAN_CONFIG_DIR=$dir target/debug/koan scan >/dev/null 2>&1
    KOAN_CONFIG_DIR=$dir KOAN_SUBSONIC__ENABLED=true KOAN_GRAPHQL__AUTH_ENABLED=true \
        nohup target/debug/koan --headless --port "$port" >"{{out}}/server.log" 2>&1 &
    echo "$! $dir" > "{{out}}/server.pid"
    echo "$password" > "{{out}}/server.password"
    # A key as well: a koan server refuses token auth with a password, which is
    # all an account given through the environment can use.
    KOAN_CONFIG_DIR=$dir target/debug/koan auth api-key create --username owner --name simulator </dev/null 2>/dev/null \
        | sed 's/\x1b\[[0-9;]*m//g' | awk 'NF == 1 && length($1) > 30 { print $1 }' > "{{out}}/server.key"
    for _ in $(seq 60); do
        curl -sf "http://127.0.0.1:$port/rest/ping?f=json" >/dev/null && break
        sleep 0.5
    done
    echo "http://127.0.0.1:$port"

_demo-server-stop out:
    #!/usr/bin/env bash
    read -r pid dir < "{{out}}/server.pid" || exit 0
    kill "$pid" 2>/dev/null || true
    rm -rf "$dir" "{{out}}/server.pid" "{{out}}/server.password" "{{out}}/server.key"

# Clear the configuration as tvOS does when it runs short of space, and check
# the television is still signed in. Plants a sign-in in the simulator's copy
# of the app, launches it so the configuration is mirrored into its
# preferences, deletes `Library/Caches/koan-config`, and launches it again.
tv-kept: (tv-ffi "appletvsimulator") ios-project
    #!/usr/bin/env bash
    set -euo pipefail
    sim=$(xcrun simctl list devices available -j \
        | python3 -c 'import json,sys; ds=[d for k,v in json.load(sys.stdin)["devices"].items() if "tvOS-" in k for d in v if d["isAvailable"] and "Apple TV" in d["name"]]; print(next((d["udid"] for d in ds if d["state"]=="Booted"), ds[0]["udid"] if ds else ""))')
    [ -n "$sim" ] || { echo "No Apple TV simulator." >&2; exit 1; }
    xcrun simctl boot "$sim" 2>/dev/null || true
    xcrun simctl bootstatus "$sim" -b >/dev/null
    trap 'xcrun simctl shutdown "$sim"' EXIT
    xcodebuild build -quiet \
        -project apps/ios/Koan.xcodeproj -scheme KoanTV -configuration Debug \
        -destination "id=$sim" -derivedDataPath target/tv-build
    xcrun simctl uninstall "$sim" {{bundle_id}}
    xcrun simctl install "$sim" target/tv-build/Build/Products/Debug-appletvsimulator/koan.app
    data=$(xcrun simctl get_app_container "$sim" {{bundle_id}} data)
    dir="$data/Library/Caches/koan-config"
    prefs="$data/Library/Preferences/{{bundle_id}}.plist"
    mkdir -p "$dir"
    printf '[remote]\nenabled = true\nurl = "http://127.0.0.1:4812"\nusername = "kept"\napi_key = "kept-key"\n\n[devices]\nnearby = false\n' \
        > "$dir/config.local.toml"
    wait_for() { for _ in $(seq 40); do eval "$1" && return 0; sleep 0.5; done; return 1; }
    xcrun simctl launch "$sim" {{bundle_id}} >/dev/null
    wait_for 'plutil -p "$prefs" 2>/dev/null | grep -q "config.local.toml"' \
        || { echo "FAIL: the configuration was not mirrored into the preferences" >&2; exit 1; }
    xcrun simctl terminate "$sim" {{bundle_id}}
    rm -rf "$dir"
    xcrun simctl launch "$sim" {{bundle_id}} >/dev/null
    wait_for 'grep -q "username = \"kept\"" "$dir/config.local.toml" 2>/dev/null' \
        || { echo "FAIL: the configuration did not come back after Caches was cleared" >&2; exit 1; }
    sleep 10
    xcrun simctl io "$sim" screenshot target/tv-kept.png >/dev/null
    echo "kept: signed in as kept after Caches was cleared (target/tv-kept.png)"

# Sign a television in through an invite: the simulator by default, or
# `device=tv` for the Apple TV paired with Xcode (signed as `tv-device` is).
tv-join link device="sim": ios-project
    #!/usr/bin/env bash
    set -euo pipefail
    if [ "{{device}}" = tv ]; then
        just tv-ffi appletvos
        dest=$(xcrun devicectl list devices | awk '/Apple TV/ && /physical/' \
            | grep -oE '[0-9a-f]{40}|[0-9A-F]{8}-([0-9A-F]{4}-){3}[0-9A-F]{12}' | head -1)
        APPLE_TEAM_ID=${APPLE_TEAM_ID:-2256Q92VF2} just ios-project
    else
        just tv-ffi appletvsimulator
        dest=$(xcrun simctl list devices available -j \
            | python3 -c 'import json,sys; ds=[d for k,v in json.load(sys.stdin)["devices"].items() if "tvOS-" in k for d in v if d["isAvailable"] and "Apple TV" in d["name"]]; print(next((d["udid"] for d in ds if d["state"]=="Booted"), ds[0]["udid"] if ds else ""))')
        trap 'xcrun simctl shutdown "$dest"' EXIT
    fi
    auth=(-allowProvisioningUpdates)
    if [ -n "${APPLE_API_KEY_PATH:-}" ]; then
        auth+=(-authenticationKeyPath "$APPLE_API_KEY_PATH" -authenticationKeyID "$APPLE_API_KEY_ID" -authenticationKeyIssuerID "$APPLE_API_ISSUER_ID")
    fi
    rm -rf target/tv-join.xcresult
    TEST_RUNNER_KOAN_INVITE_LINK='{{link}}' xcodebuild test -quiet \
        -project apps/ios/Koan.xcodeproj -scheme KoanTV \
        -destination "id=$dest" -derivedDataPath target/tv-build \
        -resultBundlePath target/tv-join.xcresult \
        -only-testing:KoanTVUITests/TVInviteTests "${auth[@]}"
    echo "joined through the invite"

# Archive for a device, sign, and upload to TestFlight.
#
# Signing is cloud-managed: xcodebuild asks App Store Connect for the
# distribution certificate and profile with the API key, so there is no
# certificate or profile to keep in a secret or a keychain. That needs a key
# with the Admin role. The build number has to rise with every upload of a
# version; CI passes the time in seconds.
#
# Needs APPLE_TEAM_ID, APPLE_API_KEY_PATH (the .p8), APPLE_API_KEY_ID and
# APPLE_API_ISSUER_ID in the environment.
ios-testflight build: (ios-ffi "iphoneos") (ios-project build)
    #!/usr/bin/env bash
    set -euo pipefail
    : "${APPLE_TEAM_ID:?}" "${APPLE_API_KEY_PATH:?}" "${APPLE_API_KEY_ID:?}" "${APPLE_API_ISSUER_ID:?}"
    out=target/ios-archive
    rm -rf "$out" && mkdir -p "$out"
    auth=(
        -allowProvisioningUpdates
        -authenticationKeyPath "$APPLE_API_KEY_PATH"
        -authenticationKeyID "$APPLE_API_KEY_ID"
        -authenticationKeyIssuerID "$APPLE_API_ISSUER_ID"
    )
    # KOAN_STORE leaves out what App Review would reject, such as the silent
    # keepalive; development builds keep it.
    xcodebuild archive \
        -project apps/ios/Koan.xcodeproj -scheme Koan \
        -destination 'generic/platform=iOS' \
        -archivePath "$out/koan.xcarchive" \
        SWIFT_ACTIVE_COMPILATION_CONDITIONS='$(inherited) KOAN_STORE' \
        "${auth[@]}" | tail -n 20
    # `upload` sends the export straight to App Store Connect, where it
    # appears under TestFlight once processed.
    cat > "$out/ExportOptions.plist" <<PLIST
    <?xml version="1.0" encoding="UTF-8"?>
    <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
    <plist version="1.0">
    <dict>
        <key>method</key><string>app-store-connect</string>
        <key>destination</key><string>upload</string>
        <key>signingStyle</key><string>automatic</string>
        <key>teamID</key><string>$APPLE_TEAM_ID</string>
        <key>uploadSymbols</key><true/>
        <key>manageAppVersionAndBuildNumber</key><false/>
    </dict>
    </plist>
    PLIST
    xcodebuild -exportArchive \
        -archivePath "$out/koan.xcarchive" \
        -exportOptionsPlist "$out/ExportOptions.plist" \
        -exportPath "$out/export" \
        "${auth[@]}"
    echo "uploaded build {{build}} to App Store Connect"

# Archive the television app, sign, and upload to TestFlight, as
# `ios-testflight` does for the phone. The same bundle id, so the same App
# Store Connect record, under its tvOS platform; a build number has to rise
# with every tvOS upload of a version.
tv-testflight build: (tv-ffi "appletvos") (ios-project build)
    #!/usr/bin/env bash
    set -euo pipefail
    : "${APPLE_TEAM_ID:?}" "${APPLE_API_KEY_PATH:?}" "${APPLE_API_KEY_ID:?}" "${APPLE_API_ISSUER_ID:?}"
    out=target/tv-archive
    rm -rf "$out" && mkdir -p "$out"
    auth=(
        -allowProvisioningUpdates
        -authenticationKeyPath "$APPLE_API_KEY_PATH"
        -authenticationKeyID "$APPLE_API_KEY_ID"
        -authenticationKeyIssuerID "$APPLE_API_ISSUER_ID"
    )
    xcodebuild archive \
        -project apps/ios/Koan.xcodeproj -scheme KoanTV \
        -destination 'generic/platform=tvOS' \
        -archivePath "$out/koan.xcarchive" \
        SWIFT_ACTIVE_COMPILATION_CONDITIONS='$(inherited) KOAN_STORE' \
        "${auth[@]}" | tail -n 20
    cat > "$out/ExportOptions.plist" <<PLIST
    <?xml version="1.0" encoding="UTF-8"?>
    <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
    <plist version="1.0">
    <dict>
        <key>method</key><string>app-store-connect</string>
        <key>destination</key><string>upload</string>
        <key>signingStyle</key><string>automatic</string>
        <key>teamID</key><string>$APPLE_TEAM_ID</string>
        <key>uploadSymbols</key><true/>
        <key>manageAppVersionAndBuildNumber</key><false/>
    </dict>
    </plist>
    PLIST
    xcodebuild -exportArchive \
        -archivePath "$out/koan.xcarchive" \
        -exportOptionsPlist "$out/ExportOptions.plist" \
        -exportPath "$out/export" \
        "${auth[@]}"
    echo "uploaded tvOS build {{build}} to App Store Connect"

# Frame a walk's screenshots for the App Store: each screen on a blur of its
# own colours, captioned from apps/ios/store/captions.toml, at the size it was
# taken. `just ios-store-shots target/ios-walk target/shots-iphone`, and
# `captions-ipad` for an iPad's walk.
ios-store-shots src out captions="captions":
    uv run apps/ios/store/frame.py apps/ios/store/{{captions}}.toml {{src}} {{out}}

# Push the App Store listing (apps/ios/store/listing.toml) through the App
# Store Connect API: text, review details, the newest processed build, and
# screenshots when given as `iphone=DIR ipad=DIR`. Never submits for review.
# Needs the same APPLE_API_* environment as ios-testflight.
ios-store *screenshots:
    uv run apps/ios/store/push.py {{screenshots}}
