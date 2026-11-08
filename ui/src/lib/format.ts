/**
 * Formatting helpers for UI display.
 *
 * Timestamps such as `SessionSummary.updated_at`, `AuditLogEntry.created_at` and
 * `system_status.last_synced_at` are all epoch milliseconds, so formatting is
 * centralized here.
 */

import { m } from "#lib/paraglide/messages.js";

/** Format epoch ms as a local date-time (`YYYY-MM-DD HH:mm:ss`). */
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

/** Format epoch ms as a relative expression (`just now / N min ago / N hr ago / N d ago`). */
export function formatRelativeTime(
  ms: number | null | undefined,
  now: number = Date.now(),
): string {
  if (ms == null) return "-";
  const diffSec = Math.floor((now - ms) / 1000);
  if (diffSec < 5) return m.lib_relative_just_now();
  if (diffSec < 60) return m.lib_relative_seconds({ count: diffSec });
  const diffMin = Math.floor(diffSec / 60);
  if (diffMin < 60) return m.lib_relative_minutes({ count: diffMin });
  const diffHour = Math.floor(diffMin / 60);
  if (diffHour < 24) return m.lib_relative_hours({ count: diffHour });
  const diffDay = Math.floor(diffHour / 24);
  return m.lib_relative_days({ count: diffDay });
}

/** Format epoch ms as a compact expression (`now / 5m / 2h / 3d`). */
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

/** Format milliseconds as a duration expression (`1.2s / 1m 4s`). */
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

/** Format a token count as a compact expression (`999 / 12.3k / 1.2M`). */
export function formatTokenCount(tokens: number): string {
  if (!Number.isFinite(tokens) || tokens < 0) return "-";
  if (tokens < 1000) return String(Math.round(tokens));
  if (tokens < 1_000_000) return `${(tokens / 1000).toFixed(1)}k`;
  return `${(tokens / 1_000_000).toFixed(1)}M`;
}
