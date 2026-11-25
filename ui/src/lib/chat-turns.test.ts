import { describe, expect, it } from "vitest";
import { buildChatTurns, turnNeedsAttention } from "./chat-turns.ts";
import type { TimelineItem } from "./sync/reducer.ts";

let seq = 0;

function user(text: string): TimelineItem {
  seq += 1;
  return {
    kind: "user",
    key: `user:${seq}`,
    seq,
    createdAt: seq,
    text,
    attachments: [],
    clientSource: "cli",
  };
}

function tool(): TimelineItem {
  seq += 1;
  return {
    kind: "tool",
    key: `tool:${seq}`,
    seq,
    createdAt: seq,
    toolCallId: `call-${seq}`,
    title: "cargo test",
    toolKind: "execute",
    status: "completed",
    locations: [],
    diff: null,
    rawOutput: null,
    terminalId: null,
    subagentId: null,
  };
}

function permission(resolved: boolean): TimelineItem {
  seq += 1;
  return {
    kind: "permission",
    key: `permission:${seq}`,
    seq,
    createdAt: seq,
    requestId: `request-${seq}`,
    toolName: "edit",
    summary: "Edit login.spec.ts",
    options: [],
    details: null,
    resolved: resolved ? { selectedOptionId: "allow", resolvedBy: "user" } : null,
    subagentId: null,
  };
}

function elicitation(): TimelineItem {
  seq += 1;
  return {
    kind: "elicitation",
    key: `elicitation:${seq}`,
    seq,
    createdAt: seq,
    elicitationId: `question-${seq}`,
    toolCallId: null,
    message: "Which strategy?",
    mode: "form",
    requestedSchema: { type: "object", properties: {} },
    resolved: null,
    subagentId: null,
  };
}

function notice(tone: "info" | "warning" | "error"): TimelineItem {
  seq += 1;
  return { kind: "notice", key: `notice:${seq}`, seq, createdAt: seq, text: "notice", tone };
}

describe("buildChatTurns", () => {
  it("returns nothing without a user message", () => {
    expect(buildChatTurns([tool(), notice("info")])).toEqual([]);
  });

  it("starts a turn at every user message", () => {
    const turns = buildChatTurns([user("first"), tool(), user("second")]);
    expect(turns.map((turn) => turn.title)).toEqual(["first", "second"]);
    expect(turns[0]?.tools).toBe(1);
    expect(turns[1]?.tools).toBe(0);
  });

  it("uses the first non-empty line as the title", () => {
    const turns = buildChatTurns([user("\n\n  fix   the   login \n test\ndetails")]);
    expect(turns[0]?.title).toBe("fix the login");
  });

  it("marks unresolved approvals and questions as attention", () => {
    const turns = buildChatTurns([user("a"), permission(false), elicitation()]);
    expect(turns[0]?.attention).toMatchObject({ approval: true, question: true, error: false });
    expect(turns.some(turnNeedsAttention)).toBe(true);
  });

  it("ignores resolved approvals", () => {
    const turns = buildChatTurns([user("a"), permission(true)]);
    expect(turns.some(turnNeedsAttention)).toBe(false);
  });

  it("marks warnings and errors, not info notices", () => {
    const info = buildChatTurns([user("a"), notice("info")]);
    expect(info[0]?.attention.error).toBe(false);
    const warning = buildChatTurns([user("a"), notice("warning")]);
    expect(warning[0]?.attention.error).toBe(true);
  });

  it("keeps attention on the turn that owns it", () => {
    const turns = buildChatTurns([user("a"), permission(false), user("b")]);
    expect(turns[0]?.attention.approval).toBe(true);
    expect(turns[1]?.attention.approval).toBe(false);
  });
});
