import type { ErrorCode } from "#lib/generated/bindings.ts";
import { ApiError } from "#lib/api/errors.ts";
import { m } from "#lib/paraglide/messages.js";

/**
 * Localized message for any thrown value.
 *
 * Prefers the structured `ErrorCode` (for example `NODE_OFFLINE`) over the
 * English server fallback, so toasts and banners say what happened in the UI
 * language.
 */
export function errorMessage(error: unknown): string {
  if (error instanceof ApiError && error.info) {
    return errorCodeMessage(error.info.code, error.info.params) ?? error.info.message;
  }
  return error instanceof Error ? error.message : String(error);
}

/**
 * Localized label for a structured API error code.
 *
 * Server responses also carry an English fallback `message`; this map lets the
 * UI show a localized explanation when the code is known.
 */
export function errorCodeMessage(
  code: ErrorCode | null,
  params?: Record<string, string>,
): string | null {
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
    case "AUTH_REQUIRED":
      return m.error_auth_required({ agent: params?.agent_id ?? "the agent" });
    case "PTY_DISABLED":
      return m.error_pty_disabled();
    case "COMMAND_DUPLICATE":
      return m.error_command_duplicate();
    case "NOT_FOUND":
      return m.error_not_found();
    case "INTERNAL":
      return m.error_internal();
    case "CONFIG_INVALID":
      return m.error_config_invalid();
    case "AGENT_NOT_INSTALLED":
      return m.error_agent_not_installed();
    case "AGENT_LAUNCH_FAILED":
      return m.error_agent_launch_failed();
    case "AGENT_PROTOCOL_ERROR":
      return m.error_agent_protocol_error();
    case "GIT_UNAVAILABLE":
      return m.error_git_unavailable();
    case "GIT_FAILED":
      return m.error_git_failed();
    case "WORKTREE_CONFLICT":
      return m.error_worktree_conflict();
    case "FORK_UNSUPPORTED":
      return m.error_fork_unsupported();
    case "FORK_SCOPE_UNSUPPORTED":
      return m.error_fork_scope_unsupported();
    case "HISTORY_UNAVAILABLE":
      return m.error_history_unavailable();
    case "SYNC_BACKLOG":
      return m.error_sync_backlog();
    case "DB_CORRUPT":
      return m.error_db_corrupt();
    case "PROVISIONER_FAILED":
      return m.error_provisioner_failed();
    case "VERSION_MISMATCH":
      return m.error_version_mismatch();
    case "TIMEOUT":
      return m.error_timeout();
    case "DISK_FULL":
      return m.error_disk_full();
    case "PERMISSION_DENIED":
      return m.error_permission_denied();
    case "IO_ERROR":
      return m.error_io_error();
    case "PTY_FAILED":
      return m.error_pty_failed();
    case "HUB_ERROR":
      return m.error_hub_error();
    case "CANCELLED":
      return m.error_cancelled();
    case "REVISION_MISMATCH":
      return m.error_invalid_state();
    case "SECRET_MISSING":
      return m.error_not_found();
    case "SECRET_IN_USE":
      return m.error_invalid_state();
    case "SECRET_INVALID":
      return m.error_invalid_state();
  }
}
