# Incremental Sync + Resilient Local/Remote Fallback

## Context

Two problems:
1. `remove_stale_tracks()` deletes DB rows when local files are missing — including merged rows that have remote_id/remote_url. If someone scans with an external drive unplugged, the whole library gets nuked from the DB, losing remote fallback info.
2. `koan remote sync` re-fetches the entire Navidrome library every time. The `remote_servers.last_sync` column exists but is dead code. Subsonic API supports `getAlbumList2(type=newest)` which we're not using.

## Changes

### 1. Don't nuke remote-backed rows on scan (critical fix)

**File:** `crates/koan-core/src/db/queries/tracks.rs` — `remove_stale_tracks()`

- If a stale track has `remote_id IS NOT NULL`: **UPDATE** instead of DELETE — set `path = NULL`, `source = 'remote'`, clear `mtime`/`size_bytes`, delete its `scan_cache` entry
- If pure local (no remote_id): DELETE as before
- Drop the `source = 'local'` filter, use `path IS NOT NULL` instead (catches all tracks with local paths regardless of source flag)
- When drive comes back, `upsert_track` content match (strategy 3) re-merges the path automatically — no extra code needed

### 2. Incremental remote sync

**File:** `crates/koan-core/src/remote/client.rs`
- Add `created: Option<String>` to `SubsonicAlbum` and `SubsonicAlbumFull` structs (standard Subsonic field, ISO 8601)

**File:** `crates/koan-core/src/remote/sync.rs`
- Add `full: bool` param to `sync_library()`
- If `full=false` and `last_sync` exists: use `getAlbumList2(type=newest)` instead of `alphabeticalByName`, stop paginating when oldest album in page has `created` < `last_sync`
- If `full=false` but no `last_sync` (first sync ever): fall through to full sync
- After successful sync: write current timestamp to `remote_servers.last_sync`
- Add helper fns: `get_last_sync()`, `update_last_sync()`, ensure `remote_servers` row exists via upsert
- Parse ISO 8601 timestamps with `chrono::DateTime::parse_from_rfc3339` (already in dep tree)

### 3. CLI wiring

**File:** `crates/koan-music/src/main.rs`
- Add `--full` flag to `RemoteCommands::Sync`

**File:** `crates/koan-music/src/commands/remote.rs`
- Pass `full` bool through to `sync_library()`
- Update `sync_library` call signature (needs server url + username now)

### 4. Docs + changelog

- README: mention incremental sync, `--full` flag
- ARCHITECTURE.md: update remote sync description
- CHANGELOG: add to Unreleased section

## Tests

In `tracks.rs`:
- `test_remove_stale_preserves_remote_backed` — merged row survives, path nulled, source flipped to "remote", resolve falls through to remote stream
- `test_remove_stale_deletes_pure_local` — pure local row still gets deleted
- `test_reattach_on_rescan` — after stale removal, re-scanning same file re-merges path back via content match

## Notes

- `UNIQUE(path)` constraint: SQLite allows multiple NULLs in UNIQUE columns, no schema change needed
- `chrono` is already transitive dep, no new deps
- `type=newest` is standard Subsonic API (Navidrome, Airsonic, etc), not an extension
- Graceful degradation: if server doesn't return `created`, incremental just processes all pages (equivalent to full sync)
- No `--skip-remove` flag needed — the fix is just "don't remove what has a remote backup"

## Verification

1. `cargo clippy --all-targets -- -D warnings`
2. `cargo test --all-targets`
3. Manual: scan with a folder that has tracks also in Navidrome → remove some local files → rescan → verify tracks demoted to remote-only → verify playback falls through to streaming → re-add files → rescan → verify path restored
