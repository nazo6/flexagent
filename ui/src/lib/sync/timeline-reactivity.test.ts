import { createEffect, createMemo, createRoot, flush } from "solid-js";
import { describe, expect, it } from "vitest";
import { buildTimelineItems, type TimelineSources } from "./reducer.ts";
import { SessionTimeline } from "./timeline.ts";

/**
 * Collect the text of the streamed items each time the timeline rebuilds.
 *
 * Reactivity is what matters here: the streaming buffers are plain `Map`s
 * behind [`ReactiveMap`], so an in-place `set()` must notify the readers of the
 * timeline and rebuild the derived items. If it did not, the UI would show only
 * the first chunk until the finalized event arrives.
 */
function watchStreamedText(timeline: SessionTimeline): { texts: string[]; dispose: () => void } {
  const texts: string[] = [];
  /** Read every timeline field while tracking, so the memo follows each one. */
  const sources = (): TimelineSources => ({
    sessionId: timeline.sessionId,
    events: timeline.events,
    messageDeltas: timeline.messageDeltas,
    thoughtDeltas: timeline.thoughtDeltas,
    subagentByMessage: timeline.subagentByMessage,
    subagentByThought: timeline.subagentByThought,
    toolProgress: timeline.toolProgress,
    terminalDeltas: timeline.terminalDeltas,
    thoughtTimes: timeline.thoughtTimes,
    streamKeys: timeline.streamKeys,
  });
  const dispose = createRoot((disposeRoot) => {
    const items = createMemo(() => buildTimelineItems(sources()));
    createEffect(
      () =>
        items()
          .filter((item) => item.kind === "agent" || item.kind === "thought")
          .map((item) => item.text)
          .join("|"),
      (text) => {
        texts.push(text);
      },
    );
    return disposeRoot;
  });
  flush();
  return { texts, dispose };
}

describe("SessionTimeline reactivity", () => {
  it("rebuilds the timeline for every streamed chunk", () => {
    const timeline = new SessionTimeline("session-1");
    const { texts, dispose } = watchStreamedText(timeline);
    try {
      timeline.applyDelta({
        delta_type: "agent_message_delta",
        message_id: "m1",
        text_delta: "Hel",
      });
      flush();
      timeline.applyDelta({
        delta_type: "agent_message_delta",
        message_id: "m1",
        text_delta: "lo",
      });
      flush();
      timeline.applyDelta({
        delta_type: "agent_message_delta",
        message_id: "m1",
        text_delta: " world",
      });
      flush();

      expect(texts).toEqual(["", "Hel", "Hello", "Hello world"]);
    } finally {
      dispose();
    }
  });

  it("streams reasoning text as well", () => {
    const timeline = new SessionTimeline("session-1");
    const { texts, dispose } = watchStreamedText(timeline);
    try {
      timeline.applyDelta({
        delta_type: "agent_thought_delta",
        thought_id: "t1",
        text_delta: "think",
      });
      flush();
      timeline.applyDelta({
        delta_type: "agent_thought_delta",
        thought_id: "t1",
        text_delta: "ing",
      });
      flush();

      expect(texts).toEqual(["", "think", "thinking"]);
    } finally {
      dispose();
    }
  });

  it("notifies readers of a stream buffer as it is appended", () => {
    // Solid equivalent of the Svelte `$state` proxy assertion: the streaming
    // buffers are plain `Map`s, so writing one must notify the readers of the
    // timeline (a reader that is not subscribed to `streamKeys`).
    const timeline = new SessionTimeline("session-1");
    const seen: Array<string | undefined> = [];
    const dispose = createRoot((disposeRoot) => {
      createEffect(
        () => timeline.messageDeltas.get("m1"),
        (text) => {
          seen.push(text);
        },
      );
      return disposeRoot;
    });
    try {
      flush();
      timeline.applyDelta({
        delta_type: "agent_message_delta",
        message_id: "m1",
        text_delta: "Hel",
      });
      flush();
      timeline.applyDelta({
        delta_type: "agent_message_delta",
        message_id: "m1",
        text_delta: "lo",
      });
      flush();

      expect(seen).toEqual([undefined, "Hel", "Hello"]);
    } finally {
      dispose();
    }
  });
});
