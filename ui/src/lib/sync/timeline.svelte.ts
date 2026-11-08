import type { SessionEventEnvelope, StreamDeltaPayload } from "#lib/generated/bindings.ts";
import {
  clearStreamBuffers,
  insertEventSorted,
  applyStreamDelta,
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
 */
export class SessionTimeline {
  readonly sessionId: string;
  events = $state<SessionEventEnvelope[]>([]);
  messageDeltas = $state(new Map<string, string>());
  thoughtDeltas = $state(new Map<string, string>());
  toolProgress = $state(new Map<string, ToolProgress>());
  terminalDeltas = $state(new Map<string, string>());

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
    this.toolProgress.clear();
    this.terminalDeltas.clear();
  }
}
