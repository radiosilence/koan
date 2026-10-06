- **`tracks(...)` loaded the whole library and filtered it in Rust.** Every predicate ran as a
  `retain()` over every row before pagination, `search:` silently truncated at 10,000 rows, and
  `yearStart`/`yearEnd` ran `SELECT date FROM albums WHERE id = ?` once per track in the library.
  Filters, ordering and the page window are now SQL with bound parameters.
