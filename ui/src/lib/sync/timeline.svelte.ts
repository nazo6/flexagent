import type { SessionEventEnvelope } from "$lib/generated/SessionEventEnvelope";
import type { StreamDeltaPayload } from "$lib/generated/StreamDeltaPayload";
import {
  clearStreamBuffers,
  insertEventSorted,
  applyStreamDelta,
  type ToolProgress,
} from "./reducer";

/**
 * 1セッション分のイベントタイムライン (Svelte 5 Runes による差分同期状態)。
 *
 * - 永続イベントは `node_seq` 昇順を正とし、重複配信 (`event_id` /
 *   `node_seq` の再送) は無害にスキップする。
 * - `LiveStreamDelta` はバッファにマージし、確定イベント到着時に破棄して
 *   置き換える (docs/03 §3.2)。
 * - `$derived` はここに置かず、消費側 (コンポーネント) で構成する。
 *   view の寿命より長生きするストア内の派生値は `derived_inert` の原因に
 *   なるため、状態のみを保持する。
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

  /** 永続イベントを適用する (重複なら `false`)。 */
  applyEvent(event: SessionEventEnvelope): boolean {
    if (!insertEventSorted(this.events, event)) return false;
    clearStreamBuffers(event, this);
    return true;
  }

  /** `LiveStreamDelta` をマージする。 */
  applyDelta(delta: StreamDeltaPayload): void {
    applyStreamDelta(this, delta);
  }

  /** ストリーミングバッファを全て破棄する (切断時など)。 */
  clearStreams(): void {
    this.messageDeltas.clear();
    this.thoughtDeltas.clear();
    this.toolProgress.clear();
    this.terminalDeltas.clear();
  }
}
