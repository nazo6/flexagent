import { describe, expect, it } from "vitest";
import type { SessionEventEnvelope } from "$lib/generated/SessionEventEnvelope";
import type { StreamDeltaPayload } from "$lib/generated/StreamDeltaPayload";
import type { UnifiedEventPayload } from "$lib/generated/UnifiedEventPayload";
import { encodeTextToBase64 } from "$lib/base64";
import {
  applyStreamDelta,
  buildTimelineItems,
  capabilitiesFromEvents,
  clearStreamBuffers,
  insertEventSorted,
  type MutableStreamBuffers,
  type PendingPrompt,
  type TimelineSources,
  type ToolProgress,
} from "./reducer";

function makeEvent(seq: number, payload: UnifiedEventPayload): SessionEventEnvelope {
  return {
    event_id: `event-${seq}`,
    session_id: "session-1",
    node_seq: seq,
    created_at: 1_700_000_000_000 + seq,
    payload,
  };
}

function emptySources(events: SessionEventEnvelope[] = []): TimelineSources & MutableStreamBuffers {
  return {
    events,
    messageDeltas: new Map(),
    thoughtDeltas: new Map(),
    toolProgress: new Map(),
    terminalDeltas: new Map(),
  };
}

describe("insertEventSorted", () => {
  it("keeps events ordered by node_seq and skips duplicates", () => {
    const events: SessionEventEnvelope[] = [];
    const first = makeEvent(2, { type: "session_title_changed", data: { title: "b" } });
    const second = makeEvent(1, { type: "session_title_changed", data: { title: "a" } });
    const third = makeEvent(3, { type: "session_title_changed", data: { title: "c" } });

    expect(insertEventSorted(events, first)).toBe(true);
    expect(insertEventSorted(events, second)).toBe(true);
    expect(insertEventSorted(events, third)).toBe(true);
    expect(
      insertEventSorted(
        events,
        makeEvent(2, { type: "session_title_changed", data: { title: "dup" } }),
      ),
    ).toBe(false);
    expect(events.map((event) => event.node_seq)).toEqual([1, 2, 3]);
  });
});

describe("streaming agent messages", () => {
  it("merges deltas and replaces them with the completed event", () => {
    const sources = emptySources();
    const delta1: StreamDeltaPayload = {
      delta_type: "agent_message_delta",
      message_id: "m1",
      text_delta: "Hello ",
    };
    const delta2: StreamDeltaPayload = {
      delta_type: "agent_message_delta",
      message_id: "m1",
      text_delta: "world",
    };
    applyStreamDelta(sources, delta1);
    applyStreamDelta(sources, delta2);

    let items = buildTimelineItems(sources);
    expect(items).toHaveLength(1);
    expect(items[0]).toMatchObject({ kind: "agent", text: "Hello world", streaming: true });

    const complete = makeEvent(1, {
      type: "agent_message",
      data: { message_id: "m1", text: "Hello world!", is_complete: true },
    });
    insertEventSorted(sources.events, complete);
    clearStreamBuffers(complete, sources);

    items = buildTimelineItems(sources);
    expect(items).toHaveLength(1);
    expect(items[0]).toMatchObject({ kind: "agent", text: "Hello world!", streaming: false });
  });
});

describe("tool calls", () => {
  it("coalesces repeated tool_call events into a single item", () => {
    const sources = emptySources([
      makeEvent(1, {
        type: "tool_call",
        data: {
          tool_call_id: "t1",
          title: "cargo test",
          kind: "execute",
          status: "pending",
          locations: [],
          diff: null,
          raw_output: null,
        },
      }),
      makeEvent(2, {
        type: "tool_call",
        data: {
          tool_call_id: "t1",
          title: "cargo test",
          kind: "execute",
          status: "completed",
          locations: [],
          diff: null,
          raw_output: "ok",
        },
      }),
    ]);
    const items = buildTimelineItems(sources);
    expect(items).toHaveLength(1);
    expect(items[0]).toMatchObject({
      kind: "tool",
      toolCallId: "t1",
      status: "completed",
      rawOutput: "ok",
      seq: 1,
    });
  });

  it("preserves title, kind, and locations when partial update arrives with empty fields", () => {
    const sources = emptySources([
      makeEvent(1, {
        type: "tool_call",
        data: {
          tool_call_id: "t2",
          title: "Running client_view_file",
          kind: "read",
          status: "in_progress",
          locations: ["/repo/README.md"],
          diff: null,
          raw_output: null,
        },
      }),
      makeEvent(2, {
        type: "tool_call",
        data: {
          tool_call_id: "t2",
          title: "",
          kind: "",
          status: "completed",
          locations: [],
          diff: null,
          raw_output: "file content",
        },
      }),
    ]);
    const items = buildTimelineItems(sources);
    expect(items).toHaveLength(1);
    expect(items[0]).toMatchObject({
      kind: "tool",
      toolCallId: "t2",
      title: "Running client_view_file",
      toolKind: "read",
      status: "completed",
      locations: ["/repo/README.md"],
      rawOutput: "file content",
    });
  });

  it("clears streamed tool progress when the completed event arrives", () => {
    const sources = emptySources();
    applyStreamDelta(sources, {
      delta_type: "tool_call_progress",
      tool_call_id: "t2",
      status: "in_progress",
      raw_output_delta: "building...",
    });
    expect(buildTimelineItems(sources)).toHaveLength(1);
    expect(buildTimelineItems(sources)[0]).toMatchObject({
      kind: "tool",
      status: "in_progress",
      rawOutput: "building...",
    });

    const complete = makeEvent(1, {
      type: "tool_call",
      data: {
        tool_call_id: "t2",
        title: "cargo build",
        kind: "execute",
        status: "completed",
        locations: [],
        diff: null,
        raw_output: "done",
      },
    });
    insertEventSorted(sources.events, complete);
    clearStreamBuffers(complete, sources);
    const items = buildTimelineItems(sources);
    expect(items).toHaveLength(1);
    expect(items[0]).toMatchObject({ kind: "tool", status: "completed", rawOutput: "done" });
  });
});

describe("terminal output", () => {
  it("merges streamed output and drops the buffer on completion", () => {
    const sources = emptySources();
    applyStreamDelta(sources, {
      delta_type: "terminal_output_delta",
      terminal_id: "term-1",
      data_b64: encodeTextToBase64("line 1\n"),
    });
    applyStreamDelta(sources, {
      delta_type: "terminal_output_delta",
      terminal_id: "term-1",
      data_b64: encodeTextToBase64("line 2\n"),
    });

    const complete = makeEvent(1, {
      type: "terminal_output",
      data: {
        terminal_id: "term-1",
        command: "ls",
        data_b64: encodeTextToBase64("line 1\nline 2\n"),
        exit_code: 0,
      },
    });
    insertEventSorted(sources.events, complete);
    clearStreamBuffers(complete, sources);

    const items = buildTimelineItems(sources);
    expect(items).toHaveLength(1);
    expect(items[0]).toMatchObject({
      kind: "terminal",
      terminalId: "term-1",
      command: "ls",
      output: "line 1\nline 2\n",
      exitCode: 0,
    });
  });

  it("appends persisted terminal output for the same terminal id", () => {
    const sources = emptySources([
      makeEvent(1, {
        type: "terminal_output",
        data: {
          terminal_id: "term-2",
          command: "echo a",
          data_b64: encodeTextToBase64("a\n"),
          exit_code: 0,
        },
      }),
      makeEvent(2, {
        type: "terminal_output",
        data: {
          terminal_id: "term-2",
          command: "echo b",
          data_b64: encodeTextToBase64("b\n"),
          exit_code: 0,
        },
      }),
    ]);
    const items = buildTimelineItems(sources);
    expect(items).toHaveLength(1);
    expect(items[0]).toMatchObject({ kind: "terminal", output: "a\nb\n" });
  });
});

describe("permissions", () => {
  it("merges permission_resolved into the request item", () => {
    const sources = emptySources([
      makeEvent(1, {
        type: "permission_request",
        data: {
          request_id: "req-1",
          tool_name: "terminal/create",
          summary: 'Run "cargo test"',
          options: [{ option_id: "allow_once", name: "Allow", kind: "allow_once" }],
          details: null,
        },
      }),
      makeEvent(2, {
        type: "permission_resolved",
        data: {
          request_id: "req-1",
          selected_option_id: "allow_once",
          resolved_by: "android_push",
        },
      }),
    ]);
    const items = buildTimelineItems(sources);
    expect(items).toHaveLength(1);
    expect(items[0]).toMatchObject({
      kind: "permission",
      requestId: "req-1",
      resolved: { selectedOptionId: "allow_once", resolvedBy: "android_push" },
    });
  });
});

describe("metadata helpers", () => {
  it("extracts the latest capabilities", () => {
    const events = [
      makeEvent(1, {
        type: "capabilities_updated",
        data: {
          current_mode: "ask",
          available_modes: [{ mode_id: "ask", name: "Ask", description: null }],
          available_commands: [],
          config_options: [],
        },
      }),
      makeEvent(2, {
        type: "capabilities_updated",
        data: {
          current_mode: "code",
          available_modes: [
            { mode_id: "ask", name: "Ask", description: null },
            { mode_id: "code", name: "Code", description: null },
          ],
          available_commands: [{ name: "review", description: "レビュー", input_hint: null }],
          config_options: [],
        },
      }),
    ];
    expect(capabilitiesFromEvents(events)).toMatchObject({
      currentMode: "code",
      availableCommands: [{ name: "review" }],
    });
  });
});

describe("pending prompts", () => {
  it("appends pending prompts after persisted items", () => {
    const pending: PendingPrompt = {
      commandId: "cmd-1",
      sessionId: "session-1",
      text: "レビューして",
      createdAt: 1,
    };
    const sources: TimelineSources = {
      ...emptySources([
        makeEvent(1, {
          type: "user_message",
          data: {
            text: "hello",
            attachments: [],
            client_source: "web",
            snapshot_tree_hash: null,
          },
        }),
      ]),
      pendingPrompts: new Map([[pending.commandId, pending]]),
    };
    const items = buildTimelineItems(sources);
    expect(items.map((item) => item.kind)).toEqual(["user", "pending"]);
  });
});

describe("tool progress map typing", () => {
  it("exposes ToolProgress shape for the timeline", () => {
    const progress: ToolProgress = { status: "in_progress", output: "" };
    expect(progress.status).toBe("in_progress");
  });
});
