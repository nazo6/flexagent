import type { RecoveryAction } from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";

/** Localized button label for a server-provided recovery action. */
export function recoveryActionLabel(action: RecoveryAction): string {
  switch (action.kind) {
    case "retry":
      return action.idempotent ? m.recovery_retry() : m.recovery_retry_unsafe();
    case "resume_with_replay":
      return m.recovery_resume_with_replay();
    case "reconnect_node":
      return m.recovery_reconnect_node();
    case "select_online_node":
      return m.recovery_select_online_node();
    case "reinstall_agent":
      return m.recovery_reinstall_agent();
    case "login_agent":
      return m.recovery_login_agent();
    case "recreate_worktree":
      return m.recovery_recreate_worktree();
    case "revert_to_nearest_snapshot":
      return m.recovery_revert_snapshot();
    case "open_config":
      return m.recovery_open_config();
    case "show_logs":
      return m.recovery_show_logs();
    case "export_diagnostics":
      return m.recovery_export_diagnostics();
  }
}

/** Stable key for a recovery action (used for keyed rendering). */
export function recoveryActionKey(action: RecoveryAction): string {
  switch (action.kind) {
    case "retry":
      return `retry:${action.idempotent}`;
    case "resume_with_replay":
      return `resume:${action.session_id}`;
    case "reconnect_node":
      return `reconnect:${action.node_id}`;
    case "reinstall_agent":
      return `reinstall:${action.node_id}:${action.agent_id}`;
    case "login_agent":
      return `login:${action.agent_id}`;
    case "recreate_worktree":
      return `worktree:${action.project_id}:${action.branch ?? ""}`;
    case "revert_to_nearest_snapshot":
      return `revert:${action.session_id}:${action.node_seq ?? ""}`;
    case "show_logs":
      return `logs:${action.session_id ?? "all"}`;
    case "open_config":
      return `config:${action.path}`;
    default:
      return action.kind;
  }
}
