- **Path-keyed state follows the file.** Only `tracks.path` and `scan_cache.path` were rewritten, orphaning favourites (keyed by path), queue snapshots, saved playback state and cached paths. All of them are now rewritten in the same transaction as the move — and the database work happens first, so a `UNIQUE(path)` violation aborts before the file is touched instead of after.

