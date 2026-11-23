import type { TimelineItem } from "#lib/sync/reducer.ts";

import { m } from "#lib/paraglide/messages.js";

/** Things inside a turn that still need the reader. */
export interface TurnAttention {
  /** A permission request is still waiting for an answer. */
  approval: boolean;
  /** An elicitation (question) is still waiting for an answer. */
  question: boolean;
  /** The turn contains an error or warning notice. */
  error: boolean;
}

/**
 * One exchange in the conversation: a user message plus everything the agent
 * emitted before the next user message.
 *
 * Turns are the unit of navigation (the rail and the outline), not raw timeline
 * items: tool runs are folded by `ActivityBlock`, so a map of items would be
 * dominated by work the reader never sees.
 */
export interface ChatTurn {
  /** `node_seq` of the user message that starts the turn (the DOM anchor). */
  seq: number;
  /** First non-empty line of the user message, whitespace collapsed. */
  title: string;
  createdAt: number;
  /** Tool calls made before the next user message. */
  tools: number;
  attention: TurnAttention;
}

function firstLine(text: string): string {
  for (const line of text.split("\n")) {
    const trimmed = line.trim();
    if (trimmed !== "") return trimmed.replaceAll(/\s+/g, " ");
  }
  return "";
}

/**
 * Split the timeline into turns at user messages.
 *
 * Items before the first user message (a partially loaded page, provisioning
 * notices) belong to no turn; the rail renders them as a lead-in segment.
 */
export function buildChatTurns(items: readonly TimelineItem[]): ChatTurn[] {
  const turns: ChatTurn[] = [];
  let current: ChatTurn | null = null;

  for (const item of items) {
    if (item.kind === "user") {
      current = {
        seq: item.seq,
        title: firstLine(item.text),
        createdAt: item.createdAt,
        tools: 0,
        attention: { approval: false, question: false, error: false },
      };
      turns.push(current);
      continue;
    }
    if (current === null) continue;

    switch (item.kind) {
      case "tool":
        current.tools += 1;
        break;
      case "permission":
        if (item.resolved === null) current.attention.approval = true;
        break;
      case "elicitation":
        if (item.resolved === null) current.attention.question = true;
        break;
      case "notice":
        if (item.tone !== "info") current.attention.error = true;
        break;
      default:
        break;
    }
  }

  return turns;
}

/** Whether a turn needs the reader (an approval, a question or an error). */
export function turnNeedsAttention(turn: ChatTurn): boolean {
  return turn.attention.approval || turn.attention.question || turn.attention.error;
}

/** Tool count label shared by the rail tooltip and the outline badges. */
export function turnToolLabel(count: number): string {
  return count === 1 ? m.chat_turns_tool({ count }) : m.chat_turns_tools({ count });
}
