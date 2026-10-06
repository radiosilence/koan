- **ratatui 0.29 → 0.30, crossterm 0.28 → 0.29** — the two move together because `ratatui-crossterm` defaults to crossterm 0.29; bumping ratatui alone resolves two crossterm versions and breaks at the backend boundary. No source changes: koan's ratatui surface is `Buffer`, `Rect`, `Style`, `Line`/`Span`, `Widget` and `Length`/`Min`/`Percentage` constraints, and every 0.30 breaking change lands elsewhere.

  0.30 splits into `ratatui-core`/`ratatui-widgets`/`ratatui-crossterm` and replaces the cassowary layout solver with kasuari. Solver output is unchanged for koan's constraint sets, and `Buffer`'s out-of-bounds policy still panics rather than clamping, so nothing that used to render now renders differently or silently truncates. The one behavioural change is that halfwidth katakana dakuten/handakuten (`U+FF9E`/`U+FF9F`) now measure one cell instead of zero, matching how terminals actually draw them.

  The optional `termwiz`/`termina` backends appear in `Cargo.lock` but stay out of the build graph.

