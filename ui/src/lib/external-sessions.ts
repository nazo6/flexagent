import type { ExternalSessionSummary, ProjectSummary } from "#lib/generated/bindings.ts";

/**
 * Grouping helpers for the external session list (`/external`).
 *
 * Sessions come from the agent side and are not necessarily part of a registered
 * project. They are grouped by their logical project when one matches, and by
 * their exact working directory otherwise, so one unregistered repository no
 * longer merges every unrelated folder into a single huge group.
 */

/** One project binding on the selected node, with a normalized path. */
export interface ExternalProjectPath {
  projectId: string;
  name: string;
  /** Path used for matching (separators unified, trailing slashes removed). */
  path: string;
}

/** A group of external sessions shown under one header. */
export interface ExternalSessionGroup {
  /** Stable key for the keyed list and the per-device collapsed state. */
  key: string;
  /** Project name, or the folder's basename for an unregistered folder. */
  name: string;
  /** Full working directory for a folder group (`null` for a project group). */
  path: string | null;
  /** Sessions, newest first. */
  sessions: ExternalSessionSummary[];
}

/**
 * Normalize a local path for comparison.
 *
 * Windows paths are case-insensitive, so callers pass `caseInsensitive` when the
 * target node runs Windows.
 */
export function normalizeExternalPath(path: string, caseInsensitive: boolean): string {
  const normalized = path.replaceAll("\\", "/").replace(/\/+$/, "");
  return caseInsensitive ? normalized.toLowerCase() : normalized;
}

/** Last path segment of a POSIX or Windows path. */
export function externalPathBasename(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts.length > 0 ? parts[parts.length - 1] : path;
}

/** Paths of every project binding on one node. */
export function externalProjectPaths(
  projects: readonly ProjectSummary[],
  nodeId: string,
  caseInsensitive: boolean,
): ExternalProjectPath[] {
  const paths: ExternalProjectPath[] = [];
  for (const project of projects) {
    for (const binding of project.bindings) {
      if (binding.node_id !== nodeId) continue;
      paths.push({
        projectId: project.project_id,
        name: project.name,
        path: normalizeExternalPath(binding.local_path, caseInsensitive),
      });
    }
  }
  return paths;
}

/**
 * Group sessions by their logical project, or by the exact working directory
 * when the folder is not a registered project.
 *
 * `sessions` must already be sorted newest first: the first session seen for a
 * group is its latest activity, so insertion order doubles as the group order
 * (most recently active first). This keeps recently used projects at the top
 * instead of sorting them alphabetically.
 */
export function groupExternalSessions(
  sessions: readonly ExternalSessionSummary[],
  projectPaths: readonly ExternalProjectPath[],
  caseInsensitive: boolean,
): ExternalSessionGroup[] {
  const groups = new Map<string, ExternalSessionGroup>();
  for (const session of sessions) {
    const cwd = normalizeExternalPath(session.cwd, caseInsensitive);
    let project: ExternalProjectPath | null = null;
    for (const candidate of projectPaths) {
      const matches = cwd === candidate.path || cwd.startsWith(`${candidate.path}/`);
      if (matches && (project === null || candidate.path.length > project.path.length)) {
        project = candidate;
      }
    }

    const key = project ? `project:${project.projectId}` : `folder:${cwd}`;
    const group = groups.get(key);
    if (group) {
      group.sessions.push(session);
      continue;
    }
    groups.set(key, {
      key,
      name: project ? project.name : externalPathBasename(session.cwd),
      path: project ? null : session.cwd,
      sessions: [session],
    });
  }
  return [...groups.values()];
}
