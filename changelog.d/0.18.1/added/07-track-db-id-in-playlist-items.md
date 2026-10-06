- **Track `db_id` in playlist items** — `PlaylistItem` and `PersistedQueueItem` now carry `db_id: Option<i64>`, enabling re-download of remote tracks after session restore. Backwards-compatible: old persisted state without `db_id` deserializes cleanly via `#[serde(default)]` ([#94](https://github.com/radiosilence/koan/issues/94))
### Changed

