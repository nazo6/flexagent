import { SvelteMap } from "svelte/reactivity";
import type { SessionEventEnvelope, StreamDeltaPayload } from "#lib/generated/bindings.ts";
import {
  clearStreamBuffers,
  insertEventSorted,
  applyStreamDelta,
  type ThoughtTiming,
  type ToolProgress,
} from "./reducer";

/**
 * Event timeline for a single session (differential sync state using Svelte 5 Runes).
 *
 * - Persisted events are ordered by `node_seq`; duplicate deliveries (retries of
 *   the same `event_id` / `node_seq`) are skipped harmlessly.
 * - `LiveStreamDelta` entries are merged into buffers and discarded when the
 *   finalized event arrives (docs/03 §3.2).
 * - `$derived` does not live here; consumers (components) compose it. Derived
 *   values inside stores outlive the view and cause `derived_inert`, so this
 *   class holds state only.
 * - The streaming buffers are [`SvelteMap`], not `$state(new Map())`: Svelte 5
 *   only proxies plain objects and arrays, so mutating a `Map` held in `$state`
 *   does not invalidate readers (streamed text would appear only once the
 *   finalized event arrives).
 */
export class SessionTimeline {
  readonly sessionId: string;
  events = $state<SessionEventEnvelope[]>([]);
  messageDeltas = new SvelteMap<string, string>();
  thoughtDeltas = new SvelteMap<string, string>();
  subagentByMessage = new SvelteMap<string, string>();
  subagentByThought = new SvelteMap<string, string>();
  toolProgress = new SvelteMap<string, ToolProgress>();
  terminalDeltas = new SvelteMap<string, string>();
  /**
   * Client-side arrival / completion times of a streamed thought (UI only).
   *
   * Kept after the stream is finalized so the collapsed summary can show how
   * long the agent reasoned.
   */
  thoughtTimes = new SvelteMap<string, ThoughtTiming>();
  /** Streaming buffers in the order their first delta arrived (`${kind}:${id}`). */
  streamKeys = $state<string[]>([]);

  constructor(sessionId: string) {
    this.sessionId = sessionId;
  }

  /** Apply a persisted event (`false` when it is a duplicate). */
  applyEvent(event: SessionEventEnvelope): boolean {
    if (!insertEventSorted(this.events, event)) return false;
    clearStreamBuffers(event, this);
    return true;
  }

  /** Merge a `LiveStreamDelta`. */
  applyDelta(delta: StreamDeltaPayload): void {
    applyStreamDelta(this, delta);
  }

  /** Discard all streaming buffers (for example on disconnect). */
  clearStreams(): void {
    this.messageDeltas.clear();
    this.thoughtDeltas.clear();
    this.subagentByMessage.clear();
    this.subagentByThought.clear();
    this.toolProgress.clear();
    this.terminalDeltas.clear();
    this.thoughtTimes.clear();
    this.streamKeys.length = 0;
  }
}
