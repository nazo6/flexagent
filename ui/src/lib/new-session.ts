/**
 * Working directory candidate resolution for the new session screen.
 *
 * By default the **most recently used location wins**:
 * 1. Project × node bindings (`project_node_bindings`) in last-used order
 * 2. Paths not covered by a binding are filled in from the `local_path` of
 *    recent sessions (a fallback for sessions started before binding
 *    registration was introduced)
 */
import type { NodeSummary, ProjectSummary, SessionSummary } from "#lib/generated/bindings.ts";

/** Working directory candidate (for display and selection in the UI). */
export interface PathCandidate {
  /** Absolute path of the working directory */
  path: string;
  /** Candidate origin (`binding` = project binding / `session` = session history) */
  source: "binding" | "session";
  /** Whether this is a Git worktree */
  isWorktree: boolean;
  /** Whether it exists on the node (session-only candidates are unverified and treated as true) */
  exists: boolean;
  /** Git branch at the last check (a startup snapshot for session-only candidates) */
  gitBranch: string | null;
  /** Sort key (last-used time for bindings / last-updated time for sessions; Unix epoch ms) */
  usedAt: number;
}

/** Upper bound on candidates filled in from session history (so old locations do not flood the list). */
const MAX_SESSION_CANDIDATES = 5;

/**
 * Build the working directory candidates for a project × node in last-used order.
 *
 * When the same path appears in both a binding and session history, the binding
 * wins (its worktree flag and branch are the authoritative data).
 */
export function buildPathCandidates(
  project: ProjectSummary | null,
  nodeId: string,
  sessions: readonly SessionSummary[],
): PathCandidate[] {
  if (!project || nodeId === "") return [];

  const byPath = new Map<string, PathCandidate>();
  for (const binding of project.bindings) {
    if (binding.node_id !== nodeId) continue;
    byPath.set(binding.local_path, {
      path: binding.local_path,
      source: "binding",
      isWorktree: binding.is_worktree,
      exists: binding.path_exists,
      gitBranch: binding.git_branch,
      usedAt: binding.last_used_at,
    });
  }

  const recentSessions = sessions
    .filter((session) => session.project_id === project.project_id && session.node_id === nodeId)
    .toSorted((a, b) => b.updated_at - a.updated_at);
  let added = 0;
  for (const session of recentSessions) {
    if (added >= MAX_SESSION_CANDIDATES) break;
    if (byPath.has(session.local_path)) continue;
    byPath.set(session.local_path, {
      path: session.local_path,
      source: "session",
      isWorktree: session.is_worktree,
      // Paths derived from session history are unverified (warn when one is picked as the default)
      exists: true,
      gitBranch: session.git_branch,
      usedAt: session.updated_at,
    });
    added += 1;
  }

  return [...byPath.values()].toSorted(
    (a, b) =>
      Number(b.exists) - Number(a.exists) ||
      b.usedAt - a.usedAt ||
      (a.source === b.source ? 0 : a.source === "binding" ? -1 : 1) ||
      Number(a.isWorktree) - Number(b.isWorktree),
  );
}

/**
 * Pick the candidate to select by default.
 *
 * Returns the first existing candidate (i.e. in last-used order). If no
 * candidate exists, returns the first candidate, or `null` when empty.
 */
export function pickDefaultCandidate(candidates: readonly PathCandidate[]): PathCandidate | null {
  return candidates.find((candidate) => candidate.exists) ?? candidates[0] ?? null;
}

/**
 * Return the last-used time for a project × node (the maximum across bindings and session history).
 *
 * Returns `0` when there is no history.
 */
export function projectRecency(
  project: ProjectSummary,
  nodeId: string,
  sessions: readonly SessionSummary[],
): number {
  let latest = 0;
  for (const binding of project.bindings) {
    if (binding.node_id === nodeId) latest = Math.max(latest, binding.last_used_at);
  }
  for (const session of sessions) {
    if (session.project_id === project.project_id && session.node_id === nodeId) {
      latest = Math.max(latest, session.updated_at);
    }
  }
  return latest;
}

/**
 * Pick the default node.
 *
 * Among online nodes, picks the one with the most recent run history (bindings
 * and sessions) for the project. Without history it falls back to the first
 * online node. Returns `""` when no node is online (never selects an offline
 * node, which would only produce `NODE_OFFLINE` errors).
 */
export function pickDefaultNode(
  nodes: readonly NodeSummary[],
  project: ProjectSummary | null,
  sessions: readonly SessionSummary[],
): string {
  const candidates = nodes.filter((node) => node.is_online);
  if (candidates.length === 0) return "";
  if (project) {
    let best: NodeSummary | null = null;
    let bestRecency = 0;
    for (const node of candidates) {
      const recency = projectRecency(project, node.node_id, sessions);
      if (recency > bestRecency) {
        bestRecency = recency;
        best = node;
      }
    }
    if (best !== null) return best.node_id;
  }
  return candidates[0].node_id;
}
