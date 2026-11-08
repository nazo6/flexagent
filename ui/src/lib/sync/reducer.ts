import type {
  CommandInfo,
  CompactionStatus,
  ConfigOptionInfo,
  ElicitationAction,
  ElicitationSchema,
  FileDiff,
  ModeInfo,
  PermissionOption,
  PlanEntry,
  RawJson,
  SessionEventEnvelope,
  SessionStatus,
  SessionUsage,
  StreamDeltaPayload,
} from "#lib/generated/bindings.ts";
import { decodeBase64ToText } from "#lib/base64.ts";
import { m } from "#lib/paraglide/messages.js";

/** Accumulated tool execution progress while streaming (`tool_call_progress`). */
export interface ToolProgress {
  status: string;
  output: string;
}

/** A prompt that was sent but has no `CommandResult` yet (shown in the pending queue). */
export interface PendingPrompt {
  commandId: string;
  sessionId: string;
  text: string;
  createdAt: number;
}

/** Inputs for timeline construction (reactive sources can be passed as-is). */
export interface TimelineSources {
  /** Target session ID (used to filter `pendingPrompts`). */
  sessionId: string;
  events: SessionEventEnvelope[];
  messageDeltas: ReadonlyMap<string, string>;
  thoughtDeltas: ReadonlyMap<string, string>;
  toolProgress: ReadonlyMap<string, ToolProgress>;
  terminalDeltas: ReadonlyMap<string, string>;
  pendingPrompts?: ReadonlyMap<string, PendingPrompt>;
  /**
   * Streaming buffers in the order their first delta arrived, as
   * `${kind}:${id}` keys. Keeps reasoning and messages interleaved correctly
   * while they are still streaming.
   */
  streamKeys?: readonly string[];
}

/** Streaming buffers that can be rewritten (mutable maps). */
export interface MutableStreamBuffers {
  messageDeltas: Map<string, string>;
  thoughtDeltas: Map<string, string>;
  toolProgress: Map<string, ToolProgress>;
  terminalDeltas: Map<string, string>;
  streamKeys: string[];
}

type StreamKind = "message" | "thought" | "tool" | "terminal";

function streamKey(kind: StreamKind, id: string): string {
  return `${kind}:${id}`;
}

function trackStream(sources: MutableStreamBuffers, key: string): void {
  if (!sources.streamKeys.includes(key)) sources.streamKeys.push(key);
}

function untrackStream(sources: MutableStreamBuffers, key: string): void {
  const index = sources.streamKeys.indexOf(key);
  if (index !== -1) sources.streamKeys.splice(index, 1);
}

/** One entry in the chat timeline. */
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
      details: RawJson;
      resolved: { selectedOptionId: string; resolvedBy: string } | null;
    }
  | {
      kind: "elicitation";
      key: string;
      seq: number;
      createdAt: number;
      elicitationId: string;
      message: string;
      mode: string;
      requestedSchema: ElicitationSchema;
      resolved: { action: ElicitationAction; resolvedBy: string } | null;
    }
  | {
      kind: "plan";
      key: string;
      seq: number;
      createdAt: number;
      entries: PlanEntry[];
    }
  | {
      kind: "notice";
      key: string;
      seq: number;
      createdAt: number;
      text: string;
      tone: "info" | "warning" | "error";
    }
  | {
      kind: "pending";
      key: string;
      seq: number;
      createdAt: number;
      text: string;
    };

/** Virtual `node_seq` inserted after the event list (for streaming entries). */
const STREAM_SEQ = Number.MAX_SAFE_INTEGER - 1_000_000;

/**
 * Insert an event while keeping `node_seq` ascending order.
 *
 * If the same `node_seq` already exists, the event is treated as a duplicate
 * delivery (reconnect replay / overlapping live updates) and `false` is
 * returned.
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
 * Apply a `LiveStreamDelta` to the streaming buffers.
 *
 * Discarding the matching buffer via `clearCompleted` when the completed event
 * arrives replaces the streamed text with the finalized one (docs/03 §3.2).
 */
export function applyStreamDelta(sources: MutableStreamBuffers, delta: StreamDeltaPayload): void {
  switch (delta.delta_type) {
    case "agent_message_delta":
      sources.messageDeltas.set(
        delta.message_id,
        (sources.messageDeltas.get(delta.message_id) ?? "") + delta.text_delta,
      );
      trackStream(sources, streamKey("message", delta.message_id));
      break;
    case "agent_thought_delta":
      sources.thoughtDeltas.set(
        delta.thought_id,
        (sources.thoughtDeltas.get(delta.thought_id) ?? "") + delta.text_delta,
      );
      trackStream(sources, streamKey("thought", delta.thought_id));
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
      trackStream(sources, streamKey("tool", delta.tool_call_id));
      break;
    }
    case "terminal_output_delta":
      sources.terminalDeltas.set(
        delta.terminal_id,
        (sources.terminalDeltas.get(delta.terminal_id) ?? "") + decodeBase64ToText(delta.data_b64),
      );
      trackStream(sources, streamKey("terminal", delta.terminal_id));
      break;
  }
}

/** Discard the streaming buffer matching a finalized event when it arrives. */
export function clearStreamBuffers(
  event: SessionEventEnvelope,
  sources: MutableStreamBuffers,
): void {
  const payload = event.payload;
  switch (payload.type) {
    case "agent_message":
      if (payload.data.is_complete) {
        sources.messageDeltas.delete(payload.data.message_id);
        untrackStream(sources, streamKey("message", payload.data.message_id));
      }
      break;
    case "agent_thought":
      if (payload.data.is_complete) {
        sources.thoughtDeltas.delete(payload.data.thought_id);
        untrackStream(sources, streamKey("thought", payload.data.thought_id));
      }
      break;
    case "tool_call":
      if (payload.data.status === "completed" || payload.data.status === "failed") {
        sources.toolProgress.delete(payload.data.tool_call_id);
        untrackStream(sources, streamKey("tool", payload.data.tool_call_id));
      }
      break;
    case "terminal_output":
      // When the finalized event for command completion (with `exit_code`)
      // arrives, discard the matching streaming buffer to avoid duplicates
      if (payload.data.exit_code !== null) {
        sources.terminalDeltas.delete(payload.data.terminal_id);
        untrackStream(sources, streamKey("terminal", payload.data.terminal_id));
      }
      break;
    default:
      break;
  }
}

/** Retention limit for terminal output text (prevents bloat from abnormally large output). */
const MAX_TERMINAL_TEXT = 200_000;

function appendTerminalText(current: string, addition: string): string {
  const combined = current + addition;
  if (combined.length <= MAX_TERMINAL_TEXT) return combined;
  return `${combined.slice(0, MAX_TERMINAL_TEXT)}\n${m.timeline_output_truncated()}`;
}

/**
 * Build the chat timeline from the persisted event list and streaming buffers.
 *
 * Ordering follows `node_seq`; streaming entries (with no finalized event yet)
 * are appended at the end.
 */
export function buildTimelineItems(sources: TimelineSources): TimelineItem[] {
  const items: TimelineItem[] = [];
  const toolIndex = new Map<string, number>();
  const terminalIndex = new Map<string, number>();
  const permissionIndex = new Map<string, number>();
  const elicitationIndex = new Map<string, number>();

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
            text: m.timeline_permission_resolved({
              id: payload.data.request_id,
              option: payload.data.selected_option_id,
              by: payload.data.resolved_by,
            }),
            tone: "info",
          });
        }
        break;
      }
      case "elicitation_request": {
        elicitationIndex.set(payload.data.elicitation_id, items.length);
        items.push({
          kind: "elicitation",
          key: `elicitation:${payload.data.elicitation_id}`,
          ...base,
          elicitationId: payload.data.elicitation_id,
          message: payload.data.message,
          mode: payload.data.mode,
          requestedSchema: payload.data.requested_schema,
          resolved: null,
        });
        break;
      }
      case "elicitation_resolved": {
        const index = elicitationIndex.get(payload.data.elicitation_id);
        if (index !== undefined) {
          const existing = items[index];
          if (existing.kind === "elicitation") {
            items[index] = {
              ...existing,
              resolved: {
                action: payload.data.action,
                resolvedBy: payload.data.resolved_by,
              },
            };
          }
        } else {
          items.push({
            kind: "notice",
            key: `elicitation-resolved:${payload.data.elicitation_id}`,
            ...base,
            text: m.timeline_elicitation_resolved({
              id: payload.data.elicitation_id,
              action: payload.data.action,
              by: payload.data.resolved_by,
            }),
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
          text: m.timeline_session_reverted({
            seq: payload.data.target_node_seq,
            restored: payload.data.restored_files,
            removed: payload.data.removed_files,
          }),
          tone: "info",
        });
        break;
      case "status_changed":
        if (payload.data.status === "error") {
          items.push({
            kind: "notice",
            key: `status:${event.node_seq}`,
            ...base,
            text: payload.data.error_message ?? m.timeline_session_error(),
            tone: "error",
          });
        }
        break;
      case "turn_ended":
        items.push({
          kind: "notice",
          key: `turn-ended:${event.node_seq}`,
          ...base,
          text: turnStopReasonLabel(payload.data.reason),
          tone: "warning",
        });
        break;
      case "compaction_updated":
        items.push({
          kind: "notice",
          key: `compaction:${event.node_seq}`,
          ...base,
          text: compactionStatusLabel(payload.data.status, payload.data.detail),
          tone: payload.data.status === "failed" ? "error" : "info",
        });
        break;
      default:
        break;
    }
  }

  // ---- Append streaming entries at the end ---------------------------------
  let streamOffset = 0;
  const nextStreamSeq = () => STREAM_SEQ + streamOffset++;

  // Preserve the order in which each stream first received a delta, so a
  // reasoning block and the answer that follows it keep their real order while
  // they are still streaming.
  const streamingOrder = sources.streamKeys ?? [
    ...[...sources.messageDeltas.keys()].map((id) => streamKey("message", id)),
    ...[...sources.thoughtDeltas.keys()].map((id) => streamKey("thought", id)),
    ...[...sources.toolProgress.keys()].map((id) => streamKey("tool", id)),
    ...[...sources.terminalDeltas.keys()].map((id) => streamKey("terminal", id)),
  ];

  for (const key of streamingOrder) {
    const separator = key.indexOf(":");
    const kind = key.slice(0, separator) as StreamKind;
    const id = key.slice(separator + 1);
    switch (kind) {
      case "message": {
        const text = sources.messageDeltas.get(id);
        if (text === undefined) continue;
        items.push({
          kind: "agent",
          key: `agent:${id}:stream`,
          seq: nextStreamSeq(),
          createdAt: items.at(-1)?.createdAt ?? 0,
          text,
          streaming: true,
        });
        break;
      }
      case "thought": {
        const text = sources.thoughtDeltas.get(id);
        if (text === undefined) continue;
        items.push({
          kind: "thought",
          key: `thought:${id}:stream`,
          seq: nextStreamSeq(),
          createdAt: items.at(-1)?.createdAt ?? 0,
          text,
          streaming: true,
        });
        break;
      }
      case "tool": {
        const progress = sources.toolProgress.get(id);
        if (progress === undefined || toolIndex.has(id)) continue;
        items.push({
          kind: "tool",
          key: `tool:${id}`,
          seq: nextStreamSeq(),
          createdAt: items.at(-1)?.createdAt ?? 0,
          toolCallId: id,
          title: id,
          toolKind: "other",
          status: progress.status,
          locations: [],
          diff: null,
          rawOutput: progress.output === "" ? null : progress.output,
        });
        break;
      }
      case "terminal": {
        const output = sources.terminalDeltas.get(id);
        if (output === undefined) continue;
        const existing = terminalIndex.get(id);
        if (existing !== undefined) {
          const item = items[existing];
          if (item.kind === "terminal") {
            items[existing] = {
              ...item,
              output: appendTerminalText(item.output, output),
            };
          }
        } else {
          terminalIndex.set(id, items.length);
          items.push({
            kind: "terminal",
            key: `terminal:${id}`,
            seq: nextStreamSeq(),
            createdAt: items.at(-1)?.createdAt ?? 0,
            terminalId: id,
            command: "",
            output,
            exitCode: null,
          });
        }
        break;
      }
    }
  }
  for (const pending of sources.pendingPrompts?.values() ?? []) {
    // Do not mix prompts sent in other sessions into this timeline
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
// Metadata extraction helpers (session header, control bar, Bootstrap log)
// ----------------------------------------------------------------------

/** Session capabilities (the latest applied `CapabilitiesUpdated`). */
export interface SessionCapabilities {
  currentMode: string | null;
  availableModes: ModeInfo[];
  availableCommands: CommandInfo[];
  configOptions: ConfigOptionInfo[];
}

/** Restore the latest capabilities from the event list (null when absent). */
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

/** Restore the latest `status_changed` from the event list (null when absent). */
export function latestStatusFromEvents(
  events: SessionEventEnvelope[],
): { status: SessionStatus; errorMessage: string | null } | null {
  for (let i = events.length - 1; i >= 0; i -= 1) {
    const payload = events[i].payload;
    if (payload.type === "status_changed") {
      return {
        status: payload.data.status,
        errorMessage: payload.data.error_message,
      };
    }
  }
  return null;
}

/** Restore the latest execution plan from the event list (null when absent). */
export function latestPlanFromEvents(events: SessionEventEnvelope[]): PlanEntry[] | null {
  for (let i = events.length - 1; i >= 0; i -= 1) {
    const payload = events[i].payload;
    if (payload.type === "plan_update") return payload.data.entries;
  }
  return null;
}

/** Restore the latest context usage and accumulated cost from the event list (null when absent). */
export function latestUsageFromEvents(events: SessionEventEnvelope[]): SessionUsage | null {
  for (let i = events.length - 1; i >= 0; i -= 1) {
    const payload = events[i].payload;
    if (payload.type === "usage_updated") {
      return {
        used_tokens: payload.data.used_tokens,
        context_size: payload.data.context_size,
        cost: payload.data.cost,
      };
    }
  }
  return null;
}

/** Convert a `turn_ended` stop reason into a label. */
function turnStopReasonLabel(reason: string): string {
  switch (reason) {
    case "max_tokens":
      return m.timeline_turn_stopped_max_tokens();
    case "max_turn_requests":
      return m.timeline_turn_stopped_max_turn_requests();
    case "refusal":
      return m.timeline_turn_stopped_refusal();
    default:
      return m.timeline_turn_stopped_unknown({ reason });
  }
}

/** Convert a `compaction_updated` progress state into a label. */
function compactionStatusLabel(status: CompactionStatus, detail: string | null): string {
  switch (status) {
    case "started":
      return m.timeline_compaction_started();
    case "completed":
      return m.timeline_compaction_completed();
    case "failed":
      return detail ?? m.timeline_compaction_failed();
  }
}

/**
 * Extract Bootstrap log lines from the event list (`stderr` output when starting
 * a temporary VM).
 */
export function bootstrapLogLines(events: SessionEventEnvelope[], maxLines = 500): string[] {
  const lines: string[] = [];
  for (const event of events) {
    if (event.payload.type === "bootstrap_log") lines.push(event.payload.data.line);
  }
  return lines.length > maxLines ? lines.slice(lines.length - maxLines) : lines;
}
