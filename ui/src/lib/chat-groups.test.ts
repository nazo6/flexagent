import { describe, expect, it } from "vitest";
import { groupTimelineItems, type ToolItem } from "./chat-groups.ts";
import type { TimelineItem } from "./sync/reducer.ts";

function tool(toolCallId: string): ToolItem {
  return {
    kind: "tool",
    key: `tool:${toolCallId}`,
    seq: 1,
    createdAt: 0,
    toolCallId,
    title: "cargo test",
    toolKind: "execute",
    status: "completed",
    locations: [],
    diff: null,
    rawOutput: null,
    subagentId: null,
  };
}

function question(elicitationId: string, toolCallId: string | null): TimelineItem {
  return {
    kind: "elicitation",
    key: `elicitation:${elicitationId}`,
    seq: 2,
    createdAt: 0,
    elicitationId,
    toolCallId,
    message: "Which strategy?",
    mode: "form",
    requestedSchema: { type: "object", properties: {} },
    resolved: null,
    subagentId: null,
  };
}

function message(text: string, subagentId: string | null = null): TimelineItem {
  return {
    kind: "agent",
    key: `agent:${text}`,
    seq: 3,
    createdAt: 0,
    text,
    streaming: false,
    subagentId,
  };
}

function subagent(childSessionId: string): TimelineItem {
  return {
    kind: "subagent",
    key: `subagent:${childSessionId}`,
    seq: 2,
    createdAt: 0,
    childSessionId,
    parentSessionId: null,
    depth: 1,
    title: "Explore",
    description: null,
    status: "running",
  };
}

describe("groupTimelineItems", () => {
  it("groups consecutive tool calls", () => {
    const groups = groupTimelineItems([tool("t1"), tool("t2"), message("done")]);
    expect(groups).toHaveLength(2);
    expect(groups[0]).toMatchObject({
      kind: "tools",
      tools: [{ toolCallId: "t1" }, { toolCallId: "t2" }],
    });
    expect(groups[1]).toMatchObject({ kind: "single", item: { kind: "agent" } });
  });

  it("hides the tool call that only triggered a question", () => {
    const groups = groupTimelineItems([tool("q1"), question("e1", "q1")]);
    expect(groups).toHaveLength(1);
    expect(groups[0]).toMatchObject({ kind: "single", item: { kind: "elicitation" } });
  });

  it("keeps other tool calls in the group around a question", () => {
    const groups = groupTimelineItems([tool("t1"), tool("q1"), question("e1", "q1"), tool("t2")]);
    expect(groups.map((group) => group.kind)).toEqual(["tools", "single", "tools"]);
    expect(groups[0]).toMatchObject({ kind: "tools", tools: [{ toolCallId: "t1" }] });
    expect(groups[2]).toMatchObject({ kind: "tools", tools: [{ toolCallId: "t2" }] });
  });

  it("collects child items into a subagent block", () => {
    const groups = groupTimelineItems([
      message("parent"),
      subagent("child-1"),
      message("child", "child-1"),
      message("parent again"),
    ]);
    expect(groups.map((group) => group.kind)).toEqual(["single", "subagent", "single"]);
    const group = groups[1];
    if (group.kind !== "subagent") throw new Error("expected subagent group");
    expect(group.subagent.title).toBe("Explore");
    expect(group.items.map((item) => item.kind === "agent" && item.text)).toEqual(["child"]);
  });

  it("keeps the tool call when the question has no linked tool call", () => {
    const groups = groupTimelineItems([tool("q1"), question("e1", null)]);
    expect(groups.map((group) => group.kind)).toEqual(["tools", "single"]);
  });
});
