- **ClearPlaylist snapshot race** — `stop_playback_and_clear_state()` (engine teardown only) now runs before snapshotting the playlist for undo, then clears. Previously `stop()` cleared the playlist before the undo snapshot was captured ([#78](https://github.com/radiosilence/koan/pull/78))

