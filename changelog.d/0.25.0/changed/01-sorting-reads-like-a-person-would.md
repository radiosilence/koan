- **Sorting reads like a person would.** SQLite's default collation is a byte comparison, so the artist list ran `Zebra` before `aphex twin` and put `Âme` after the entire alphabet. A `LIBRARY` collation folds case and accents onto the base letter and compares digit runs as numbers, so `Track 2` precedes `Track 10`.

