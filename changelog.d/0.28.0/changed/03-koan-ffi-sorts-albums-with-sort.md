- **`koan-ffi` sorts albums with `sort_by_cached_key`.** `sort_by_key` recomputes its key on every comparison, so sorting by title lowercased each one a couple of dozen times over.

