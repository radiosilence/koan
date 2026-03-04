# Codebase Audit v0.5.2 — Progress Report

**Plan:** [10-codebase-audit-2026-03-04-v0.5.2.md](10-codebase-audit-2026-03-04-v0.5.2.md)
**Started:** 2026-03-04
**Last updated:** 2026-03-04

---

## Phase Status

| Phase | PR Branch | Status | PR |
|-------|-----------|--------|-----|
| 1 — Security & Quick Wins | `fix/security-hardening` | ✅ DONE | [#13](https://github.com/radiosilence/koan/pull/13) |
| 5 — Tests | `test/coverage-and-cleanup` | 🔄 IN PROGRESS | — |
| 2 — Performance | `perf/render-loop` | ⏳ TODO | — |
| 3 — Architecture | `refactor/module-decomposition` | ⏳ TODO | — |
| 4 — Dependencies | `chore/dependency-cleanup` | ⏳ TODO | — |

---

## Phase 1 — Security & Quick Wins ✅

**PR:** [#13](https://github.com/radiosilence/koan/pull/13) — 19 files, +64 -92 lines

| # | Item | Status |
|---|------|--------|
| 1 | S0: Remove auth creds from remote_url DB column | ✅ |
| 2 | S1: chmod 0o600 on config.local.toml and DB | ✅ |
| 3 | S3: Sanitize FTS5 search queries | ✅ |
| 4 | S4: Replace /dev/urandom with getrandom | ✅ |
| 5 | S7: HTTPS warning for non-localhost | ✅ |
| 6 | S6: Secure cover art temp path (PID-based) | ✅ |
| 7 | S9: Escape LIKE wildcards | ✅ |
| 8 | S8: DB file permissions restricted | ✅ |
| 9 | DC1-DC6: Remove dead code (6 items) | ✅ |
| 10 | U1-U2: Fix panicking unwraps | ✅ |
| 11 | D1: Scope symphonia features | ✅ |

## Phase 5 — Tests 🔄

**Branch:** `test/coverage-and-cleanup` — 3 parallel agents running

| # | Item | Status |
|---|------|--------|
| 21 | Delete 4 AI-generated duplicate tests | 🔄 |
| 22 | Add PlaybackTimeline tests | 🔄 |
| 23 | Add SharedPlayerState tests | 🔄 |
| 24 | Add favourites.rs tests | 🔄 |
| 25 | Add Subsonic deserialization tests | 🔄 |
| 26 | Add metadata_from_probe_result tests | 🔄 |

## Phase 2 — Performance ⏳

| # | Item | Status |
|---|------|--------|
| 11 | P1: playlist_version check in refresh_visible_queue | ⏳ |
| 12 | P2-P3: Cache build_display_lines, borrowed keys | ⏳ |
| 13 | P5: VizFrame spectrum Vec→array | ⏳ |

## Phase 3 — Architecture ⏳

| # | Item | Status |
|---|------|--------|
| 14 | app.rs decomposition | ⏳ |
| 15 | player/mod.rs: extract undo, dedup playback | ⏳ |
| 16 | organize.rs: dedup plan_moves | ⏳ |
| 17 | tracks.rs: extract row_to_track helper | ⏳ |

## Phase 4 — Dependencies ⏳

| # | Item | Status |
|---|------|--------|
| 18 | D2: Move rusqlite out of koan-music | ⏳ |
| 19 | D3: Audit rusqlite feature flags | ⏳ |
| 20 | D4-D6: Version alignment, workspace deps | ⏳ |
