import type { DataSafety } from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";

/** Localized explanation of what happened to the user's data. */
export function dataSafetyMessage(safety: DataSafety | null | undefined): string | null {
  if (safety === null || safety === undefined) return null;
  if (typeof safety === "object") {
    return m.data_safety_recoverable({ location: safety.recoverable.location });
  }
  switch (safety) {
    case "unchanged":
      return m.data_safety_unchanged();
    case "preserved":
      return m.data_safety_preserved();
    case "not_sent":
      return m.data_safety_not_sent();
    case "lost":
      return m.data_safety_lost();
    case "unknown":
      return m.data_safety_unknown();
  }
}
