import type { BadgeVariant } from "#lib/components/ui/badge/index.ts";
import type { SessionStatus, SessionSummary } from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";

/** Whether a session status is waiting for a user decision (approval or structured input). */
export function isWaitingSessionStatus(status: SessionStatus): boolean {
  return status === "waiting_permission" || status === "waiting_input";
}

/**
 * Whether a session should be surfaced in the "Needs attention" group / badges.
 *
 * Waiting sessions stay visible even when archived so a paused agent cannot be
 * hidden accidentally; error sessions are shown only while unarchived.
 */
export function sessionNeedsAttention(session: SessionSummary): boolean {
  return (
    isWaitingSessionStatus(session.status) ||
    (session.archived_at === null && session.status === "error")
  );
}

/** Compact label for attention badges inside narrow session rows. */
export function sessionAttentionBadgeLabel(status: SessionStatus): string | null {
  switch (status) {
    case "waiting_permission":
      return m.shell_badge_approval();
    case "waiting_input":
      return m.shell_badge_input();
    case "error":
      return m.status_error();
    default:
      return null;
  }
}

/** English label for a session status. */
export function sessionStatusLabel(status: SessionStatus): string {
  switch (status) {
    case "provisioning":
      return m.status_provisioning();
    case "idle":
      return m.status_idle();
    case "running":
      return m.status_running();
    case "waiting_permission":
      return m.status_waiting_permission();
    case "waiting_input":
      return m.status_waiting_input();
    case "stopped":
      return m.status_stopped();
    case "error":
      return m.status_error();
  }
}

/** Badge variant for a session status. */
export function sessionStatusVariant(status: SessionStatus): BadgeVariant {
  switch (status) {
    case "running":
      return "default";
    case "waiting_permission":
      return "destructive";
    case "waiting_input":
      return "destructive";
    case "error":
      return "destructive";
    case "stopped":
      return "outline";
    default:
      return "secondary";
  }
}

/** Resolve an English label for a permission request option kind (falls back to the original name). */
export function permissionOptionLabel(name: string, kind: string): string {
  if (name) return name;
  switch (kind) {
    case "allow_once":
      return m.permission_allow_once();
    case "allow_always":
      return m.permission_allow_always();
    case "reject_once":
      return m.permission_reject_once();
    case "reject_always":
      return m.permission_reject_always();
    default:
      return kind;
  }
}

/** Abbreviate a node ID for display (truncate long auto-generated IDs). */
export function shortId(id: string, max = 24): string {
  return id.length <= max ? id : `${id.slice(0, max - 1)}…`;
}
