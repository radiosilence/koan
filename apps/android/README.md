# kōan for Android

A Compose app over the same engine as the Apple apps: `libkoan_ffi.so`, loaded through uniffi's Kotlin bindings (package `cc.blit.koan.ffi`). Gradle builds the core with cargo-ndk and generates the bindings before every build; neither is checked in. See ARCHITECTURE.md's Android section for the audio path.

```bash
just android-run              # build, install on the device adb sees, open
just android-ffi arm64-v8a    # only the core and the bindings
```

Gradle finds the SDK through `ANDROID_HOME` or `local.properties`; mise provides the JDK and cargo-ndk.

## Sideloading

`just android-apk` builds the release APK, minified and signed with this machine's debug key, and prints its path. Install it with `adb install -r <path>`, or copy it to the phone and open it there, allowing the file manager or browser to install apps when Android asks.

CI builds the same APK on every pull request and push to main that touches the app or the core, and attaches it to the run as `koan-android-<commit>`; download the artifact from the run's page and unzip it. CI signs with the key in the `ANDROID_DEBUG_KEYSTORE` secret when it is set, so each build installs over the last. Android refuses an update signed with a different key, which is what a local build over a CI one is; uninstall first, which signs the app out and deletes its downloads.

The APK carries arm64-v8a only: phones, and emulators on Apple silicon.

## Layout

Under `app/src/main/kotlin/cc/blit/koan/`:

| Package | Owns |
|---|---|
| `KoanApp`, `MainActivity` | The process: config directory, the one engine, Coil's loader, the playback service connection |
| `engine/` | `EngineMirror`, the engine's state slices as `StateFlow`s; `PlayerModel`, the commands |
| `playback/` | The Media3 session, background playback, audio focus, routes |
| `art/` | Covers for Coil: `AsyncImage(model = Art.album(id), …)` |
| `ui/` | `AppShell` (sign-in, tabs, mini player, Now Playing over the tabs) and small shared pieces |
| `ui/navigation/` | `Tab`, `Navigator` and `LocalNavigator` |
| `ui/theme/` | The kōan theme (`docs/design/koan-theme.md`): tokens, type, the record accent, the roles pages draw with, `KoanIcon`, and `ThemeSheet`. The glyphs are compiled from the Swift apps' SVGs by `scripts/icons.py`, and the launcher and notification icons drawn from the ensō by `scripts/launcher.py` |
| `ui/queue/`, `ui/library/`, `ui/search/`, `ui/settings/` | The four tabs |
| `ui/playlists/`, `ui/downloads/` | Pages the library opens |
| `ui/nowplaying/` | Now Playing and the mini player |

Each `ui/` package owns its routes, pages and view models, and exposes one `EntryProviderScope<NavKey>.…Entries()` function that `AppShell` calls; nothing else in the shell needs to change when a package grows.

State comes from `koanApp().mirror`, collected with `collectAsStateWithLifecycle()`. A listing reloads when `mirror.library` moves (see `LibraryModel`). The playhead is an anchor rather than a ticking value: use `rememberPosition(playhead)` or `Playhead.positionAt()`. Commands go through `koanApp().player`, or `mirror.launch { … }` for engine calls `PlayerModel` lacks.

## Adding a page

1. In the package that owns it, declare a key: `@Serializable data class FooRoute(val id: Long, val nonce: String = nonce()) : NavKey`. The `nonce` lets the same page be open in two tabs.
2. Register it in that package's entries function: `entry<FooRoute> { key -> FooPage(key.id) }`.
3. Open it from anywhere with `LocalNavigator.current.push(FooRoute(id))`. A view model inside the entry is scoped to it: `viewModel { FooModel(koanApp().mirror, id) }`.

A new tab is a `Tab` object, its place in `Tab.all`, a label and an icon in `AppShell`, and an `entry<Tab.Foo>` in its package.

## Links and shared files

`koan.rocks/join/`, `koan.rocks/pair/` and `koan://join` open the app (`ui/settings/Links.kt`), as they open the iPhone app. The https filters are App Links with `autoVerify`; until `koan.rocks/.well-known/assetlinks.json` lists the release signing key's fingerprint, Android does not hand those links to the app, and only `koan://join` reaches it.

EQ and convolution files (`.txt`, `.wav`, `.zip`) shared to the app or opened with it are copied into the cache and go through the same import as Settings → EQ's file picker (`DspModel`), questions included. Profiles apply to the phone's own output, keyed as the core keys it; nothing is applied until one is chosen.
