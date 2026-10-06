- **The FFI never blocks its caller.** Every one of the 80 exported calls that can block is `async` now and runs on a worker thread; the six that stay synchronous read a single atomic or an O(1) length and cannot block by construction. Previously all of them were synchronous FFI functions and the hazard lived in doc comments, which had already failed — `lyrics()` was documented as safe to call from a view body and opened a database connection, and six paths in the macOS app reached the engine directly on the main actor, one of them from inside a SwiftUI `ForEach`.

  Blocking moved into Rust rather than being wrapped at each call site. The app used to hop with `Task.detached`, which runs on Swift's cooperative pool — sized to the core count — so a cover-art storm or a scan starved every other task in the process. The engine's pool grows on demand and is deliberately not core-sized: a thread waiting on a socket costs a kernel stack, not CPU, and capping blocking work at the core count queues it behind nothing. koan-core stays synchronous, which is what its three synchronous consumers and its dedicated audio threads want; only the boundary changed.

  Queue mutations and transport commands share one lane, in submission order. They previously each got their own `Task.detached`, so two in quick succession could reach the player in either order — dropping in an album and pressing undo could undo it before it landed.

  Every `Task.detached` around an engine call is gone from the app, along with the helper that wrapped them.

