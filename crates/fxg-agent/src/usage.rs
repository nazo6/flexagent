//! Shared helpers for agent usage limits.

use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// Parses an RFC 3339 timestamp into Unix epoch milliseconds.
#[allow(
    clippy::manual_ok_err,
    reason = "Result::ok is disallowed by the workspace lint"
)]
pub(crate) fn rfc3339_to_epoch_ms(value: &str) -> Option<i64> {
    let parsed = match OffsetDateTime::parse(value, &Rfc3339) {
        Ok(parsed) => parsed,
        Err(_) => return None,
    };
    match i64::try_from(parsed.unix_timestamp_nanos() / 1_000_000) {
        Ok(ms) => Some(ms),
        Err(_) => None,
    }
}

/// Converts a remaining fraction into a clamped used fraction.
pub(crate) fn used_fraction_from_remaining(remaining: f64) -> f64 {
    (1.0 - remaining).clamp(0.0, 1.0)
}

/// Converts a used percentage (`0..=100`) into a clamped fraction.
pub(crate) fn used_fraction_from_percent(percent: f64) -> f64 {
    (percent / 100.0).clamp(0.0, 1.0)
}
