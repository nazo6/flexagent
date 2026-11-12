import type { TimelineItem } from "#lib/sync/reducer.ts";

export type ToolItem = Extract<TimelineItem, { kind: "tool" }>;
export type SubagentItem = Extract<TimelineItem, { kind: "subagent" }>;

export type TimelineGroup =
  | { kind: "single"; key: string; item: TimelineItem }
  | { kind: "tools"; key: string; tools: ToolItem[] }
  | { kind: "subagent"; key: string; subagent: SubagentItem; items: TimelineItem[] };

function subagentIdOf(item: TimelineItem): string | null {
  return "subagentId" in item ? item.subagentId : null;
}

interface GroupOptions {
  /**
   * Whether items tagged with `subagentId` are collected into a subagent block.
   * Disabled for the nested timeline inside a subagent block.
   */
  groupSubagents?: boolean;
}

/**
 * Groups consecutive tool calls so the timeline renders them as one card, and
 * collects subagent items into a block at the first item of each child.
 *
 * Tool calls that only exist to trigger a question are omitted: the
 * elicitation card (`item.kind === "elicitation"`) already shows the question,
 * and the linked tool call carries no extra information.
 */
export function groupTimelineItems(
  items: TimelineItem[],
  options: GroupOptions = {},
): TimelineGroup[] {
  const groupSubagents = options.groupSubagents ?? true;
  const questionToolCallIds = new Set(
    items.flatMap((item) =>
      item.kind === "elicitation" && item.toolCallId !== null ? [item.toolCallId] : [],
    ),
  );

  const subagentMeta = new Map<string, SubagentItem>();
  const subagentItems = new Map<string, TimelineItem[]>();
  if (groupSubagents) {
    for (const item of items) {
      if (item.kind === "subagent") {
        subagentMeta.set(item.childSessionId, item);
      } else if (subagentIdOf(item)) {
        const childId = subagentIdOf(item)!;
        const bucket = subagentItems.get(childId) ?? [];
        bucket.push(item);
        subagentItems.set(childId, bucket);
      }
    }
  }

  const emitted = new Set<string>();
  const groups: TimelineGroup[] = [];
  let currentTools: ToolItem[] = [];

  const flushTools = () => {
    if (currentTools.length === 0) return;
    groups.push({
      kind: "tools",
      key: `toolgroup:${currentTools[0].toolCallId}`,
      tools: currentTools,
    });
    currentTools = [];
  };

  const emitSubagent = (childId: string, at: TimelineItem) => {
    if (emitted.has(childId)) return;
    emitted.add(childId);
    const meta =
      subagentMeta.get(childId) ??
      ({
        kind: "subagent",
        key: `subagent:${childId}`,
        seq: at.seq,
        createdAt: at.createdAt,
        childSessionId: childId,
        parentSessionId: null,
        depth: 1,
        title: null,
        description: null,
        status: "unknown",
      } satisfies SubagentItem);
    groups.push({
      kind: "subagent",
      key: `subagentgroup:${childId}`,
      subagent: meta,
      items: subagentItems.get(childId) ?? [],
    });
  };

  for (const item of items) {
    if (groupSubagents && item.kind === "subagent") {
      flushTools();
      emitSubagent(item.childSessionId, item);
      continue;
    }
    if (groupSubagents && subagentIdOf(item)) {
      flushTools();
      emitSubagent(subagentIdOf(item)!, item);
      continue;
    }

    if (item.kind === "tool") {
      if (!questionToolCallIds.has(item.toolCallId)) {
        currentTools.push(item);
      }
      continue;
    }

    flushTools();
    groups.push({ kind: "single", key: item.key, item });
  }

  flushTools();
  return groups;
}
