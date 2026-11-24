//! Shared time conversion helpers.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Converts a duration to milliseconds, saturating at `u64::MAX`.
#[must_use]
pub fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Returns the current Unix timestamp in milliseconds.
#[must_use]
pub fn unix_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, duration_ms)
}
