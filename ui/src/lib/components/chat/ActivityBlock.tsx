import type { JSX } from "@solidjs/web";
import { createMemo, createSignal } from "solid-js";
import type { ActivitySection } from "#lib/chat-groups.ts";
import { m } from "#lib/paraglide/messages.js";
import IconChevronDown from "~icons/lucide/chevron-down";
import IconChevronRight from "~icons/lucide/chevron-right";
import IconLayers from "~icons/lucide/layers";

export interface ActivityBlockProps {
  section: ActivitySection;
  /** Whether this run is the tail of a turn the agent is still working on. */
  live: boolean;
  children?: JSX.Element;
}

/**
 * Folded run of intermediate work (tools, reasoning, subagents, terminal
 * output, plans) between two answers.
 *
 * It stays open while the agent is working on the current turn and folds into
 * one summary row once the run is over, unless the reader toggled it (same
 * idiom as `ThoughtBlock` and `ToolCallGroup`).
 */
export default function ActivityBlock(props: ActivityBlockProps) {
  const [userToggled, setUserToggled] = createSignal(false);
  const [userIsOpen, setUserIsOpen] = createSignal(false);
  const isOpen = () => (userToggled() ? userIsOpen() : props.live);

  function withCount(label: string, count: number): string {
    return count === 1 ? label : `${label} (${count})`;
  }

  /** One-line summary such as `Tool calls (2) · Thinking process`. */
  const summary = createMemo(() => {
    const { tools, thoughts, subagents, steps } = props.section.counts;
    const parts: string[] = [];
    if (tools > 0) parts.push(m.chat_tool_calls({ count: tools }));
    if (thoughts > 0) parts.push(withCount(m.chat_thought_process(), thoughts));
    if (subagents > 0) parts.push(withCount(m.chat_subagent_title(), subagents));
    if (steps > 0) parts.push(m.chat_activity_steps({ count: steps }));
    return parts.join(" · ");
  });

  function toggle(): void {
    setUserToggled(true);
    setUserIsOpen(!isOpen());
  }

  return (
    <div>
      <button
        type="button"
        class="text-muted-foreground hover:text-foreground flex w-full items-center gap-1.5 text-[12px] select-none"
        aria-expanded={isOpen() ? "true" : "false"}
        onClick={toggle}
      >
        {isOpen() ? (
          <IconChevronDown class="size-3.5 shrink-0" />
        ) : (
          <IconChevronRight class="size-3.5 shrink-0" />
        )}
        <IconLayers class="size-3.5 shrink-0" />
        <span class="min-w-0 flex-1 truncate text-left">{summary()}</span>
      </button>

      {isOpen() ? (
        <div class="border-line mt-1.5 rounded-md border p-2.5">{props.children}</div>
      ) : null}
    </div>
  );
}
