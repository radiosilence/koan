- **`triggerScan` and `triggerRemoteSync` return a `Job`, not a result.** Both run for minutes; they
  now start a detached worker and hand back `{ id, kind, state, message }`, polled with the new
  `job(id:)` and `jobs` queries. One job of each kind runs at a time — a second call returns the
  running one. The old `ScanResult` type is gone; added/updated/unchanged counts arrive in the
  finished job's `message`.
