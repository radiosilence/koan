# koan on iOS

Making `apps/macos` a multiplatform SwiftUI app that also builds for iPhone, rather
than writing a second client against GraphQL. Every claim below was measured on the
tree, not estimated; the commands are given so they can be re-run when they rot.

## What was measured

### The Rust core builds for iOS as it stands

`koan-ffi` — and therefore all of `koan-core` — compiles and **links** for both iOS
targets after two changes:

```diff
 # crates/koan-core/Cargo.toml
+[target.'cfg(target_os = "ios")'.dependencies]
+apple-native-keyring-store = { version = "1", features = ["protected"] }
```
```diff
 // crates/koan-core/src/audio/mod.rs — platform_backend()
     #[cfg(target_os = "linux")]
     { Box::new(cpal_backend::CpalBackend::new()) }
+    #[cfg(target_os = "ios")]
+    { unimplemented!() }
```

```
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
export IPHONEOS_DEPLOYMENT_TARGET=26.0
cargo check -p koan-ffi  --target aarch64-apple-ios                # clean
cargo test  -p koan-core --target aarch64-apple-ios-sim --no-run   # links, 1m42s
cargo build -p koan-ffi  --target aarch64-apple-ios                # 564MB .a
cargo build -p koan-ffi  --target aarch64-apple-ios-sim            # 564MB .a
xcodebuild -create-xcframework \
  -library target/aarch64-apple-ios/debug/libkoan_ffi.a       -headers <hdrs> \
  -library target/aarch64-apple-ios-sim/debug/libkoan_ffi.a   -headers <hdrs> \
  -output koan_ffi.xcframework                                     # both slices
```

`IPHONEOS_DEPLOYMENT_TARGET` is not optional. Without it rustc targets
`arm64-apple-ios10.0.0` while every C dependency compiled against SDK 26, and the
cdylib link dies under a wall of "built for newer iOS version" warnings. `macos-ffi`
already exports `MACOSX_DEPLOYMENT_TARGET=26.0` for the same reason; the iOS recipe
needs the sibling.

The test build is the load-bearing one: it produces a linked Mach-O for the
simulator, so this is not "the type checker was happy". The `.xcframework` packages
both slices with the uniffi header and module map, which is the form an Xcode project
consumes. Every dependency resolves — symphonia,
bundled rusqlite, reqwest/rustls, `ring` + `aws-lc-rs`, `bliss-audio`, `realfft`,
`lofty`, `rayon`. `notify` has an iOS backend (kqueue rather than FSEvents) and
`dirs` resolves inside the app container, so `set_config_dir()` is already the hook
for pointing the database at it.

Without the keyring change the build stops at a `compile_error!`: iOS has no
file-based keychain, only the data-protection one, and that store is what
`apple-native-keyring-store` gates behind `protected`. This matters beyond the build
— see [Distribution](#distribution-is-the-real-gate).

Not yet verified: nothing has been *run*. No simulator runtime is installed on the
build machine, and that is the next cheap check (`xcrun simctl spawn` will run the
linked test binary directly, no app bundle required).

### 51 of 60 app files typecheck for iOS

The whole of `Sources/Koan` was copied, ported, and type-checked against the iOS
simulator SDK:

```
xcrun -sdk iphonesimulator swiftc -target arm64-apple-ios26.0-simulator \
  -swift-version 6 -typecheck -module-name Koan \
  -I <KoanFFI module> -Xcc -fmodule-map-file=Sources/koan_ffiFFI/module.modulemap \
  <sources>
```

It reaches **zero errors** with 19 files touched, ~139 changed lines, and one new
28-line `Compat.swift`. The uniffi-generated `koan_ffi.swift` compiles for iOS
unmodified.

Nine files are the macOS shell and were excluded rather than ported — they are the
work, and no amount of `#if` makes them a phone:

| Shell file | Lines | Why it doesn't cross |
|---|---|---|
| `KoanApp.swift` | 369 | `WindowGroup` + `Settings` scene + seven `CommandGroup`/`CommandMenu` blocks |
| `RootView.swift` | 394 | `NavigationSplitView` with a permanent sidebar |
| `SidebarView.swift` | 325 | ported, but it is a sidebar |
| `MenuShortcuts.swift` | 154 | the menu bar's shortcut table |
| `Hotkeys.swift` | 192 | `NSEvent` local monitor |
| `ShortcutsSheet.swift` | 97 | documents the above |
| `TextFocus.swift` | 73 | `NSText`/`NSWindow` notifications |
| `EditCommands.swift` | 50 | first-responder routing for menu enablement |
| `FilterField.swift` | 70 | `NSSearchField` representable; needs a `TextField` twin |

What the other 51 needed, in full:

| Change | Files | Shape |
|---|---|---|
| `NSImage` → `PlatformImage` typealias | 5 | `CoverArtCache`, `AlbumArtwork`, `ArtworkBleed`, `NowPlayingCentre`, `Palette`. Decode already goes through `CGImageSource`, so the bodies are unchanged; only construction (`NSImage(cgImage:size:)` vs `UIImage(cgImage:)`) and `cost()` needed a branch |
| `NSPasteboard` → `UIPasteboard` | 1 | 26 lines, same two representations |
| accent from `NSColor(named:)` | 1 | `Color("AccentColor")` on iOS; the asset catalog compiles for both platforms |
| `image.cgImage(forProposedRect:…)` | 1 | `UIImage.cgImage` on iOS |
| `.onDeleteCommand` / `.onExitCommand` | 4 | unavailable on iOS — fence, and swipe-to-delete instead |
| `.pointerStyle(.link)` | 1 | unavailable on iOS |
| `.toggleStyle(.checkbox)` | 1 | unavailable on iOS |
| `@Environment(\.controlActiveState)` | 1 | `\.scenePhase` on iOS — same "window came back, re-read config.toml" intent |
| `NSSavePanel` for M3U8 export | 1 | write into the container, hand to a share sheet |
| explicit `import UIKit` | 9 | files whose only import was AppKit lost Foundation with it |

The surprises were all in koan's favour: no `Table` anywhere (the one that would
have hurt); `draggable`/`dropDestination`, `contextMenu`, `fileImporter`,
`keyboardShortcut`, `openWindow`, `listStyle(.sidebar)` and even `.help()` all
compile for iOS; `onHover` no-ops rather than failing. `Navigator` not using
`NavigationStack` turns out to be a gift — browser-style history ports untouched.

### CoreAudio exists on iOS; the part koan uses does not

`audio/engine.rs` is AUHAL and `audio/device.rs` is the HAL device-property API.
From the iOS 26.5 SDK on this machine:

- `kAudioUnitSubType_HALOutput` is declared inside `#if !TARGET_OS_IPHONE` in
  `AUComponent.h`. The `#else` branch offers one thing: `kAudioUnitSubType_RemoteIO`.
- `CoreAudio.framework` ships exactly one header on iOS: `CoreAudioTypes.h`. There is
  no `AudioHardware.h`. The `AudioObject*` symbols are in `CoreAudio.tbd` but
  undeclared — SPI, so `coreaudio-sys` generates nothing for them.

So `engine.rs` + `device.rs` (905 lines) get an iOS sibling, and `AudioBackend`
absorbs it: the decode pipeline, `PlaybackTimeline` and the rtrb ring are untouched,
and the render block drains the consumer exactly as the AUHAL callback does.

What has no iOS equivalent at all is device enumeration, nominal-rate switching and
hog mode. `AVAudioSession.setPreferredSampleRate(_:)` is a request the system may
decline, and everything crosses the system mixer regardless. **koan cannot make the
bit-perfect claim on iOS.** A wired class-compliant DAC over USB-C will usually be
given the rate asked for; AirPlay and Bluetooth never will. The honest surface is
what #301 already built for macOS — show the user the output format and flag when it
isn't the source's.

New obligations with no macOS counterpart: session category and activation;
interruption handling (a call must pause and resume); route-change-on-unplug must
pause or iOS keeps playing out of the speaker; `UIBackgroundModes: audio` or the app
is suspended and the audio thread with it.

**Open:** raw `RemoteIO` keeps the callback in Rust and mirrors the existing macOS
code; `AVAudioEngine` + `AVAudioSourceNode` hands interruption and route negotiation
to the framework at the cost of a Swift-side render block and one more mixer node.
Less unsafe code, less control.

### SwiftPM cannot produce an iOS app

`apps/macos` is an `executableTarget` whose `.app` is assembled by hand in
`macos-bundle`. There is no iOS equivalent of that trick: an iOS bundle needs the iOS
SDK, a provisioning profile and a real signature, which means `xcodebuild` against an
Xcode project. The shape that follows:

- `Koan` becomes a **library** target holding `Sources/Koan`, still SwiftPM.
- Two thin app targets, macOS and iOS, each with its own scene root, Info.plist and
  entitlements.
- `just macos-ffi` grows a sibling that builds both iOS slices and packages
  everything as an `.xcframework` — the only form an Xcode project links cleanly
  across device and simulator. Verified above; the recipe is a dozen lines.
- CI gains an iOS build job; `macos-app`, `macos-verify` and `macos-dmg` all move
  onto `xcodebuild`.

This is the piece most likely to be underestimated, because it replaces the one part
of the build that currently works and is understood. Checking a `.xcodeproj` in makes
`project.pbxproj` a merge-conflict surface; XcodeGen or Tuist generates it from a
manifest instead, which is closer to how the justfile already works.

### Distribution is the real gate

A physical device needs the Apple Developer Program. Free provisioning gives 7-day
certificates and no `keychain-access-groups` entitlement — which is exactly what the
data-protection keychain store demands. The simulator is free and covers everything
up to and including audio; the phone is not. Same $99/yr that notarisation has so far
not been worth, spent for a harder reason to avoid.

### What the sandbox takes

`index/scanner.rs` and `organize.rs` have no meaning inside the container: no
`~/Music` to walk, no library to rename in place. What survives is the document
picker plus persisted security-scoped bookmarks — the scope is a process-wide sandbox
extension, so Rust's `std::fs` reads the path fine while it is held, and the existing
`import_paths` flow works on a folder the user hands over. The remote path is
untouched: Subsonic sync, streaming, the download queue and the cache all work as they
do today.

Whether iOS is remote-only or offers imported folders is a product decision and can be
deferred past the first build.

## Order of work

M1 blocks everything else. M2 is independent and can land on `main` immediately —
it is invisible on macOS.

### M1 — build system

`.xcframework` packaging, Xcode project (generated, not checked in), the macOS app
moved onto `xcodebuild` and still shipping.

*Done when:* `just macos-dmg` produces a launchable app as it does today, and
`xcodebuild -sdk iphonesimulator` builds an `.app` from the same library target.

### M2 — platform shims

The 19 files above, `#if os(macOS)` where the twin is trivial, a protocol where it
isn't. Ships on macOS with no behaviour change.

*Done when:* the `swiftc -typecheck` invocation from this document runs green in CI
as a lint, so the port cannot silently rot while nobody is looking at iOS.

### M3 — iOS audio backend

`IosAudioBackend` behind the existing trait. Sine first, then a FLAC, then a track
boundary.

*Done when:* two consecutive FLACs are gapless in the simulator; a simulated
interruption pauses and resumes; unplugging a route pauses rather than blaring.

### M4 — the phone shell

A second scene root over the same models: tab bar or browse stack, mini-player that
expands into a now-playing sheet, no menu bar. `QueueView` (613), `TransportBar`
(342) and the row components come across; `RootView` and `SidebarView` do not.

iPad can plausibly run the split view nearly as-is, which makes it the cheaper first
target and the less interesting one.

### M5 — library on a phone

Document picker, security-scoped bookmarks persisted across launches, `import_paths`
wired to what the picker returns. Or: decide iOS is remote-only and skip it.

## Open questions

- `RemoteIO` or `AVAudioEngine`? Control against the framework handling interruptions.
- Ship without a bit-perfect claim, or claim "source rate where the route allows" and
  surface when it isn't getting it, the way #301 does on macOS?
- iPad first (split view ports nearly as-is) or iPhone first (the actual reason)?
- Does the macOS app move to `xcodebuild` as its own change, ahead of any iOS work, so
  the build migration is bisectable on its own?
- App Group container now or later? A widget or Live Activity can't be retrofitted onto
  a non-shared container without a migration.
