import { render, waitFor } from "@solidjs/testing-library";
import { describe, expect, it, vi } from "vitest";
import type {
  AgentCapabilitiesPreview,
  AgentSummary,
  NodeSummary,
  ProjectSummary,
} from "#lib/generated/bindings.ts";
import NewSessionChat from "./NewSessionChat.tsx";
import { sync } from "#lib/stores/app.ts";

/**
 * `useSearchParams` is the Solid replacement for Svelte's `page.url`: the
 * screen reads the deep link once when it is created. Assigning the object the
 * hook returns avoids mounting a whole router for a query string.
 */
const searchParams = vi.hoisted(() => ({ current: {} as Record<string, string> }));

vi.mock("@solidjs/router", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@solidjs/router")>();
  return {
    ...actual,
    useSearchParams: () => [searchParams.current, () => {}],
    useNavigate: () => () => {},
  };
});

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
function seed(params: Record<string, string> = {}): void {
  searchParams.current = params;
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
 * instead of initialization effects. A derived default that fed itself would
 * make the screen fail to render (`REACTIVITY_HALTED` in Solid).
 */
describe("NewSessionChat defaults", () => {
  it("resolves project / node / path / agent", async () => {
    seed();
    const { container } = render(() => <NewSessionChat />);

    await waitFor(() => expect(container.textContent ?? "").toContain("Second Agent"));
    const text = container.textContent ?? "";
    expect(text).toContain("Demo Repo");
    expect(text).toContain("main");
  });

  it("applies deep-link parameters once", async () => {
    seed({
      project: "p1",
      node: "n1",
      path: "/repo/custom",
      agent: "ghost-agent",
    });
    const { container } = render(() => <NewSessionChat />);

    // The agent is missing from the catalog, so the raw ID is shown.
    await waitFor(() => expect(container.textContent ?? "").toContain("ghost-agent"));
    const text = container.textContent ?? "";
    expect(text).toContain("Demo Repo");
    expect(text).toContain("custom");
  });
});
