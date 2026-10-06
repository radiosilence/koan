- **`ReplacePlaylist { items, start }`** — replace the queue and choose where it starts, as one command. Clear-then-add-then-play is three commands down a bounded channel, each acted on as it arrives, so playing track nine of an album audibly started track one first. `replaceQueue` and the FFI both take the index.

