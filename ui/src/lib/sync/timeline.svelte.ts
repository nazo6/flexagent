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
  /** Rendered conversation window (newest page plus pages loaded on demand). */
  events = $state<SessionEventEnvelope[]>([]);
  /**
   * Oldest page of the session, kept aside until the reader pages back to it.
   *
   * It supplies session-level metadata (capabilities, provisioning log, ...)
   * while the conversation window only contains the recent tail.
   */
  headEvents = $state<SessionEventEnvelope[]>([]);
  /** Whether the REST history window has been loaded for this timeline. */
  historyLoaded = $state(false);
  /** Whether an older history page is being fetched. */
  historyLoading = $state(false);
  /** Whether the conversation window still has unloaded older events. */
  hasOlder = $state(false);
  /** `before_cursor` for the next older page (`null` when there is none). */
  olderCursor = $state<number | null>(null);
  /** `node_seq` reached by [`headEvents`] (`0` when the head has not been loaded). */
  headEndSeq = $state(0);
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

  /** Apply a persisted event to the rendered conversation window (`false` when it is a duplicate). */
  applyEvent(event: SessionEventEnvelope): boolean {
    if (!insertEventSorted(this.events, event)) return false;
    clearStreamBuffers(event, this);
    return true;
  }

  /** Apply an event of the oldest page kept aside for session metadata. */
  applyHeadEvent(event: SessionEventEnvelope): void {
    insertEventSorted(this.headEvents, event);
  }

  /**
   * Events used to derive session-level metadata (capabilities, provisioning
   * log, plan). `headEvents` precedes the rendered window in `node_seq` order.
   */
  metadataEvents(): SessionEventEnvelope[] {
    return this.headEvents.length === 0 ? this.events : [...this.headEvents, ...this.events];
  }

  /**
   * Move the oldest page into the rendered window once paging reached it.
   */
  mergeHeadIntoWindow(): void {
    if (this.headEvents.length === 0) return;
    for (const event of this.headEvents) this.applyEvent(event);
    this.headEvents = [];
  }

  /** Reset the paging window (used when a timeline is reloaded or pruned). */
  resetWindow(): void {
    this.headEvents = [];
    this.headEndSeq = 0;
    this.olderCursor = null;
    this.hasOlder = false;
    this.historyLoaded = false;
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
