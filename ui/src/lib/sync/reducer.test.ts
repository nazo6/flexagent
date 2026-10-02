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
  latestUsageFromEvents,
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
    sessionId: "session-1",
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

describe("elicitations", () => {
  it("merges elicitation_resolved into the question item", () => {
    const sources = emptySources([
      makeEvent(1, {
        type: "elicitation_request",
        data: {
          elicitation_id: "elic-1",
          message: "どの戦略で進めますか?",
          mode: "form",
          requested_schema: {
            type: "object",
            properties: { strategy: { type: "string", enum: ["a", "b"] } },
            required: ["strategy"],
          },
          tool_call_id: null,
        },
      }),
      makeEvent(2, {
        type: "elicitation_resolved",
        data: {
          elicitation_id: "elic-1",
          action: "accept",
          content: { strategy: "a" },
          resolved_by: "web",
        },
      }),
    ]);
    const items = buildTimelineItems(sources);
    expect(items).toHaveLength(1);
    expect(items[0]).toMatchObject({
      kind: "elicitation",
      elicitationId: "elic-1",
      message: "どの戦略で進めますか?",
      resolved: { action: "accept", resolvedBy: "web" },
    });
  });

  it("renders a notice for an elicitation_resolved without its request (resync)", () => {
    const sources = emptySources([
      makeEvent(1, {
        type: "elicitation_resolved",
        data: {
          elicitation_id: "elic-1",
          action: "cancel",
          content: null,
          resolved_by: "system",
        },
      }),
    ]);
    const items = buildTimelineItems(sources);
    expect(items).toHaveLength(1);
    expect(items[0]).toMatchObject({ kind: "notice", tone: "info" });
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

  it("merges partial capabilities updates without losing previously emitted fields", () => {
    const events = [
      // 1) 初期 capabilities (modes と config_options)
      makeEvent(1, {
        type: "capabilities_updated",
        data: {
          current_mode: "ask",
          available_modes: [
            { mode_id: "ask", name: "Ask", description: null },
            { mode_id: "code", name: "Code", description: null },
          ],
          available_commands: [],
          config_options: [
            {
              key: "model",
              name: "Model",
              current_value: "gpt-4",
              options: ["gpt-4", "gpt-3.5"],
            },
          ],
        },
      }),
      // 2) コマンドのみの部分更新 (他が空配列)
      makeEvent(2, {
        type: "capabilities_updated",
        data: {
          current_mode: null,
          available_modes: [],
          available_commands: [{ name: "review", description: "コードレビュー", input_hint: null }],
          config_options: [],
        },
      }),
      // 3) モード変更のみの部分更新 (他が空配列)
      makeEvent(3, {
        type: "capabilities_updated",
        data: {
          current_mode: "code",
          available_modes: [],
          available_commands: [],
          config_options: [],
        },
      }),
    ];

    const caps = capabilitiesFromEvents(events);
    expect(caps).not.toBeNull();
    expect(caps).toEqual({
      currentMode: "code",
      availableModes: [
        { mode_id: "ask", name: "Ask", description: null },
        { mode_id: "code", name: "Code", description: null },
      ],
      availableCommands: [{ name: "review", description: "コードレビュー", input_hint: null }],
      configOptions: [
        {
          key: "model",
          name: "Model",
          current_value: "gpt-4",
          options: ["gpt-4", "gpt-3.5"],
        },
      ],
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

  it("ignores pending prompts from other sessions", () => {
    const mine: PendingPrompt = {
      commandId: "cmd-1",
      sessionId: "session-1",
      text: "自分のプロンプト",
      createdAt: 1,
    };
    const other: PendingPrompt = {
      commandId: "cmd-2",
      sessionId: "session-2",
      text: "他セッションのプロンプト",
      createdAt: 2,
    };
    const sources: TimelineSources = {
      ...emptySources(),
      pendingPrompts: new Map([
        [mine.commandId, mine],
        [other.commandId, other],
      ]),
    };
    const items = buildTimelineItems(sources);
    expect(items.map((item) => item.kind)).toEqual(["pending"]);
    expect(items[0]).toMatchObject({ key: "pending:cmd-1", text: "自分のプロンプト" });
  });
});

describe("tool progress map typing", () => {
  it("exposes ToolProgress shape for the timeline", () => {
    const progress: ToolProgress = { status: "in_progress", output: "" };
    expect(progress.status).toBe("in_progress");
  });
});

describe("usage and abnormal turn end", () => {
  it("extracts the latest usage update (last wins)", () => {
    const events = [
      makeEvent(1, {
        type: "usage_updated",
        data: { used_tokens: 1000, context_size: 100000, cost: null },
      }),
      makeEvent(2, {
        type: "usage_updated",
        data: {
          used_tokens: 53000,
          context_size: 200000,
          cost: { amount: 0.045, currency: "USD" },
        },
      }),
    ];
    expect(latestUsageFromEvents(events)).toEqual({
      used_tokens: 53000,
      context_size: 200000,
      cost: { amount: 0.045, currency: "USD" },
    });
    expect(latestUsageFromEvents([])).toBeNull();
  });

  it("renders a warning notice with a fallback label for abnormal turn ends", () => {
    const sources = emptySources([
      makeEvent(1, { type: "turn_ended", data: { reason: "max_tokens", message: null } }),
    ]);
    const items = buildTimelineItems(sources);
    expect(items).toHaveLength(1);
    expect(items[0]).toMatchObject({ kind: "notice", tone: "warning" });
    expect(items[0].kind === "notice" ? items[0].text : "").toContain("最大トークン数");
  });

  it("prefers the agent-supplied turn end message", () => {
    const sources = emptySources([
      makeEvent(1, { type: "turn_ended", data: { reason: "refusal", message: "拒否されました" } }),
    ]);
    const items = buildTimelineItems(sources);
    expect(items[0]).toMatchObject({ kind: "notice", text: "拒否されました", tone: "warning" });
  });
});

describe("compaction notices", () => {
  it("renders info notices for started and completed compaction", () => {
    const sources = emptySources([
      makeEvent(1, { type: "compaction_updated", data: { status: "started", detail: null } }),
      makeEvent(2, { type: "compaction_updated", data: { status: "completed", detail: null } }),
    ]);
    const items = buildTimelineItems(sources);
    expect(items).toHaveLength(2);
    expect(items[0]).toMatchObject({ kind: "notice", tone: "info" });
    expect(items[0].kind === "notice" ? items[0].text : "").toContain("開始");
    expect(items[1]).toMatchObject({ kind: "notice", tone: "info" });
    expect(items[1].kind === "notice" ? items[1].text : "").toContain("完了");
  });

  it("renders an error notice with detail for failed compaction", () => {
    const sources = emptySources([
      makeEvent(1, {
        type: "compaction_updated",
        data: { status: "failed", detail: "要約に失敗しました" },
      }),
    ]);
    const items = buildTimelineItems(sources);
    expect(items[0]).toMatchObject({
      kind: "notice",
      tone: "error",
      text: "要約に失敗しました",
    });
  });
});
