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

function isAutoApprovedPermission(item: TimelineItem): boolean {
  return item.kind === "permission" && item.resolved?.resolvedBy === "auto";
}

interface GroupOptions {
  /**
   * Whether items tagged with `subagentId` are collected into a subagent block.
   * Disabled for the nested timeline inside a subagent block.
   */
  groupSubagents?: boolean;
}

/** Work behind an answer that the timeline folds away once the turn is over. */
export interface ActivityCounts {
  tools: number;
  thoughts: number;
  subagents: number;
  /** Terminal output, plans and subagent messages. */
  steps: number;
}

export interface ActivitySection {
  kind: "activity";
  key: string;
  /** The folded groups, in timeline order. */
  groups: TimelineGroup[];
  counts: ActivityCounts;
}

/** One row of the rendered timeline: a single group or a folded activity run. */
export type TimelineEntry = TimelineGroup | ActivitySection;

/**
 * Whether a group is intermediate work rather than part of the answer. A run of
 * these is folded into one summary row; messages, approvals and notices stay
 * visible so the answer and anything that needs the reader remain in place.
 */
function isActivity(group: TimelineGroup): boolean {
  switch (group.kind) {
    case "tools":
    case "subagent":
      return true;
    case "single":
      switch (group.item.kind) {
        case "thought":
        case "terminal":
        case "plan":
        case "subagent_message":
          return true;
        default:
          return false;
      }
  }
}

/**
 * Collapses each run of activity groups into one section so a finished turn
 * reads as its answer plus a single "what the agent did" row.
 *
 * A run of one group is left alone: tool cards, reasoning blocks and subagent
 * blocks already collapse on their own, so wrapping them would only add a
 * duplicate header. `splitAfterKey` ends a run after that group so the fork
 * origin marker between copied history and live work stays outside the fold.
 */
export function groupActivitySections(
  groups: TimelineGroup[],
  splitAfterKey: string | null = null,
): TimelineEntry[] {
  const entries: TimelineEntry[] = [];
  let run: TimelineGroup[] = [];
  let counts: ActivityCounts = { tools: 0, thoughts: 0, subagents: 0, steps: 0 };

  const flush = () => {
    if (run.length === 0) return;
    if (run.length === 1) {
      entries.push(run[0]);
    } else {
      entries.push({ kind: "activity", key: `activity:${run[0].key}`, groups: run, counts });
    }
    run = [];
    counts = { tools: 0, thoughts: 0, subagents: 0, steps: 0 };
  };

  for (const group of groups) {
    if (!isActivity(group)) {
      flush();
      entries.push(group);
      continue;
    }

    run.push(group);
    if (group.kind === "tools") {
      counts.tools += group.tools.length;
    } else if (group.kind === "subagent") {
      counts.subagents += 1;
    } else if (group.item.kind === "thought") {
      counts.thoughts += 1;
    } else {
      counts.steps += 1;
    }

    if (group.key === splitAfterKey) flush();
  }

  flush();
  return entries;
}

/**
 * Groups consecutive tool calls so the timeline renders them as one card, and
 * collects subagent items into a block at the first item of each child.
 *
 * Tool calls that only exist to trigger a question are omitted: the
 * elicitation card (`item.kind === "elicitation"`) already shows the question,
 * and the linked tool call carries no extra information. Auto-approved
 * permission requests are likewise omitted so they do not duplicate the tool
 * rows or split consecutive tool calls when auto-approve is enabled.
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
      if (isAutoApprovedPermission(item)) continue;
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
    if (isAutoApprovedPermission(item)) {
      continue;
    }
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
