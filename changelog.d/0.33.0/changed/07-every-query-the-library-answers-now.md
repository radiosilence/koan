- **Every query the library answers now goes through an index.** `EXPLAIN QUERY PLAN` was run over every statement koan issues, against a 47,944-track library, and each full table read was either fixed or established as a query that genuinely means all of them. The costs were invisible because nothing ever failed — they only grow with the size of somebody's library:

  - Listing an artist's tracks read the whole library. A track counts as theirs by its own credit or its album's, and SQLite cannot use an index for an `OR` spanning two tables; asking `albums` in a subquery instead makes both halves indexed lookups. 10 ms → 2 ms for one artist, and the same shape in the API's batched artist load and in radio's artist picker.

  - Listing favourites read the whole library, in the one place koan#363 did not reach — a track is favourited by any of its three paths, and joining on an `OR` across the three cannot use an index. Written as a union, as the id lookup next to it already was: 27 ms → 0.7 ms.

  - A remote sync read every album and every artist once per record it enriched, matching on the server's id, which was not indexed. That is quadratic in the size of the library: 1.6 s of pure scanning for 5,510 albums, 44 ms once indexed. Starring albums and artists from the server was the same lookup and got the same fix.

  - Radio resolves the artists a recommender names back to local rows, by MusicBrainz id and then by name. Neither could use an index — nothing indexed `mbid`, and `UNIQUE(name)` is case-sensitive while the lookup is not — so every candidate read all 7,138 artists.

  - Forgetting a folder, and the stale-file check every scan runs, matched paths with `LIKE 'folder/%'`, which no index can serve. Now a range over the indexed path column. It also takes the pattern out of the path: `LIKE` reads `_` as "any character" and folds ASCII case, so a folder named `My_Music` matched `My Music` too.

  - Deleting a track's scan-cache entry, and reading one back during organize, searched by track id where the key is the path. Undo read the whole organize log to find one batch.

  - The download cache's eviction pass read all 47,944 tracks to find the ten that were downloaded, despite an index on exactly that. koan never ran `ANALYZE`, so the planner had no statistics and picked an index that merely supplied the `ORDER BY`. It now runs `PRAGMA optimize` on open and after a scan or a sync: 5.3 ms → 0.1 ms, with no query change at all.

  One index went the other way: `refresh_tokens(expires_at)` was read by nothing and cost every sign-in, so it is dropped.
  
