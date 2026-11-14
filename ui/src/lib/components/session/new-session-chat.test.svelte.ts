import { flushSync, mount, unmount } from "svelte";
import { describe, expect, it, vi } from "vitest";
import { page } from "$app/state";
import type {
  AgentCapabilitiesPreview,
  AgentSummary,
  NodeSummary,
  ProjectSummary,
} from "#lib/generated/bindings.ts";
import NewSessionChat from "./NewSessionChat.svelte";
import { sync } from "#lib/stores/app.svelte.ts";

function agent(id: string, name: string): AgentSummary {
  return {
    id,
    name,
    version: "1.0.0",
    description: null,
    sources: [],
    external_path: null,
    custom_config: null,
    extension: null,
    capabilities: {},
    backend: "acp",
    icon: null,
    installed_nodes: ["n1"],
  } as unknown as AgentSummary;
}

function node(): NodeSummary {
  return {
    node_id: "n1",
    name: "Node One",
    os: "linux",
    arch: "x86_64",
    version: "0.1.0",
    is_ephemeral: false,
    provisioner: null,
    lifecycle_status: "ready",
    is_online: true,
    last_seen_at: 0,
    installed_agents: ["agent2"],
  } as unknown as NodeSummary;
}

function project(): ProjectSummary {
  return {
    project_id: "p1",
    name: "Demo Repo",
    canonical_git_url: null,
    identity: { kind: "local", key: "p1" },
    created_at: 0,
    updated_at: 0,
    bindings: [
      {
        node_id: "n1",
        local_path: "/repo/main",
        is_worktree: false,
        path_exists: true,
        git_branch: "main",
        last_used_at: 100,
      },
    ],
  } as unknown as ProjectSummary;
}

/** Seed the sync singleton and the node / agent APIs used on mount. */
function seed(url: string): void {
  page.url = new URL(url);
  sync.projects = [project()];
  sync.nodes = [node()];
  sync.sessions = [];
  sync.connection.client.provisioners = async () => [];
  sync.connection.client.nodeAgents = async () => [agent("agent2", "Second Agent")];
  sync.connection.client.probeAgent = async () =>
    ({ available_modes: [], config_options: [] }) as AgentCapabilitiesPreview;
}

/**
 * The screen resolves project / node / path / agent from derived defaults
 * instead of initialization effects. Mounting would throw
 * `effect_update_depth_exceeded` if a derived default fed itself.
 */
describe("NewSessionChat defaults", () => {
  it("resolves project / node / path / agent", async () => {
    seed("http://localhost/");
    const target = document.createElement("div");
    document.body.appendChild(target);
    const component = mount(NewSessionChat, { target });
    try {
      flushSync();
      await vi.waitFor(() => expect(target.textContent ?? "").toContain("Second Agent"));
      const text = target.textContent ?? "";
      expect(text).toContain("Demo Repo");
      expect(text).toContain("main");
    } finally {
      unmount(component);
    }
  });

  it("applies deep-link parameters once", async () => {
    seed("http://localhost/?project=p1&node=n1&path=/repo/custom&agent=ghost-agent");
    const target = document.createElement("div");
    document.body.appendChild(target);
    const component = mount(NewSessionChat, { target });
    try {
      flushSync();
      // The agent is missing from the catalog, so the raw ID is shown.
      await vi.waitFor(() => expect(target.textContent ?? "").toContain("ghost-agent"));
      const text = target.textContent ?? "";
      expect(text).toContain("Demo Repo");
      expect(text).toContain("custom");
    } finally {
      unmount(component);
    }
  });
});
