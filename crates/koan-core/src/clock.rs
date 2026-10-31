//! The wall clock, as the database and the wire store it. A clock set before
//! 1970 reads as the epoch.

use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since the unix epoch.
pub fn unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Milliseconds since the unix epoch.
pub fn unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}
