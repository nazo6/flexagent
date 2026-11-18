import type { SessionEventEnvelope, StreamDeltaPayload } from "#lib/generated/bindings.ts";
import { ReactiveCell, ReactiveMap } from "#lib/stores/reactive.ts";
import {
  applyStreamDelta,
  clearStreamBuffers,
  insertEventSorted,
  type ThoughtTiming,
  type ToolProgress,
} from "./reducer";

/**
 * Event timeline for a single session (differential sync state).
 *
 * - Persisted events are ordered by `node_seq`; duplicate deliveries (retries of
 *   the same `event_id` / `node_seq`) are skipped harmlessly.
 * - `LiveStreamDelta` entries are merged into buffers and discarded when the
 *   finalized event arrives (docs/03 §3.2).
 * - Derived values do not live here; consumers (components) compose them. This
 *   class holds state only, and every field is backed by a [`ReactiveCell`] /
 *   [`ReactiveMap`]: the ported reducer mutates the held arrays and maps in
 *   place, so the mutation helpers below notify readers explicitly.
 */
export class SessionTimeline {
  readonly sessionId: string;

  readonly #events = new ReactiveCell<SessionEventEnvelope[]>([]);
  readonly #headEvents = new ReactiveCell<SessionEventEnvelope[]>([]);
  readonly #historyLoaded = new ReactiveCell(false);
  readonly #historyLoading = new ReactiveCell(false);
  readonly #hasOlder = new ReactiveCell(false);
  readonly #olderCursor = new ReactiveCell<number | null>(null);
  readonly #headEndSeq = new ReactiveCell(0);
  readonly #streamKeys = new ReactiveCell<string[]>([]);

  readonly messageDeltas = new ReactiveMap<string, string>();
  readonly thoughtDeltas = new ReactiveMap<string, string>();
  readonly subagentByMessage = new ReactiveMap<string, string>();
  readonly subagentByThought = new ReactiveMap<string, string>();
  readonly toolProgress = new ReactiveMap<string, ToolProgress>();
  readonly terminalDeltas = new ReactiveMap<string, string>();
  /**
   * Client-side arrival / completion times of a streamed thought (UI only).
   *
   * Kept after the stream is finalized so the collapsed summary can show how
   * long the agent reasoned.
   */
  readonly thoughtTimes = new ReactiveMap<string, ThoughtTiming>();

  constructor(sessionId: string) {
    this.sessionId = sessionId;
  }

  /** Rendered conversation window (newest page plus pages loaded on demand). */
  get events(): SessionEventEnvelope[] {
    return this.#events.get();
  }

  set events(value: SessionEventEnvelope[]) {
    this.#events.set(value);
  }

  /**
   * Oldest page of the session, kept aside until the reader pages back to it.
   *
   * It supplies session-level metadata (capabilities, provisioning log, ...)
   * while the conversation window only contains the recent tail.
   */
  get headEvents(): SessionEventEnvelope[] {
    return this.#headEvents.get();
  }

  set headEvents(value: SessionEventEnvelope[]) {
    this.#headEvents.set(value);
  }

  /** Whether the REST history window has been loaded for this timeline. */
  get historyLoaded(): boolean {
    return this.#historyLoaded.get();
  }

  set historyLoaded(value: boolean) {
    this.#historyLoaded.set(value);
  }

  /** Whether an older history page is being fetched. */
  get historyLoading(): boolean {
    return this.#historyLoading.get();
  }

  set historyLoading(value: boolean) {
    this.#historyLoading.set(value);
  }

  /** Whether the conversation window still has unloaded older events. */
  get hasOlder(): boolean {
    return this.#hasOlder.get();
  }

  set hasOlder(value: boolean) {
    this.#hasOlder.set(value);
  }

  /** `before_cursor` for the next older page (`null` when there is none). */
  get olderCursor(): number | null {
    return this.#olderCursor.get();
  }

  set olderCursor(value: number | null) {
    this.#olderCursor.set(value);
  }

  /** `node_seq` reached by [`headEvents`] (`0` when the head has not been loaded). */
  get headEndSeq(): number {
    return this.#headEndSeq.get();
  }

  set headEndSeq(value: number) {
    this.#headEndSeq.set(value);
  }

  /** Streaming buffers in the order their first delta arrived (`${kind}:${id}`). */
  get streamKeys(): string[] {
    return this.#streamKeys.get();
  }

  set streamKeys(value: string[]) {
    this.#streamKeys.set(value);
  }

  /** Apply a persisted event to the rendered conversation window (`false` when it is a duplicate). */
  applyEvent(event: SessionEventEnvelope): boolean {
    if (!insertEventSorted(this.events, event)) return false;
    // `insertEventSorted` splices the held array in place, so readers are
    // notified explicitly (the cell cannot observe an in-place mutation).
    this.#events.touch();
    clearStreamBuffers(event, this);
    this.#streamKeys.touch();
    return true;
  }

  /** Apply an event of the oldest page kept aside for session metadata. */
  applyHeadEvent(event: SessionEventEnvelope): void {
    if (!insertEventSorted(this.headEvents, event)) return;
    this.#headEvents.touch();
  }

  /**
   * Events used to derive session-level metadata (capabilities, provisioning
   * log, plan). `headEvents` precedes the rendered window in `node_seq` order.
   */
  metadataEvents(): SessionEventEnvelope[] {
    const head = this.headEvents;
    const events = this.events;
    return head.length === 0 ? events : [...head, ...events];
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
    // `applyStreamDelta` rewrites the buffers in place and pushes to
    // `streamKeys`; the maps notify themselves, the array needs a nudge.
    this.#streamKeys.touch();
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
    this.streamKeys = [];
  }
}
