- **A native macOS app** (`apps/macos`) — SwiftUI on top of koan-core through new uniffi bindings (`koan-ffi`), not a client of the GraphQL server. The app links the engine in-process, so there is no daemon, no port and no auth surface between the UI and the audio it is driving. GraphQL remains the surface for clients that genuinely cannot link the core.

  Queue, albums, artists, favourites and snapshots, with album-grouped queue editing, drag and drop, a ⌘K picker, synced lyrics, global search, media keys and Now Playing, output device switching, cover art with a disk cache, and session restore that picks up mid-track — resuming only if playback was running when you quit.

