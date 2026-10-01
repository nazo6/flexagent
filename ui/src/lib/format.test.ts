import { describe, expect, it } from "vitest";
import {
  formatDurationMs,
  formatEpochMs,
  formatRelativeTime,
  formatRelativeTimeCompact,
} from "./format";

describe("formatEpochMs", () => {
  it("formats an epoch to local date-time", () => {
    const ms = new Date(2026, 9, 1, 5, 3, 9).getTime();
    expect(formatEpochMs(ms)).toBe("2026-10-01 05:03:09");
  });

  it("returns a placeholder for null / invalid values", () => {
    expect(formatEpochMs(null)).toBe("-");
    expect(formatEpochMs(undefined)).toBe("-");
    expect(formatEpochMs(Number.NaN)).toBe("-");
  });
});

describe("formatRelativeTime", () => {
  const now = new Date(2026, 9, 1, 12, 0, 0).getTime();

  it("formats recent times as just now", () => {
    expect(formatRelativeTime(now - 2_000, now)).toBe("たった今");
  });

  it("formats minutes, hours and days", () => {
    expect(formatRelativeTime(now - 90_000, now)).toBe("1分前");
    expect(formatRelativeTime(now - 3 * 3_600_000, now)).toBe("3時間前");
    expect(formatRelativeTime(now - 2 * 86_400_000, now)).toBe("2日前");
  });
});

describe("formatRelativeTimeCompact", () => {
  const now = new Date(2026, 9, 1, 12, 0, 0).getTime();

  it("formats compact recent times", () => {
    expect(formatRelativeTimeCompact(now - 2_000, now)).toBe("now");
    expect(formatRelativeTimeCompact(now - 30_000, now)).toBe("30s");
    expect(formatRelativeTimeCompact(now - 120_000, now)).toBe("2m");
    expect(formatRelativeTimeCompact(now - 3 * 3_600_000, now)).toBe("3h");
    expect(formatRelativeTimeCompact(now - 2 * 86_400_000, now)).toBe("2d");
  });
});

describe("formatDurationMs", () => {
  it("formats sub-second and second ranges", () => {
    expect(formatDurationMs(250)).toBe("250ms");
    expect(formatDurationMs(1500)).toBe("1.5s");
  });

  it("formats minute and hour ranges", () => {
    expect(formatDurationMs(64_000)).toBe("1m 4s");
    expect(formatDurationMs(3_900_000)).toBe("1h 5m");
  });
});
