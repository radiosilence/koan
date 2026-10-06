- **A fresh SQLite connection was opened per resolver field.** `Database::open` creates the parent
  directory, chmods the file, sets four pragmas, attempts a WAL checkpoint and runs a ~30-statement
  DDL batch plus three migrations — all of it, every field. On a 500-artist library the nested
  artists → albums → tracks query cost roughly 3,500 open cycles, about 120,000 statements. The
  schema now holds a small connection pool sized to the core count, `Database::open_existing` skips
  the setup for pooled connections, and the DDL runs once.
