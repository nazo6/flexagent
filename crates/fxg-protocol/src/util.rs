//! 時刻・ID 生成などの小さなユーティリティ。

use std::time::{SystemTime, UNIX_EPOCH};

/// 現在時刻を Unix epoch ミリ秒で返す。
///
/// イベントの `created_at` は「ノード側のローカル時計」で記録する
/// (複数ノード横断の並びは NTP 同期済みクロックを前提とする)。
pub fn now_ms() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_millis().min(i64::MAX as u128) as i64,
        // システム時計が UNIX epoch より前の異常時は 0 を返す (パニックさせない)
        Err(_) => 0,
    }
}

/// タイムスタンプ順序付き UUID (**UUID v7**) を文字列で生成する。
///
/// イベントID (`event_id`)・セッションID (`session_id`)・コマンドID
/// (`command_id`) に使用する。時刻順ソートと重複排除 (`event_id` UNIQUE) の
/// 両方を兼ねる。
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
