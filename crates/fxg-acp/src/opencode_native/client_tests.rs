//! Unit tests for the native OpenCode HTTP client.

use super::{OpenCodeError, epoch_millis_to_rfc3339, is_http_status, parse_log_watermark};

#[test]
fn classifies_http_statuses_without_string_matching() {
    let error: anyhow::Error = OpenCodeError::Status {
        path: "/api/session/ses_1".to_owned(),
        status: reqwest::StatusCode::NOT_FOUND,
        body: "not found".to_owned(),
    }
    .into();
    assert!(is_http_status(&error, 404));
    assert!(!is_http_status(&error, 409));
}

#[test]
fn formats_opencode_timestamps_as_rfc3339() {
    assert_eq!(
        epoch_millis_to_rfc3339(0),
        "1970-01-01T00:00:00.000Z".to_owned()
    );
    assert_eq!(
        epoch_millis_to_rfc3339(1_794_477_600_000),
        "2026-11-12T10:00:00.000Z".to_owned()
    );
}

#[test]
fn parses_the_log_synced_watermark() {
    assert_eq!(
        parse_log_watermark(
            "data: {\"type\":\"log.synced\",\"aggregateID\":\"ses_1\",\"seq\":12}\n\n"
        ),
        Some(12)
    );
    assert_eq!(parse_log_watermark("data: {\"type\":\"other\"}\n\n"), None);
}
