import { flushSync } from "svelte";
import { describe, expect, it } from "vitest";
import { buildTimelineItems, type TimelineSources } from "./reducer.ts";
import { SessionTimeline } from "./timeline.svelte.ts";

/**
 * Collect the text of the streamed items each time the timeline rebuilds.
 *
 * Reactivity is what breaks here: with a plain `Map` in `$state`, the Svelte
 * compiler does not proxy it, so `set()` never invalidates the `$derived` and
 * the UI shows only the first chunk until the finalized event arrives.
 */
function watchStreamedText(timeline: SessionTimeline): { texts: string[]; stop: () => void } {
  const texts: string[] = [];
  const sources: TimelineSources = {
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
  };
  const stop = $effect.root(() => {
    const items = $derived(buildTimelineItems(sources));
    $effect(() => {
      texts.push(
        items
          .filter((item) => item.kind === "agent" || item.kind === "thought")
          .map((item) => item.text)
          .join("|"),
      );
    });
  });
  flushSync();
  return { texts, stop: () => stop() };
}

describe("SessionTimeline reactivity", () => {
  it("rebuilds the timeline for every streamed chunk", () => {
    const timeline = new SessionTimeline("session-1");
    const { texts, stop } = watchStreamedText(timeline);
    try {
      timeline.applyDelta({
        delta_type: "agent_message_delta",
        message_id: "m1",
        text_delta: "Hel",
      });
      flushSync();
      timeline.applyDelta({
        delta_type: "agent_message_delta",
        message_id: "m1",
        text_delta: "lo",
      });
      flushSync();
      timeline.applyDelta({
        delta_type: "agent_message_delta",
        message_id: "m1",
        text_delta: " world",
      });
      flushSync();

      expect(texts).toEqual(["", "Hel", "Hello", "Hello world"]);
    } finally {
      stop();
    }
  });

  it("streams reasoning text as well", () => {
    const timeline = new SessionTimeline("session-1");
    const { texts, stop } = watchStreamedText(timeline);
    try {
      timeline.applyDelta({
        delta_type: "agent_thought_delta",
        thought_id: "t1",
        text_delta: "think",
      });
      flushSync();
      timeline.applyDelta({
        delta_type: "agent_thought_delta",
        thought_id: "t1",
        text_delta: "ing",
      });
      flushSync();

      expect(texts).toEqual(["", "think", "thinking"]);
    } finally {
      stop();
    }
  });
});
