- **Remote tracks now play when queued via GQL/MCP** — two-part fix:
  1. GQL mutations now trigger background downloads for remote tracks (0.12.4)
  2. Remote tracks now get the correct cache path via `resolve_item_path()` — same code path as the TUI
