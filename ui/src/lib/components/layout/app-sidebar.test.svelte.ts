import { flushSync, mount, unmount } from "svelte";
import { describe, expect, it } from "vitest";
import { page } from "$app/state";
import type { ProjectSummary } from "#lib/generated/bindings.ts";
import AppSidebar from "./AppSidebar.svelte";
import { sync } from "#lib/stores/app.svelte.ts";

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

function seed(projects: ProjectSummary[]): HTMLElement {
  page.url = new URL("http://localhost/");
  sync.projects = projects;
  sync.nodes = [];
  sync.sessions = [];
  const target = document.createElement("div");
  document.body.appendChild(target);
  return target;
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
