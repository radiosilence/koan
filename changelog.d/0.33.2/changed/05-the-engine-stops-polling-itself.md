- **The engine stops polling itself.** A thread woke ten times a second for as long as koan was open, rebuilt what is playing, sampled every transfer's byte count, compared three version counters and published whatever had moved. The interface was reactive — the app has read events rather than asking since v0.32 — but the events were manufactured by a clock.

  The writers say so now. Every setter on the player's shared state, the download store and the library version bump a wake; the watcher waits on it and reads the versions when it comes round, so a burst is still one pass and one message per slice. A koan with nothing happening does not schedule that thread at all.

  Transfer rates go the same way: a reading is taken as the bytes land, held to one every 250ms, rather than by whoever happened to be watching. A transfer that settles zeroes its own figure rather than waiting to be sampled again — a row that finished used to keep the rate it managed on its last chunk until something looked.

