- **The spectrum is published, not polled.** The playing indicator asked the engine for levels on a clock — a timer at first, then the display link — because there was no event to react to. There is one now: the analyser sends a frame when it has one, the app wakes on it, and the bars are drawn from what arrived. No timer, no tick, and no frame read twice or missed.

  What falls out of that is the idle cost. The analyser decays the bars to flat when the play head stops rather than holding the last chord, and once they are flat it publishes nothing at all — so a paused koan wakes nothing. The thread itself parks on a wait instead of standing down to a quarter-second look-again loop: with no reader and nothing playing it is not scheduled at all, until a reader arrives or playback starts. That last one cannot be signalled from the play head, which the audio render callback writes and which may never take a lock, so the player says so on the two edges where silence ends.

  `koan-core` takes `tokio` for `sync` only — `watch` is the analyser telling its subscribers a frame is ready. No runtime, no reactor.

