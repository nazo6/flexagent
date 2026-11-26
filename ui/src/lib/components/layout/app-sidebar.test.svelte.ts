import { flushSync, mount, unmount } from "svelte";
import { describe, expect, it } from "vitest";
import { page } from "$app/state";
import type { ProjectSummary, SessionSummary } from "#lib/generated/bindings.ts";
import AppSidebar from "./AppSidebar.svelte";
import { sync } from "#lib/stores/app.svelte.ts";

const COLLAPSED_PROJECTS_KEY = "fxg:sidebar_collapsed_projects";

function project(projectId: string, hasSessions: boolean): ProjectSummary {
  return {
    project_id: projectId,
    name: projectId,
    canonical_git_url: null,
    identity: { kind: "local", key: projectId },
    created_at: 0,
    updated_at: 0,
    has_sessions: hasSessions,
    bindings: [],
  };
}

function session(
  overrides: Partial<SessionSummary> &
    Pick<SessionSummary, "session_id" | "project_id" | "title" | "status">,
): SessionSummary {
  return {
    node_id: "node-1",
    local_path: `/tmp/${overrides.project_id}`,
    git_branch: null,
    is_worktree: false,
    agent_id: "opencode",
    agent_session_id: null,
    parent_session_id: null,
    current_mode: null,
    usage: null,
    last_node_seq: 0,
    created_at: 0,
    updated_at: 0,
    archived_at: null,
    ...overrides,
  };
}

function seed(projects: ProjectSummary[], sessions: SessionSummary[] = []): HTMLElement {
  page.url = new URL("http://localhost/");
  sync.projects = projects;
  sync.nodes = [];
  sync.sessions = sessions;
  const target = document.createElement("div");
  document.body.appendChild(target);
  return target;
}

function occurrences(text: string, needle: string): number {
  return text.split(needle).length - 1;
}

/** Renders the sidebar and returns its text after the project groups settle. */
function render(target: HTMLElement): { component: ReturnType<typeof mount>; text: string } {
  const component = mount(AppSidebar, { target });
  flushSync();
  return { component, text: target.textContent ?? "" };
}

describe("AppSidebar project groups", () => {
  it("offers to start a session for a project that has none", () => {
    const target = seed([project("empty-project", false)]);
    const { component, text } = render(target);
    try {
      expect(text).toContain("Start a session");
      expect(text).not.toContain("Load sessions");
    } finally {
      unmount(component);
    }
  });

  it("offers to load the first page of a project that has unloaded sessions", () => {
    const target = seed([project("busy-project", true)]);
    const { component, text } = render(target);
    try {
      expect(text).toContain("Load sessions");
      expect(text).not.toContain("Start a session");
    } finally {
      unmount(component);
    }
  });
});

describe("AppSidebar attention filter", () => {
  it("lists a session once and filters down to the sessions that need attention", () => {
    const target = seed(
      [project("alpha-app", true)],
      [
        session({
          session_id: "waiting",
          project_id: "alpha-app",
          title: "Waiting session",
          status: "waiting_input",
          updated_at: 2,
        }),
        session({
          session_id: "calm",
          project_id: "alpha-app",
          title: "Calm session",
          status: "idle",
          updated_at: 1,
        }),
      ],
    );
    const { component } = render(target);
    try {
      expect(occurrences(target.textContent ?? "", "Waiting session")).toBe(1);

      const strip = target.querySelector<HTMLButtonElement>('button[aria-pressed="false"]');
      expect(strip).not.toBeNull();
      strip?.click();
      flushSync();

      expect(target.textContent).toContain("Waiting session");
      expect(target.textContent).not.toContain("Calm session");
      expect(strip?.getAttribute("aria-pressed")).toBe("true");
    } finally {
      unmount(component);
    }
  });
});

describe("AppSidebar project collapse", () => {
  it("restores collapsed projects on the next mount", () => {
    localStorage.clear();
    const target = seed([project("alpha-app", false)]);
    const first = render(target);
    const header = Array.from(target.querySelectorAll("button")).find((button) =>
      button.textContent?.includes("alpha-app"),
    );
    header?.click();
    flushSync();
    expect(localStorage.getItem(COLLAPSED_PROJECTS_KEY)).toBe('["alpha-app"]');
    unmount(first.component);

    const second = render(target);
    try {
      expect(target.textContent).not.toContain("Start a session");
    } finally {
      unmount(second.component);
    }
  });
});

describe("AppSidebar selection", () => {
  it("marks the open session as the current page with a surface beyond color", () => {
    localStorage.clear();
    const target = seed(
      [project("selection-app", true)],
      [
        session({
          session_id: "open",
          project_id: "selection-app",
          title: "Open session",
          status: "idle",
          updated_at: 2,
        }),
        session({
          session_id: "other",
          project_id: "selection-app",
          title: "Other session",
          status: "idle",
          updated_at: 1,
        }),
      ],
    );
    page.url = new URL("http://localhost/sessions/open");
    const { component } = render(target);
    try {
      const current = target.querySelector<HTMLAnchorElement>('a[href="/sessions/open"]');
      expect(current?.getAttribute("aria-current")).toBe("page");
      // The fill alone is too close to the sidebar in the light palettes;
      // the hairline and the weight carry the selection with it.
      expect(current?.className).toContain("bg-selection");
      expect(current?.className).toContain("ring-1");
      expect(current?.className).toContain("font-semibold");

      const other = target.querySelector<HTMLAnchorElement>('a[href="/sessions/other"]');
      expect(other?.getAttribute("aria-current")).toBeNull();
      expect(other?.className).not.toContain("bg-selection");
    } finally {
      unmount(component);
    }
  });
});
