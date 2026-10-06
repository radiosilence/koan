- **The macOS app takes the TUI's single-key shortcuts.** `space`, `<`/`>`, `,`/`.`, `f`, `R`, `p`, `/`, `l`/`a`, `r`, `g`/`G`, `L`, `z` and `?` do there what they do in the TUI, and Help ▸ Keyboard Shortcuts (`?`) lists them — generated from the table that implements them, so the two cannot drift.

  They are handled by a local event monitor rather than declared as menu shortcuts. A modifier-less key equivalent is claimed by the menu bar before the responder chain sees it, which would mean `f` favouriting a track instead of typing an f into the search field. The monitor asks what has focus first: the keys are live in the app and dead in any text field, sheet or the settings window. The cost is type-select in lists, which koan's browsers have a filter field for.

  `Esc` now leaves a search or filter field rather than only clearing it, and `⌘F` focuses the filter on the albums and artists pages, the library search everywhere else.

