import { describe, expect, it } from "vitest";
import type { NodeSummary } from "$lib/generated/NodeSummary";
import type { ProjectSummary } from "$lib/generated/ProjectSummary";
import type { SessionSummary } from "$lib/generated/SessionSummary";
import {
  buildPathCandidates,
  pickDefaultCandidate,
  pickDefaultNode,
  projectRecency,
} from "./new-session";

function node(node_id: string, is_online = true, name = node_id): NodeSummary {
  return {
    node_id,
    name,
    os: "linux",
    arch: "x86_64",
    version: "0.1.0",
    is_ephemeral: false,
    provisioner: null,
    lifecycle_status: "ready",
    is_online,
    last_seen_at: 0,
    installed_agents: ["opencode2"],
  };
}

function project(
  bindings: ProjectSummary["bindings"],
  project_id = "github.com/nazo6/flexagent",
): ProjectSummary {
  return {
    project_id,
    name: "flexagent",
    canonical_git_url: "git@github.com:nazo6/flexagent.git",
    created_at: 0,
    updated_at: 0,
    bindings,
  };
}

function binding(
  local_path: string,
  node_id = "n1",
  last_used_at = 0,
  is_worktree = false,
  path_exists = true,
): ProjectSummary["bindings"][number] {
  return {
    node_id,
    local_path,
    is_worktree,
    path_exists,
    git_branch: is_worktree ? "feat/x" : "main",
    last_used_at,
  };
}

function session(
  local_path: string,
  updated_at: number,
  overrides: Partial<SessionSummary> = {},
): SessionSummary {
  return {
    session_id: `s-${local_path}-${updated_at}`,
    project_id: "github.com/nazo6/flexagent",
    node_id: "n1",
    local_path,
    git_branch: "main",
    is_worktree: false,
    agent_id: "opencode2",
    agent_session_id: null,
    parent_session_id: null,
    fork_from_node_seq: null,
    title: "New Session",
    status: "stopped",
    current_mode: null,
    usage: null,
    last_node_seq: 1,
    created_at: updated_at,
    updated_at,
    archived_at: null,
    ...overrides,
  };
}

describe("buildPathCandidates", () => {
  it("returns node-scoped bindings sorted by recency", () => {
    const candidates = buildPathCandidates(
      project([
        binding("/repo/main", "n1", 100),
        binding("/repo/wt", "n1", 300, true),
        binding("/other", "n2", 999),
      ]),
      "n1",
      [],
    );
    expect(candidates.map((c) => c.path)).toEqual(["/repo/wt", "/repo/main"]);
    expect(candidates[0]).toMatchObject({
      source: "binding",
      isWorktree: true,
      exists: true,
      gitBranch: "feat/x",
      usedAt: 300,
    });
  });

  it("sorts existing paths before stale ones and skips stale for the default", () => {
    const candidates = buildPathCandidates(
      project([
        binding("/repo/stale-wt", "n1", 900, true, false),
        binding("/repo/main", "n1", 100),
        binding("/repo/stale-main", "n1", 800, false, false),
      ]),
      "n1",
      [],
    );
    expect(candidates.map((c) => c.path)).toEqual([
      "/repo/main",
      "/repo/stale-wt",
      "/repo/stale-main",
    ]);
    expect(candidates[0]).toMatchObject({ path: "/repo/main", exists: true });
    expect(pickDefaultCandidate(candidates)?.path).toBe("/repo/main");

    // 実在する候補が無い場合は先頭 (直近使用) を返す
    const staleOnly = candidates.filter((c) => !c.exists);
    expect(pickDefaultCandidate(staleOnly)?.path).toBe("/repo/stale-wt");
    expect(pickDefaultCandidate([])).toBeNull();
  });

  it("fills session-only paths by recency and prefers bindings for duplicates", () => {
    const candidates = buildPathCandidates(project([binding("/repo/main", "n1", 100)]), "n1", [
      session("/repo/main", 999),
      session("/legacy", 500),
      session("/older", 200),
      session("/other-node", 9_999, { node_id: "n2" }),
    ]);
    expect(candidates.map((c) => c.path)).toEqual(["/legacy", "/older", "/repo/main"]);
    expect(candidates[2]).toMatchObject({ source: "binding", usedAt: 100 });
    expect(candidates[0]).toMatchObject({ source: "session", usedAt: 500 });
  });

  it("caps session-derived candidates and returns empty for missing inputs", () => {
    const sessions = Array.from({ length: 8 }, (_, i) =>
      session(`/legacy-${i}`, 1_000 - i, { node_id: "n1" }),
    );
    const candidates = buildPathCandidates(project([]), "n1", sessions);
    expect(candidates).toHaveLength(5);
    expect(candidates[0].path).toBe("/legacy-0");

    expect(buildPathCandidates(null, "n1", sessions)).toEqual([]);
    expect(buildPathCandidates(project([binding("/repo")]), "", sessions)).toEqual([]);
  });
});

describe("projectRecency", () => {
  it("takes the maximum of bindings and sessions for the node", () => {
    const recency = projectRecency(
      project([binding("/repo", "n1", 100), binding("/repo", "n2", 9_000)]),
      "n1",
      [session("/legacy", 400)],
    );
    expect(recency).toBe(400);
    expect(projectRecency(project([]), "n1", [])).toBe(0);
  });
});

describe("pickDefaultNode", () => {
  it("prefers the online node with the most recent project history", () => {
    const nodes = [node("n1"), node("n2")];
    const p = project([binding("/repo", "n2", 500)]);
    expect(pickDefaultNode(nodes, p, [])).toBe("n2");

    const withSession = pickDefaultNode(nodes, p, [session("/repo", 1_000, { node_id: "n1" })]);
    expect(withSession).toBe("n1");
  });

  it("falls back to the first online node without history", () => {
    const nodes = [node("n1", false), node("n2"), node("n3")];
    expect(pickDefaultNode(nodes, project([binding("/repo", "n1", 100)]), [])).toBe("n2");
    expect(pickDefaultNode([], project([]), [])).toBe("");
  });

  it("uses all nodes when none are online", () => {
    const nodes = [node("n1", false), node("n2", false)];
    expect(pickDefaultNode(nodes, project([binding("/repo", "n2", 100)]), [])).toBe("n2");
    expect(pickDefaultNode(nodes, project([]), [])).toBe("n1");
  });
});
