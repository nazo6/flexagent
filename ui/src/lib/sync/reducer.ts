import type { CommandInfo } from "$lib/generated/CommandInfo";
import type { ConfigOptionInfo } from "$lib/generated/ConfigOptionInfo";
import type { FileDiff } from "$lib/generated/FileDiff";
import type { ModeInfo } from "$lib/generated/ModeInfo";
import type { PermissionOption } from "$lib/generated/PermissionOption";
import type { PlanEntry } from "$lib/generated/PlanEntry";
import type { SessionEventEnvelope } from "$lib/generated/SessionEventEnvelope";
import type { SessionStatus } from "$lib/generated/SessionStatus";
import type { StreamDeltaPayload } from "$lib/generated/StreamDeltaPayload";
import { decodeBase64ToText } from "$lib/base64";

/** ストリーミング中のツール実行進捗 (`tool_call_progress` の累積)。 */
export interface ToolProgress {
  status: string;
  output: string;
}

/** 送信済みで `CommandResult` 未着のプロンプト (Pending Queue 表示用)。 */
export interface PendingPrompt {
  commandId: string;
  sessionId: string;
  text: string;
  createdAt: number;
}

/** タイムライン構築の入力 (リアクティブなソースをそのまま渡せる)。 */
export interface TimelineSources {
  /** 対象セッションの ID (`pendingPrompts` の絞り込みに使う)。 */
  sessionId: string;
  events: SessionEventEnvelope[];
  messageDeltas: ReadonlyMap<string, string>;
  thoughtDeltas: ReadonlyMap<string, string>;
  toolProgress: ReadonlyMap<string, ToolProgress>;
  terminalDeltas: ReadonlyMap<string, string>;
  pendingPrompts?: ReadonlyMap<string, PendingPrompt>;
}

/** ストリーミングバッファの書換え対象 (可変 Map)。 */
export interface MutableStreamBuffers {
  messageDeltas: Map<string, string>;
  thoughtDeltas: Map<string, string>;
  toolProgress: Map<string, ToolProgress>;
  terminalDeltas: Map<string, string>;
}

/** チャットタイムラインの1項目。 */
export type TimelineItem =
  | {
      kind: "user";
      key: string;
      seq: number;
      createdAt: number;
      text: string;
      clientSource: string;
      snapshotTreeHash: string | null;
    }
  | {
      kind: "agent";
      key: string;
      seq: number;
      createdAt: number;
      text: string;
      streaming: boolean;
    }
  | {
      kind: "thought";
      key: string;
      seq: number;
      createdAt: number;
      text: string;
      streaming: boolean;
    }
  | {
      kind: "tool";
      key: string;
      seq: number;
      createdAt: number;
      toolCallId: string;
      title: string;
      toolKind: string;
      status: string;
      locations: string[];
      diff: FileDiff | null;
      rawOutput: string | null;
    }
  | {
      kind: "terminal";
      key: string;
      seq: number;
      createdAt: number;
      terminalId: string;
      command: string;
      output: string;
      exitCode: number | null;
    }
  | {
      kind: "permission";
      key: string;
      seq: number;
      createdAt: number;
      requestId: string;
      toolName: string;
      summary: string;
      options: PermissionOption[];
      details: unknown;
      resolved: { selectedOptionId: string; resolvedBy: string } | null;
    }
  | { kind: "plan"; key: string; seq: number; createdAt: number; entries: PlanEntry[] }
  | {
      kind: "notice";
      key: string;
      seq: number;
      createdAt: number;
      text: string;
      tone: "info" | "error";
    }
  | { kind: "pending"; key: string; seq: number; createdAt: number; text: string };

/** イベント列の末尾に挿入する仮想 `node_seq` (ストリーミング項目用)。 */
const STREAM_SEQ = Number.MAX_SAFE_INTEGER - 1_000_000;

/**
 * `node_seq` 昇順を保ってイベントを挿入する。
 *
 * 同一 `node_seq` が既に存在する場合は重複配信 (再接続リプレイ /
 * ライブ配信の重なり) とみなし `false` を返す。
 */
export function insertEventSorted(
  events: SessionEventEnvelope[],
  event: SessionEventEnvelope,
): boolean {
  let low = 0;
  let high = events.length;
  while (low < high) {
    const mid = (low + high) >> 1;
    if (events[mid].node_seq < event.node_seq) low = mid + 1;
    else high = mid;
  }
  if (low < events.length && events[low].node_seq === event.node_seq) return false;
  events.splice(low, 0, event);
  return true;
}

/**
 * `LiveStreamDelta` をストリーミングバッファへ適用する。
 *
 * 完成イベントの到着時に `clearCompleted` で対応バッファを破棄することで、
 * 確定表示へ置き換わる (docs/03 §3.2)。
 */
export function applyStreamDelta(sources: MutableStreamBuffers, delta: StreamDeltaPayload): void {
  switch (delta.delta_type) {
    case "agent_message_delta":
      sources.messageDeltas.set(
        delta.message_id,
        (sources.messageDeltas.get(delta.message_id) ?? "") + delta.text_delta,
      );
      break;
    case "agent_thought_delta":
      sources.thoughtDeltas.set(
        delta.thought_id,
        (sources.thoughtDeltas.get(delta.thought_id) ?? "") + delta.text_delta,
      );
      break;
    case "tool_call_progress": {
      const current = sources.toolProgress.get(delta.tool_call_id) ?? {
        status: "in_progress",
        output: "",
      };
      sources.toolProgress.set(delta.tool_call_id, {
        status: delta.status,
        output: current.output + (delta.raw_output_delta ?? ""),
      });
      break;
    }
    case "terminal_output_delta":
      sources.terminalDeltas.set(
        delta.terminal_id,
        (sources.terminalDeltas.get(delta.terminal_id) ?? "") + decodeBase64ToText(delta.data_b64),
      );
      break;
  }
}

/** 確定イベント到着時に対応するストリーミングバッファを破棄する。 */
export function clearStreamBuffers(
  event: SessionEventEnvelope,
  sources: MutableStreamBuffers,
): void {
  const payload = event.payload;
  switch (payload.type) {
    case "agent_message":
      if (payload.data.is_complete) sources.messageDeltas.delete(payload.data.message_id);
      break;
    case "agent_thought":
      if (payload.data.is_complete) sources.thoughtDeltas.delete(payload.data.thought_id);
      break;
    case "tool_call":
      if (payload.data.status === "completed" || payload.data.status === "failed") {
        sources.toolProgress.delete(payload.data.tool_call_id);
      }
      break;
    case "terminal_output":
      // コマンド完了 (`exit_code` あり) の確定イベントが来たら、同じ内容の
      // ストリーミングバッファは破棄して二重表示を防ぐ
      if (payload.data.exit_code !== null) {
        sources.terminalDeltas.delete(payload.data.terminal_id);
      }
      break;
    default:
      break;
  }
}

/** ターミナル出力テキストの保持上限 (異常な大出力での肥大化防止)。 */
const MAX_TERMINAL_TEXT = 200_000;

function appendTerminalText(current: string, addition: string): string {
  const combined = current + addition;
  if (combined.length <= MAX_TERMINAL_TEXT) return combined;
  return `${combined.slice(0, MAX_TERMINAL_TEXT)}\n… (出力が長いため省略)`;
}

/**
 * 永続イベント列 + ストリーミングバッファからチャットタイムラインを構築する。
 *
 * 並び順は `node_seq` を正とし、ストリーミング中の項目 (確定イベント未着) は
 * 末尾に追加される。
 */
export function buildTimelineItems(sources: TimelineSources): TimelineItem[] {
  const items: TimelineItem[] = [];
  const toolIndex = new Map<string, number>();
  const terminalIndex = new Map<string, number>();
  const permissionIndex = new Map<string, number>();

  for (const event of sources.events) {
    const payload = event.payload;
    const base = { seq: event.node_seq, createdAt: event.created_at };
    switch (payload.type) {
      case "user_message":
        items.push({
          kind: "user",
          key: `user:${event.node_seq}`,
          ...base,
          text: payload.data.text,
          clientSource: payload.data.client_source,
          snapshotTreeHash: payload.data.snapshot_tree_hash,
        });
        break;
      case "agent_message":
        items.push({
          kind: "agent",
          key: `agent:${payload.data.message_id}:${event.node_seq}`,
          ...base,
          text: payload.data.text,
          streaming: !payload.data.is_complete,
        });
        break;
      case "agent_thought":
        items.push({
          kind: "thought",
          key: `thought:${payload.data.thought_id}:${event.node_seq}`,
          ...base,
          text: payload.data.text,
          streaming: !payload.data.is_complete,
        });
        break;
      case "tool_call": {
        const existing = toolIndex.get(payload.data.tool_call_id);
        if (existing === undefined) {
          toolIndex.set(payload.data.tool_call_id, items.length);
          items.push({
            kind: "tool",
            key: `tool:${payload.data.tool_call_id}`,
            seq: event.node_seq,
            createdAt: event.created_at,
            toolCallId: payload.data.tool_call_id,
            title: payload.data.title,
            toolKind: payload.data.kind,
            status: payload.data.status,
            locations: payload.data.locations,
            diff: payload.data.diff,
            rawOutput: payload.data.raw_output,
          });
        } else {
          const prev = items[existing];
          if (prev.kind === "tool") {
            items[existing] = {
              ...prev,
              title: payload.data.title !== "" ? payload.data.title : prev.title,
              toolKind: payload.data.kind !== "" ? payload.data.kind : prev.toolKind,
              status: payload.data.status !== "" ? payload.data.status : prev.status,
              locations:
                payload.data.locations.length > 0 ? payload.data.locations : prev.locations,
              diff: payload.data.diff ?? prev.diff,
              rawOutput: payload.data.raw_output ?? prev.rawOutput,
            };
          }
        }
        break;
      }
      case "terminal_output": {
        const previous = terminalIndex.get(payload.data.terminal_id);
        const output = decodeBase64ToText(payload.data.data_b64);
        if (previous === undefined) {
          terminalIndex.set(payload.data.terminal_id, items.length);
          items.push({
            kind: "terminal",
            key: `terminal:${payload.data.terminal_id}`,
            ...base,
            terminalId: payload.data.terminal_id,
            command: payload.data.command,
            output,
            exitCode: payload.data.exit_code,
          });
        } else {
          const existing = items[previous];
          if (existing.kind === "terminal") {
            items[previous] = {
              ...existing,
              output: appendTerminalText(existing.output, output),
              exitCode: payload.data.exit_code ?? existing.exitCode,
            };
          }
        }
        break;
      }
      case "permission_request": {
        permissionIndex.set(payload.data.request_id, items.length);
        items.push({
          kind: "permission",
          key: `permission:${payload.data.request_id}`,
          ...base,
          requestId: payload.data.request_id,
          toolName: payload.data.tool_name,
          summary: payload.data.summary,
          options: payload.data.options,
          details: payload.data.details,
          resolved: null,
        });
        break;
      }
      case "permission_resolved": {
        const index = permissionIndex.get(payload.data.request_id);
        if (index !== undefined) {
          const existing = items[index];
          if (existing.kind === "permission") {
            items[index] = {
              ...existing,
              resolved: {
                selectedOptionId: payload.data.selected_option_id,
                resolvedBy: payload.data.resolved_by,
              },
            };
          }
        } else {
          items.push({
            kind: "notice",
            key: `permission-resolved:${payload.data.request_id}`,
            ...base,
            text: `承認 ${payload.data.request_id} は ${payload.data.resolved_by} により ${payload.data.selected_option_id} で解決済み`,
            tone: "info",
          });
        }
        break;
      }
      case "plan_update":
        items.push({
          kind: "plan",
          key: `plan:${event.node_seq}`,
          ...base,
          entries: payload.data.entries,
        });
        break;
      case "session_reverted":
        items.push({
          kind: "notice",
          key: `revert:${event.node_seq}`,
          ...base,
          text: `Revert: node_seq=${payload.data.target_node_seq} 時点へ復元 (${payload.data.restored_files} ファイル復元 / ${payload.data.removed_files} 削除)`,
          tone: "info",
        });
        break;
      case "status_changed":
        if (payload.data.status === "error") {
          items.push({
            kind: "notice",
            key: `status:${event.node_seq}`,
            ...base,
            text: payload.data.error_message ?? "セッションでエラーが発生しました",
            tone: "error",
          });
        }
        break;
      default:
        break;
    }
  }

  // ---- ストリーミング中の項目を末尾へ追加 ----------------------------------
  let streamOffset = 0;
  const nextStreamSeq = () => STREAM_SEQ + streamOffset++;

  for (const [messageId, text] of sources.messageDeltas) {
    items.push({
      kind: "agent",
      key: `agent:${messageId}:stream`,
      seq: nextStreamSeq(),
      createdAt: items.at(-1)?.createdAt ?? 0,
      text,
      streaming: true,
    });
  }
  for (const [thoughtId, text] of sources.thoughtDeltas) {
    items.push({
      kind: "thought",
      key: `thought:${thoughtId}:stream`,
      seq: nextStreamSeq(),
      createdAt: items.at(-1)?.createdAt ?? 0,
      text,
      streaming: true,
    });
  }
  for (const [toolCallId, progress] of sources.toolProgress) {
    if (toolIndex.has(toolCallId)) continue;
    items.push({
      kind: "tool",
      key: `tool:${toolCallId}`,
      seq: nextStreamSeq(),
      createdAt: items.at(-1)?.createdAt ?? 0,
      toolCallId,
      title: toolCallId,
      toolKind: "other",
      status: progress.status,
      locations: [],
      diff: null,
      rawOutput: progress.output === "" ? null : progress.output,
    });
  }
  for (const [terminalId, output] of sources.terminalDeltas) {
    const existing = terminalIndex.get(terminalId);
    if (existing !== undefined) {
      const item = items[existing];
      if (item.kind === "terminal") {
        items[existing] = {
          ...item,
          output: appendTerminalText(item.output, output),
        };
      }
    } else {
      terminalIndex.set(terminalId, items.length);
      items.push({
        kind: "terminal",
        key: `terminal:${terminalId}`,
        seq: nextStreamSeq(),
        createdAt: items.at(-1)?.createdAt ?? 0,
        terminalId,
        command: "",
        output,
        exitCode: null,
      });
    }
  }
  for (const pending of sources.pendingPrompts?.values() ?? []) {
    // 他セッションで送信中のプロンプトをこのタイムラインに混入させない
    if (pending.sessionId !== sources.sessionId) continue;
    items.push({
      kind: "pending",
      key: `pending:${pending.commandId}`,
      seq: nextStreamSeq(),
      createdAt: pending.createdAt,
      text: pending.text,
    });
  }

  return items;
}

// ----------------------------------------------------------------------
// メタデータ抽出ヘルパー (セッションヘッダ・コントロールバー・Bootstrap ログ)
// ----------------------------------------------------------------------

/** セッションの能力情報 (`CapabilitiesUpdated` の最新適用結果)。 */
export interface SessionCapabilities {
  currentMode: string | null;
  availableModes: ModeInfo[];
  availableCommands: CommandInfo[];
  configOptions: ConfigOptionInfo[];
}

/** イベント列から最新の能力情報を復元する (無ければ null)。 */
export function capabilitiesFromEvents(events: SessionEventEnvelope[]): SessionCapabilities | null {
  let found = false;
  let currentMode: string | null = null;
  let availableModes: ModeInfo[] = [];
  let availableCommands: CommandInfo[] = [];
  let configOptions: ConfigOptionInfo[] = [];

  let hasMode = false;
  let hasModes = false;
  let hasCommands = false;
  let hasOptions = false;

  for (let i = events.length - 1; i >= 0; i -= 1) {
    const payload = events[i].payload;
    if (payload.type === "capabilities_updated") {
      found = true;
      if (!hasMode && payload.data.current_mode !== null) {
        currentMode = payload.data.current_mode;
        hasMode = true;
      }
      if (!hasModes && payload.data.available_modes.length > 0) {
        availableModes = payload.data.available_modes;
        hasModes = true;
      }
      if (!hasCommands && payload.data.available_commands.length > 0) {
        availableCommands = payload.data.available_commands;
        hasCommands = true;
      }
      if (!hasOptions && payload.data.config_options.length > 0) {
        configOptions = payload.data.config_options;
        hasOptions = true;
      }
      if (hasMode && hasModes && hasCommands && hasOptions) {
        break;
      }
    }
  }

  if (!found) return null;
  return {
    currentMode,
    availableModes,
    availableCommands,
    configOptions,
  };
}

/** イベント列から最新の `status_changed` を復元する (無ければ null)。 */
export function latestStatusFromEvents(
  events: SessionEventEnvelope[],
): { status: SessionStatus; errorMessage: string | null } | null {
  for (let i = events.length - 1; i >= 0; i -= 1) {
    const payload = events[i].payload;
    if (payload.type === "status_changed") {
      return { status: payload.data.status, errorMessage: payload.data.error_message };
    }
  }
  return null;
}

/** イベント列から最新の実行計画を復元する (無ければ null)。 */
export function latestPlanFromEvents(events: SessionEventEnvelope[]): PlanEntry[] | null {
  for (let i = events.length - 1; i >= 0; i -= 1) {
    const payload = events[i].payload;
    if (payload.type === "plan_update") return payload.data.entries;
  }
  return null;
}

/** イベント列から Bootstrap ログ行を抽出する (一時VM 起動時の `stderr` 出力)。 */
export function bootstrapLogLines(events: SessionEventEnvelope[], maxLines = 500): string[] {
  const lines: string[] = [];
  for (const event of events) {
    if (event.payload.type === "bootstrap_log") lines.push(event.payload.data.line);
  }
  return lines.length > maxLines ? lines.slice(lines.length - maxLines) : lines;
}
