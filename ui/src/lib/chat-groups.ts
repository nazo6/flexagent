import type { TimelineItem } from "#lib/sync/reducer.ts";

export type ToolItem = Extract<TimelineItem, { kind: "tool" }>;

export type TimelineGroup =
  | { kind: "single"; key: string; item: TimelineItem }
  | { kind: "tools"; key: string; tools: ToolItem[] };

/**
 * Groups consecutive tool calls so the timeline renders them as one card.
 *
 * Tool calls that only exist to trigger a question are omitted: the
 * elicitation card (`item.kind === "elicitation"`) already shows the question,
 * and the linked tool call carries no extra information.
 */
export function groupTimelineItems(items: TimelineItem[]): TimelineGroup[] {
  const questionToolCallIds = new Set(
    items.flatMap((item) =>
      item.kind === "elicitation" && item.toolCallId !== null ? [item.toolCallId] : [],
    ),
  );

  const groups: TimelineGroup[] = [];
  let currentTools: ToolItem[] = [];

  for (const item of items) {
    if (item.kind === "tool") {
      if (!questionToolCallIds.has(item.toolCallId)) {
        currentTools.push(item);
      }
      continue;
    }

    if (currentTools.length > 0) {
      groups.push({
        kind: "tools",
        key: `toolgroup:${currentTools[0].toolCallId}`,
        tools: currentTools,
      });
      currentTools = [];
    }
    groups.push({ kind: "single", key: item.key, item });
  }

  if (currentTools.length > 0) {
    groups.push({
      kind: "tools",
      key: `toolgroup:${currentTools[0].toolCallId}`,
      tools: currentTools,
    });
  }

  return groups;
}
