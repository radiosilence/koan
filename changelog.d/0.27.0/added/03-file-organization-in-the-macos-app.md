- **File organization in the macOS app.** Dropping a folder of files on the queue indexes them into the library where they lie and queues them; **Organize Files…** in the queue or library context menu then opens a sheet that previews where a pattern puts each one, and moves them when you say so.

  The preview is the feature. Every selected file gets a row showing its destination relative to the library folder, *including the ones that can't move* — a destination already occupied, or two files resolving to the same path, is an orange row next to the path it collided with rather than a number in an error count underneath. Nothing is ever overwritten, so the only way that matters is if you can see it before pressing the button.

  Patterns come from `[organize.patterns]`, shared with the CLI and TUI, and can be edited in place: the preview follows what you type, and saving writes it back to `config.toml` under its name. An edited pattern previews and runs without being saved, so trying one out costs nothing. With more than one library folder configured you pick which one to organize into; the CLI's behaviour (the first) is the default.

  Dropped files get library rows *where they are*, not in the music tree — importing and organizing are separate on purpose, so files land in the library only after you have seen where they are going.

  It opens in a window rather than a sheet. A sheet cannot be resized — AppKit leaves the style mask off and SwiftUI pins its content size — and a table of file paths is exactly the thing you want to make wider. A window also leaves the library visible behind it, which suits a preview you are checking against rather than a prompt you are answering.

  Whether cover art and cue sheets travel with the music is a checkbox in the sheet and a `[organize] move_ancillary` setting behind it, so the CLI and TUI organize the way the app just did.

  Generating destinations is separated from asking the disk about them, because only one of those is fast. `organize::generate` formats a pattern against an already-resolved selection and touches no files at all, so it reruns on every keystroke; `organize::check_against_disk` is the `stat`-per-file pass that finds occupied destinations and the artwork travelling alongside, and it lands a moment later. Ancillary files were previously discovered with a directory read *per file*, so an album of a dozen tracks did the same `readdir` a dozen times.
