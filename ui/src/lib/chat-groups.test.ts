import { describe, expect, it } from "vitest";
import { groupActivitySections, groupTimelineItems, type ToolItem } from "./chat-groups.ts";
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

function thought(text: string): TimelineItem {
  return {
    kind: "thought",
    key: `thought:${text}`,
    seq: 2,
    createdAt: 0,
    text,
    streaming: false,
    startedAt: null,
    endedAt: null,
    subagentId: null,
  };
}

function permission(requestId: string): TimelineItem {
  return {
    kind: "permission",
    key: `permission:${requestId}`,
    seq: 2,
    createdAt: 0,
    requestId,
    toolName: "shell",
    summary: "Run cargo test",
    options: [],
    details: null,
    resolved: null,
    subagentId: null,
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

describe("groupActivitySections", () => {
  it("folds a run of intermediate work into one section", () => {
    const groups = groupTimelineItems([
      thought("looking"),
      tool("t1"),
      tool("t2"),
      subagent("child-1"),
      message("done"),
    ]);
    const entries = groupActivitySections(groups);
    expect(entries.map((entry) => entry.kind)).toEqual(["activity", "single"]);
    const section = entries[0];
    if (section.kind !== "activity") throw new Error("expected activity section");
    expect(section.groups.map((group) => group.kind)).toEqual(["single", "tools", "subagent"]);
    expect(section.counts).toEqual({ tools: 2, thoughts: 1, subagents: 1, steps: 0 });
  });

  it("leaves a single collapsible block on its own row", () => {
    const entries = groupActivitySections(groupTimelineItems([tool("t1"), tool("t2")]));
    expect(entries.map((entry) => entry.kind)).toEqual(["tools"]);
  });

  it("keeps approvals outside the fold", () => {
    const entries = groupActivitySections(
      groupTimelineItems([thought("planning"), tool("t1"), permission("p1"), tool("t2")]),
    );
    expect(entries.map((entry) => entry.kind)).toEqual(["activity", "single", "tools"]);
    expect(entries[1]).toMatchObject({ kind: "single", item: { kind: "permission" } });
  });

  it("ends a run after the fork origin boundary", () => {
    const groups = groupTimelineItems([thought("before"), thought("after"), tool("t1")]);
    const entries = groupActivitySections(groups, groups[0].key);
    expect(entries.map((entry) => entry.kind)).toEqual(["single", "activity"]);
    const section = entries[1];
    if (section.kind !== "activity") throw new Error("expected activity section");
    expect(section.counts).toEqual({ tools: 1, thoughts: 1, subagents: 0, steps: 0 });
  });
});
