- **`play_history.track_id` was missing its `ON DELETE CASCADE`.** Under `foreign_keys = ON` a bare `REFERENCES` makes a track with history undeletable unless the caller clears the history first. One caller did; the constraint should not depend on the next one remembering. The table is rebuilt on first open, dropping entries whose track had already gone.

