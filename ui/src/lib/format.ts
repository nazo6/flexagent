/**
 * UI 表示用のフォーマットヘルパー。
 *
 * 時刻表示は `SessionSummary.updated_at` / `AuditLogEntry.created_at` /
 * `system_status.last_synced_at` など epoch ms で統一されているため、
 * ここに集約する。
 */

/** epoch ms をローカル時刻 (`YYYY-MM-DD HH:mm:ss`) に整形する。 */
export function formatEpochMs(ms: number | null | undefined): string {
  if (ms == null) return "-";
  const date = new Date(ms);
  if (Number.isNaN(date.getTime())) return "-";
  return (
    `${date.getFullYear()}-${pad2(date.getMonth() + 1)}-${pad2(date.getDate())} ` +
    `${pad2(date.getHours())}:${pad2(date.getMinutes())}:${pad2(date.getSeconds())}`
  );
}

function pad2(value: number): string {
  return String(value).padStart(2, "0");
}

/** epoch ms を「たった今 / N分前 / N時間前 / N日前」の相対表現に整形する。 */
export function formatRelativeTime(
  ms: number | null | undefined,
  now: number = Date.now(),
): string {
  if (ms == null) return "-";
  const diffSec = Math.floor((now - ms) / 1000);
  if (diffSec < 5) return "たった今";
  if (diffSec < 60) return `${diffSec}秒前`;
  const diffMin = Math.floor(diffSec / 60);
  if (diffMin < 60) return `${diffMin}分前`;
  const diffHour = Math.floor(diffMin / 60);
  if (diffHour < 24) return `${diffHour}時間前`;
  const diffDay = Math.floor(diffHour / 24);
  return `${diffDay}日前`;
}

/** epoch ms を「now / 5m / 2h / 3d」などのコンパクト表現に整形する。 */
export function formatRelativeTimeCompact(
  ms: number | null | undefined,
  now: number = Date.now(),
): string {
  if (ms == null) return "-";
  const diffSec = Math.floor((now - ms) / 1000);
  if (diffSec < 10) return "now";
  if (diffSec < 60) return `${diffSec}s`;
  const diffMin = Math.floor(diffSec / 60);
  if (diffMin < 60) return `${diffMin}m`;
  const diffHour = Math.floor(diffMin / 60);
  if (diffHour < 24) return `${diffHour}h`;
  const diffDay = Math.floor(diffHour / 24);
  return `${diffDay}d`;
}

/** ミリ秒を「1.2s / 1m 4s」の所要時間表現に整形する。 */
export function formatDurationMs(ms: number): string {
  if (!Number.isFinite(ms) || ms < 0) return "-";
  if (ms < 1000) return `${Math.round(ms)}ms`;
  const seconds = ms / 1000;
  if (seconds < 60) return `${seconds.toFixed(1)}s`;
  const minutes = Math.floor(seconds / 60);
  const restSec = Math.round(seconds % 60);
  if (minutes < 60) return `${minutes}m ${restSec}s`;
  const hours = Math.floor(minutes / 60);
  return `${hours}h ${minutes % 60}m`;
}
