import { formatRelativeTime } from "#lib/format.ts";
import type { NodeSummary } from "#lib/generated/bindings.ts";
import type { PathCandidate } from "#lib/new-session.ts";
import { m } from "#lib/paraglide/messages.js";

/**
 * Display label for a working directory candidate.
 *
 * Missing paths are called out first (picking one would only fail at create
 * time), then the worktree flag and branch, then the path itself.
 */
export function candidateLabel(candidate: PathCandidate): string {
  const parts: string[] = [];
  if (!candidate.exists) parts.push(m.new_session_path_missing_label());
  if (candidate.isWorktree) parts.push("[Worktree]");
  if (candidate.gitBranch) parts.push(`[${candidate.gitBranch}]`);
  parts.push(candidate.path);
  const suffix =
    candidate.source === "session" ? ` ${m.new_session_path_from_previous_session()}` : "";
  return `${parts.join(" ")} — ${formatRelativeTime(candidate.usedAt)}${suffix}`;
}

/** Compact secondary line: flags, branch and the last-used time. */
export function candidateDetail(candidate: PathCandidate): string {
  const parts: string[] = [];
  if (!candidate.exists) parts.push(m.new_session_path_missing_label());
  if (candidate.isWorktree) parts.push("Worktree");
  if (candidate.gitBranch) parts.push(candidate.gitBranch);
  parts.push(formatRelativeTime(candidate.usedAt));
  return parts.join(" · ");
}

/** Last path segment (shown as the working-directory pill value). */
export function pathBasename(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts.length > 0 ? parts[parts.length - 1] : path;
}

/**
 * Nodes other than the current one that hold a path for the project.
 *
 * When the current node has no candidate, one of these is one click away from
 * the right directory; the screen lists them instead of a dead end.
 */
export function otherNodesWithPaths(
  nodes: readonly NodeSummary[],
  currentNodeId: string,
  hasPath: (nodeId: string) => boolean,
): NodeSummary[] {
  return nodes.filter((node) => node.node_id !== currentNodeId && hasPath(node.node_id));
}
