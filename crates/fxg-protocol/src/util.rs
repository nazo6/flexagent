//! Small utilities such as time and ID generation.

use std::collections::VecDeque;
use std::time::{SystemTime, UNIX_EPOCH};

/// Default capacity of the per-terminal output log (bytes).
pub const PTY_OUTPUT_LOG_CAPACITY: usize = 256 * 1024;

/// Bounded byte log of terminal output used for attach-time replay.
///
/// Every pushed chunk gets a monotonically increasing sequence number. A
/// [`snapshot`](Self::snapshot) returns the retained bytes together with the
/// sequence number of the next chunk, so a subscriber can drop live chunks that
/// the snapshot already covers.
///
/// The log keeps only the tail; a truncated snapshot may start in the middle of
/// an ANSI escape sequence, which terminals tolerate.
#[derive(Debug)]
pub struct PtyOutputLog {
    capacity: usize,
    data: VecDeque<u8>,
    next_seq: u64,
}

impl Default for PtyOutputLog {
    fn default() -> Self {
        Self::with_default_capacity()
    }
}

impl PtyOutputLog {
    /// Creates a log with an explicit capacity (at least 1 byte).
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            data: VecDeque::new(),
            next_seq: 0,
        }
    }

    /// Creates a log with [`PTY_OUTPUT_LOG_CAPACITY`].
    pub fn with_default_capacity() -> Self {
        Self::new(PTY_OUTPUT_LOG_CAPACITY)
    }

    /// Appends a chunk and returns its sequence number.
    pub fn push(&mut self, data: &[u8]) -> u64 {
        let seq = self.next_seq;
        self.next_seq = self.next_seq.saturating_add(1);
        self.data.extend(data.iter().copied());
        let excess = self.data.len().saturating_sub(self.capacity);
        if excess > 0 {
            self.data.drain(..excess);
        }
        seq
    }

    /// Returns the retained bytes and the sequence number of the next chunk.
    pub fn snapshot(&self) -> (Vec<u8>, u64) {
        (self.data.iter().copied().collect(), self.next_seq)
    }

    /// Sequence number the next pushed chunk will receive.
    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    /// Number of retained bytes.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Whether nothing is retained.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Capacity in bytes.
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

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

/// Discards a non-critical `Result` with an explicit reason.
///
/// This is the sanctioned way to ignore a `Result`: the reason stays visible in
/// the code and the error is still recorded at debug level. Use a `warn!` /
/// propagated `Err` instead when the failure changes user-visible behavior.
///
/// ```ignore
/// fxg_protocol::discard_with!(tx.send(event), "notify pty exit");
/// ```
#[macro_export]
macro_rules! discard_with {
    ($result:expr, $context:expr $(,)?) => {{
        if let Err(err) = $result {
            ::tracing::debug!(error = ?err, context = $context, "discarded error");
        }
    }};
}

/// Spawns a background task and reports both `Err` and panic (`JoinError`).
///
/// The inner future must resolve to `Result<(), E>` where `E: Display`.
///
/// ```ignore
/// fxg_protocol::spawn_logged!("outbox sync", run(state, url, token, shutdown));
/// ```
#[macro_export]
macro_rules! spawn_logged {
    ($name:expr, $future:expr $(,)?) => {{
        let future = $future;
        ::tokio::spawn(async move {
            match ::tokio::spawn(future).await {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    ::tracing::error!(task = $name, "background task failed: {err}");
                }
                Err(join) => {
                    ::tracing::error!(
                        task = $name,
                        panic = join.is_panic(),
                        "background task terminated unexpectedly: {join}"
                    );
                }
            }
        })
    }};
}

/// Spawns a fire-and-forget background task and reports a panic (`JoinError`).
///
/// Use this for tasks whose future returns `()`. When the future returns
/// `Result<(), E>`, use [`spawn_logged!`](crate::spawn_logged) instead so the
/// error is logged as well.
///
/// ```ignore
/// fxg_protocol::spawn_guarded!("tray event loop", run_tray(rx));
/// ```
#[macro_export]
macro_rules! spawn_guarded {
    ($name:expr, $future:expr $(,)?) => {{
        let future = $future;
        ::tokio::spawn(async move {
            if let Err(join) = ::tokio::spawn(future).await {
                ::tracing::error!(
                    task = $name,
                    panic = join.is_panic(),
                    "background task terminated unexpectedly: {join}"
                );
            }
        })
    }};
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

    #[test]
    fn pty_output_log_assigns_monotonic_sequence_numbers() {
        let mut log = PtyOutputLog::new(64);
        assert_eq!(log.push(b"a"), 0);
        assert_eq!(log.push(b"b"), 1);
        let (bytes, next) = log.snapshot();
        assert_eq!(bytes, b"ab");
        assert_eq!(next, 2);
    }

    #[test]
    fn pty_output_log_keeps_only_the_tail() {
        let mut log = PtyOutputLog::new(4);
        log.push(b"hello");
        log.push(b"!");
        let (bytes, _) = log.snapshot();
        assert_eq!(bytes, b"llo!");
        assert_eq!(log.len(), 4);
    }

    #[test]
    fn pty_output_log_handles_chunks_larger_than_the_capacity() {
        let mut log = PtyOutputLog::new(3);
        log.push(b"0123456789");
        let (bytes, next) = log.snapshot();
        assert_eq!(bytes, b"789");
        assert_eq!(next, 1);
    }
}
