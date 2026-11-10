//! In-memory delivery counters exposed by diagnostics.
//!
//! Counters are process-local and reset on restart. They make packet / event
//! drops visible in `/api/v1/diagnostics` and the support bundle instead of
//! leaving them in log lines only.

use std::sync::atomic::{AtomicU64, Ordering};

use fxg_protocol::common::DropCounters;

/// Counter kind (kept as an enum so call sites do not use strings).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CounterKind {
    /// Events dropped because a subscriber lagged
    DroppedEvents,
    /// Broadcast receivers that observed `Lagged`
    LaggedBroadcasts,
    /// Client WS replays that failed
    ReplayFailures,
    /// Outbox connection / send retries
    OutboxRetries,
    /// Web Push deliveries that failed
    PushFailures,
}

/// Thread-safe counters shared by the daemon / server state.
#[derive(Debug, Default)]
pub struct DiagnosticsCounters {
    dropped_events: AtomicU64,
    lagged_broadcasts: AtomicU64,
    replay_failures: AtomicU64,
    outbox_retries: AtomicU64,
    push_failures: AtomicU64,
}

impl DiagnosticsCounters {
    /// Creates zeroed counters.
    pub fn new() -> Self {
        Self::default()
    }

    /// Increments one counter by one.
    pub fn incr(&self, kind: CounterKind) {
        self.add(kind, 1);
    }

    /// Adds `value` to one counter.
    pub fn add(&self, kind: CounterKind, value: u64) {
        self.field(kind).fetch_add(value, Ordering::Relaxed);
    }

    /// Returns a serializable snapshot.
    pub fn snapshot(&self) -> DropCounters {
        DropCounters {
            dropped_events: self.dropped_events.load(Ordering::Relaxed),
            lagged_broadcasts: self.lagged_broadcasts.load(Ordering::Relaxed),
            replay_failures: self.replay_failures.load(Ordering::Relaxed),
            outbox_retries: self.outbox_retries.load(Ordering::Relaxed),
            push_failures: self.push_failures.load(Ordering::Relaxed),
        }
    }

    fn field(&self, kind: CounterKind) -> &AtomicU64 {
        match kind {
            CounterKind::DroppedEvents => &self.dropped_events,
            CounterKind::LaggedBroadcasts => &self.lagged_broadcasts,
            CounterKind::ReplayFailures => &self.replay_failures,
            CounterKind::OutboxRetries => &self.outbox_retries,
            CounterKind::PushFailures => &self.push_failures,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_reflects_increments() {
        let counters = DiagnosticsCounters::new();
        counters.incr(CounterKind::ReplayFailures);
        counters.add(CounterKind::DroppedEvents, 3);
        let snapshot = counters.snapshot();
        assert_eq!(snapshot.replay_failures, 1);
        assert_eq!(snapshot.dropped_events, 3);
        assert_eq!(snapshot.push_failures, 0);
    }
}
