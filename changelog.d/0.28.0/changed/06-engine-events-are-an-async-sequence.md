- **Engine events are an async sequence rather than a callback interface.** `PlayerEvents` was a trait the client implemented and registered; it is now `next_event()`, awaited in a loop, and `PlayerEvent` carries what changed. The loop *is* the subscription, so its lifetime is the client's task and there is nothing to register or unregister.

  The macOS app loses the object that existed only to bridge the two worlds — a callback has to be `Sendable` while the model is main-actor isolated, so every one of its three methods did nothing but hop back with `Task { @MainActor }`. It also loses an FFI round trip per event: the change already carries the new state, and the app was re-fetching it.

  A client that falls behind now drops the events it missed instead of delaying the engine. Every variant carries an absolute value — a snapshot, a version, a position — so the one that does arrive is complete on its own.

