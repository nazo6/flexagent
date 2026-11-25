import { flushSync, mount, unmount } from "svelte";
import { describe, expect, it, vi } from "vitest";
import type { SearchHit } from "#lib/generated/bindings.ts";
import { sync } from "#lib/stores/app.svelte.ts";
import TurnOutline from "./TurnOutline.svelte";

function hit(nodeSeq: number, snippet: string): SearchHit {
  return {
    session_id: "session-1",
    event_id: `event-${nodeSeq}`,
    node_seq: nodeSeq,
    event_type: "user_message",
    snippet,
    created_at: 0,
  };
}

/** Mount the outline and let the search debounce (250ms) elapse. */
async function search(target: HTMLElement, value: string): Promise<void> {
  const input = target.querySelector<HTMLInputElement>("input[type='search']");
  expect(input).not.toBeNull();
  input!.value = value;
  input!.dispatchEvent(new Event("input", { bubbles: true }));
  flushSync();
  await new Promise((resolve) => setTimeout(resolve, 320));
  flushSync();
}

describe("TurnOutline in-session search", () => {
  it("renders scoped hits and jumps to the selected event", async () => {
    const searchSpy = vi
      .spyOn(sync.connection.client, "search")
      .mockResolvedValue([hit(7, "before [term] after")]);
    const onjumpseq = vi.fn<(seq: number) => void>();
    const onclose = vi.fn<() => void>();
    const target = document.createElement("div");
    document.body.appendChild(target);
    const component = mount(TurnOutline, {
      target,
      props: {
        turns: [],
        sessionId: "session-1",
        onjump: () => {},
        onjumpseq,
        onclose,
      },
    });
    try {
      await search(target, "term");
      expect(searchSpy).toHaveBeenCalledWith("term", 50, "session-1");
      const row = target.querySelector<HTMLButtonElement>('[data-search-hit="7"]');
      expect(row).not.toBeNull();
      expect(row!.textContent).toContain("term");
      expect(target.querySelector("mark")?.textContent).toBe("term");

      row!.click();
      flushSync();
      expect(onjumpseq).toHaveBeenCalledWith(7);
      expect(onclose).toHaveBeenCalled();
    } finally {
      unmount(component);
      target.remove();
      searchSpy.mockRestore();
    }
  });

  it("loads more results when the first page is full", async () => {
    const firstPage = Array.from({ length: 50 }, (_, index) => hit(index + 1, "[term] result"));
    const searchSpy = vi.spyOn(sync.connection.client, "search").mockResolvedValue(firstPage);
    const target = document.createElement("div");
    document.body.appendChild(target);
    const component = mount(TurnOutline, {
      target,
      props: {
        turns: [],
        sessionId: "session-1",
        onjump: () => {},
        onjumpseq: () => {},
        onclose: () => {},
      },
    });
    try {
      await search(target, "term");
      const more = Array.from(target.querySelectorAll("button")).find(
        (button) => button.textContent?.trim() === "Show more",
      );
      expect(more).toBeDefined();
      more!.click();
      await new Promise((resolve) => setTimeout(resolve, 320));
      flushSync();
      expect(searchSpy).toHaveBeenLastCalledWith("term", 100, "session-1");
    } finally {
      unmount(component);
      target.remove();
      searchSpy.mockRestore();
    }
  });

  it("shows an empty state when the session has no match", async () => {
    const searchSpy = vi.spyOn(sync.connection.client, "search").mockResolvedValue([]);
    const target = document.createElement("div");
    document.body.appendChild(target);
    const component = mount(TurnOutline, {
      target,
      props: {
        turns: [],
        sessionId: "session-1",
        onjump: () => {},
        onjumpseq: () => {},
        onclose: () => {},
      },
    });
    try {
      await search(target, "nothing");
      expect(target.textContent).toContain("No matches in this session");
    } finally {
      unmount(component);
      target.remove();
      searchSpy.mockRestore();
    }
  });
});
