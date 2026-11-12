import type { ForkScope, SessionSummary } from "#lib/generated/bindings.ts";

/**
 * Returns the session's fork scope.
 *
 * The projection omits the field for sessions created before the capability
 * was recorded; those sessions cannot fork until the agent reports capabilities.
 */
export function forkScopeOf(session: SessionSummary | null | undefined): ForkScope {
  switch (session?.fork_scope) {
    case "last":
    case "message":
      return session.fork_scope;
    case "none":
    default:
      return "none";
  }
}

/** Whether the session can be forked at all. */
export function supportsFork(session: SessionSummary | null | undefined): boolean {
  return forkScopeOf(session) !== "none";
}
