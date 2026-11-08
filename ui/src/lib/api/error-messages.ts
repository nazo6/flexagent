import type { ErrorCode } from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";

/**
 * English fallback label for a structured API error code.
 *
 * Server responses usually carry a human-readable `message`; this map covers
 * the cases where only the code is available (for example `CommandResult.code`).
 */
export function errorCodeMessage(code: ErrorCode | null): string | null {
  switch (code) {
    case null:
      return null;
    case "UNAUTHORIZED":
      return m.error_unauthorized();
    case "FORBIDDEN":
      return m.error_forbidden();
    case "NODE_OFFLINE":
      return m.error_node_offline();
    case "ALREADY_RESOLVED":
      return m.error_already_resolved();
    case "BUSY":
      return m.error_busy();
    case "INVALID_STATE":
      return m.error_invalid_state();
    case "RESUME_REQUIRED":
      return m.error_resume_required();
    case "PTY_DISABLED":
      return m.error_pty_disabled();
    case "COMMAND_DUPLICATE":
      return m.error_command_duplicate();
    case "NOT_FOUND":
      return m.error_not_found();
    case "INTERNAL":
      return m.error_internal();
  }
}
