import type { SessionEventEnvelope } from "$lib/generated/SessionEventEnvelope";
import type { StreamDeltaPayload } from "$lib/generated/StreamDeltaPayload";
import {
  buildTimelineItems,
  capabilitiesFromEvents,
  clearStreamBuffers,
  insertEventSorted,
  applyStreamDelta,
  latestPlanFromEvents,
  latestStatusFromEvents,
  type PendingPrompt,
  type TimelineItem,
  type ToolProgress,
} from "./reducer";

const EMPTY_PENDING: ReadonlyMap<string, PendingPrompt> = new Map();

/**
 * 1セッション分のイベントタイムライン (Svelte 5 Runes による差分同期状態)。
 *
 * - 永続イベントは `node_seq` 昇順を正とし、重複配信 (`event_id` /
 *   `node_seq` の再送) は無害にスキップする。
 * - `LiveStreamDelta` はバッファにマージし、確定イベント到着時に破棄して
 *   置き換える (docs/03 §3.2)。
 */
export class SessionTimeline {
  readonly sessionId: string;
  events = $state<SessionEventEnvelope[]>([]);
  messageDeltas = $state(new Map<string, string>());
  thoughtDeltas = $state(new Map<string, string>());
  toolProgress = $state(new Map<string, ToolProgress>());
  terminalDeltas = $state(new Map<string, string>());

  /**
   * Pending Queue (送信済み・未確定プロンプト) のソース。
   * `SyncStore` が生成直後に接続する。
   */
  pendingPrompts: () => ReadonlyMap<string, PendingPrompt> = () => EMPTY_PENDING;

  constructor(sessionId: string) {
    this.sessionId = sessionId;
  }

  /** チャットタイムライン項目 (イベント + ストリーミングの導出値)。 */
  items: TimelineItem[] = $derived(
    buildTimelineItems({ ...this, pendingPrompts: this.pendingPrompts() }),
  );

  /** エージェントからの最新ステータス (イベント由来)。 */
  status = $derived(latestStatusFromEvents(this.events)?.status ?? null);

  capabilities = $derived(capabilitiesFromEvents(this.events));
  plan = $derived(latestPlanFromEvents(this.events));

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
