import { describe, expect, it } from "vitest";
import type { ExternalSessionSummary, ProjectSummary } from "#lib/generated/bindings.ts";
import {
  externalPathBasename,
  externalProjectPaths,
  groupExternalSessions,
  normalizeExternalPath,
} from "./external-sessions.ts";

function project(projectId: string, localPath: string, nodeId = "node-1"): ProjectSummary {
  return {
    project_id: projectId,
    name: projectId,
    canonical_git_url: null,
    identity: { kind: "local", key: projectId },
    created_at: 0,
    updated_at: 0,
    has_sessions: true,
    bindings: [
      {
        node_id: nodeId,
        local_path: localPath,
        is_worktree: false,
        path_exists: true,
        git_branch: null,
        last_used_at: 0,
      },
    ],
  };
}

function summary(agentSessionId: string, cwd: string): ExternalSessionSummary {
  return {
    agent_id: "opencode",
    agent_session_id: agentSessionId,
    cwd,
    title: agentSessionId,
    updated_at: null,
    imported_session_id: null,
  };
}

describe("externalProjectPaths", () => {
  it("collects bindings for the selected node only", () => {
    const paths = externalProjectPaths(
      [project("alpha", "/work/alpha/"), project("beta", "C:\\work\\beta", "node-2")],
      "node-1",
      false,
    );
    expect(paths).toEqual([{ projectId: "alpha", name: "alpha", path: "/work/alpha" }]);
  });
});

describe("groupExternalSessions", () => {
  it("uses the longest matching project binding", () => {
    const groups = groupExternalSessions(
      [summary("s1", "/work/alpha/packages/app")],
      externalProjectPaths(
        [project("work", "/work"), project("alpha", "/work/alpha")],
        "node-1",
        false,
      ),
      false,
    );
    expect(groups).toHaveLength(1);
    expect(groups[0]?.key).toBe("project:alpha");
  });

  it("splits unregistered folders by working directory", () => {
    const groups = groupExternalSessions(
      [summary("s1", "/tmp/one"), summary("s2", "/tmp/two"), summary("s3", "/tmp/one")],
      [],
      false,
    );
    expect(groups.map((group) => group.key)).toEqual(["folder:/tmp/one", "folder:/tmp/two"]);
    expect(groups[0]?.name).toBe("one");
    expect(groups[0]?.path).toBe("/tmp/one");
    expect(groups[0]?.sessions.map((session) => session.agent_session_id)).toEqual(["s1", "s3"]);
    expect(groups[1]?.name).toBe("two");
  });

  it("orders groups by their first session in the input (newest first)", () => {
    const groups = groupExternalSessions(
      [summary("s1", "/tmp/newest"), summary("s2", "/tmp/older")],
      [],
      false,
    );
    expect(groups.map((group) => group.name)).toEqual(["newest", "older"]);
  });

  it("matches Windows paths case-insensitively", () => {
    const groups = groupExternalSessions(
      [summary("s1", "C:\\Work\\Alpha\\sub")],
      externalProjectPaths([project("alpha", "c:/work/alpha")], "node-1", true),
      true,
    );
    expect(groups[0]?.key).toBe("project:alpha");
  });
});

describe("path helpers", () => {
  it("normalizes separators and trailing slashes", () => {
    expect(normalizeExternalPath("C:\\work\\alpha\\", true)).toBe("c:/work/alpha");
    expect(normalizeExternalPath("/work/alpha///", false)).toBe("/work/alpha");
  });

  it("returns the last segment of POSIX and Windows paths", () => {
    expect(externalPathBasename("/work/alpha")).toBe("alpha");
    expect(externalPathBasename("C:\\work\\alpha")).toBe("alpha");
    expect(externalPathBasename("/")).toBe("/");
  });
});
