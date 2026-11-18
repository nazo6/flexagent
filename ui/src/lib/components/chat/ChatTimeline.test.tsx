import { cleanup, render, screen } from "@solidjs/testing-library";
import { createSignal, flush } from "solid-js";
import { afterEach, describe, expect, it } from "vitest";
import type { TimelineItem } from "#lib/sync/reducer.ts";
import ChatTimeline from "./ChatTimeline.tsx";

afterEach(cleanup);

/**
 * The timeline hands its rows a value context built by a memo. These tests
 * render the real component, so a value/accessor mix-up (which `tsc` cannot
 * catch) fails here instead of on the session screen.
 */
function notice(key: string, text: string, seq: number): TimelineItem {
  return { kind: "notice", key, seq, createdAt: 0, text, tone: "info" };
}

describe("ChatTimeline", () => {
  it("renders notice and user items with a value context", () => {
    render(() => (
      <ChatTimeline
        sessionId="session-1"
        items={[
          notice("notice-1", "first notice", 1),
          {
            kind: "user",
            key: "user-1",
            seq: 2,
            createdAt: 0,
            text: "hello there",
            attachments: [],
            clientSource: "web",
          },
        ]}
      />
    ));

    expect(screen.getByText("first notice")).toBeTruthy();
    expect(screen.getByText("hello there")).toBeTruthy();
  });

  it("keeps rendering rows when the item list is replaced", () => {
    const [items, setItems] = createSignal<TimelineItem[]>([notice("n1", "before", 1)]);
    render(() => <ChatTimeline sessionId="session-1" items={items()} />);
    expect(screen.getByText("before")).toBeTruthy();

    setItems([notice("n1", "after", 1), notice("n2", "second", 2)]);
    flush();

    expect(screen.getByText("after")).toBeTruthy();
    expect(screen.getByText("second")).toBeTruthy();
    expect(screen.queryByText("before")).toBeNull();
  });

  it("shows the empty state without items", () => {
    render(() => <ChatTimeline sessionId="session-1" items={[]} />);
    expect(screen.queryByText("first notice")).toBeNull();
  });
});
