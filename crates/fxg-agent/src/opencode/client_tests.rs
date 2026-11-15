//! Unit tests for the native OpenCode HTTP client.

use fxg_protocol::common::UsageWindowKind;

use super::{
    OpenCodeError, epoch_millis_to_rfc3339, is_http_status, parse_go_usage, parse_local_usage,
    parse_log_watermark,
};

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

#[test]
fn parses_the_local_go_usage_meter() {
    let value = serde_json::json!({
        "location": { "directory": "/tmp/project" },
        "data": {
            "go": {
                "rolling": { "percent": 1.0, "limited": false, "resetsAt": 1_794_751_636_000_i64 },
                "weekly": { "percent": 69.0, "limited": false, "resetsAt": 1_794_787_200_000_i64 },
                "monthly": { "percent": 85.0, "limited": true, "resetsAt": 1_794_839_952_000_i64 }
            }
        }
    });

    let usage = parse_local_usage(&value).expect("local Go usage");
    assert_eq!(usage.source.id, "opencode-go");
    assert_eq!(usage.source.label, "OpenCode Go");
    assert_eq!(usage.groups.len(), 1);
    let windows = &usage.groups[0].windows;
    assert_eq!(windows.len(), 3);
    assert_eq!(windows[0].kind, UsageWindowKind::Rolling5h);
    assert_eq!(windows[1].kind, UsageWindowKind::Weekly);
    assert_eq!(windows[2].kind, UsageWindowKind::Monthly);
    assert!((windows[0].used_fraction - 0.01).abs() < 1e-9);
    assert_eq!(windows[0].resets_at, Some(1_794_751_636_000));
    assert!(!windows[0].limited);
    assert!(windows[2].limited);
}

#[test]
fn parses_the_direct_go_usage_meter() {
    let value = serde_json::json!({
        "usage": {
            "rolling": { "status": "ok", "percent": 1.0, "resetsAt": "2026-11-15T15:47:16.000Z" },
            "weekly": { "status": "ok", "percent": 69.0, "resetsAt": "2026-11-16T00:00:00.000Z" },
            "monthly": { "status": "rate-limited", "percent": 85.0, "resetsAt": "2026-11-16T14:39:12.000Z" }
        }
    });

    let usage = parse_go_usage(&value).expect("direct Go usage");
    assert_eq!(usage.source.id, "opencode-go");
    let windows = &usage.groups[0].windows;
    assert_eq!(windows[0].kind, UsageWindowKind::Rolling5h);
    assert!((windows[0].used_fraction - 0.01).abs() < 1e-9);
    assert!(windows[0].resets_at.is_some());
    assert!(!windows[0].limited);
    assert!(windows[2].limited);
}

#[test]
fn rejects_incomplete_usage_payloads() {
    let value = serde_json::json!({
        "usage": {
            "rolling": { "status": "ok", "percent": 1.0, "resetsAt": "2026-11-15T15:47:16.000Z" }
        }
    });
    assert!(parse_go_usage(&value).is_none());
    assert!(parse_local_usage(&serde_json::json!({ "data": null })).is_none());
}
