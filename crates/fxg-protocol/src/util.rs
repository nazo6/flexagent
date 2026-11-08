//! Small utilities such as time and ID generation.

use std::time::{SystemTime, UNIX_EPOCH};

/// Returns the current time as Unix epoch milliseconds.
///
/// An event's `created_at` is recorded with the node's local clock (ordering
/// across nodes assumes NTP-synchronized clocks).
pub fn now_ms() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_millis().min(i64::MAX as u128) as i64,
        // When the system clock is before the Unix epoch, return 0 instead of
        // panicking.
        Err(_) => 0,
    }
}

/// Generates a time-ordered UUID (**UUID v7**) as a string.
///
/// Used for event IDs (`event_id`), session IDs (`session_id`) and command IDs
/// (`command_id`). It serves both as a chronological sort key and as a
/// deduplication key (`event_id` UNIQUE).
pub fn uuid_v7() -> String {
    uuid::Uuid::now_v7().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_ms_is_positive_and_monotonic_enough() {
        let a = now_ms();
        assert!(a > 1_700_000_000_000, "unix ms should be plausible: {a}");
        let b = now_ms();
        assert!(b >= a);
    }

    #[test]
    fn uuid_v7_is_parseable_and_version_7() {
        let id = uuid_v7();
        let parsed = uuid::Uuid::parse_str(&id).expect("valid uuid");
        assert_eq!(parsed.get_version_num(), 7);
        assert_ne!(id, uuid_v7(), "ids must be unique");
    }

    #[test]
    fn uuid_v7_sorts_by_generation_time() {
        let a = uuid_v7();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = uuid_v7();
        assert!(a < b, "UUID v7 should be lexicographically ordered");
    }
}
