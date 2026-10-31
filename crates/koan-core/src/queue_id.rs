use std::fmt;

use uuid::Uuid;

/// Stable identity for a queue entry. UUIDv7 — time-ordered, unique across duplicates.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct QueueItemId(pub Uuid);

impl QueueItemId {
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for QueueItemId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for QueueItemId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The tail: a v7's leading hex is its timestamp, shared by a whole batch.
        let hex = self.0.simple().to_string();
        write!(f, "QId({})", &hex[hex.len() - 8..])
    }
}
