<script lang="ts">
  /**
   * Folded run of intermediate work (tools, reasoning, subagents, terminal
   * output, plans) between two answers.
   *
   * It stays open while the agent is working on the current turn and folds into
   * one summary row once the run is over, unless the reader toggled it (same
   * idiom as `ThoughtBlock` and `ToolCallGroup`).
   */
  import type { Snippet } from "svelte";
  import type { ActivitySection } from "#lib/chat-groups.ts";
  import { m } from "#lib/paraglide/messages.js";
  import ChevronDownIcon from "@lucide/svelte/icons/chevron-down";
  import ChevronRightIcon from "@lucide/svelte/icons/chevron-right";
  import LayersIcon from "@lucide/svelte/icons/layers";

  let {
    section,
    live,
    children,
  }: {
    section: ActivitySection;
    /** Whether this run is the tail of a turn the agent is still working on. */
    live: boolean;
    children: Snippet;
  } = $props();

  let userToggled = $state(false);
  let userIsOpen = $state(false);
  const isOpen = $derived(userToggled ? userIsOpen : live);

  function withCount(label: string, count: number): string {
    return count === 1 ? label : `${label} (${count})`;
  }

  /** One-line summary such as `Tool calls (2) · Thinking process`. */
  const summary = $derived.by(() => {
    const { tools, thoughts, subagents, steps } = section.counts;
    const parts: string[] = [];
    if (tools > 0) parts.push(m.chat_tool_calls({ count: tools }));
    if (thoughts > 0) parts.push(withCount(m.chat_thought_process(), thoughts));
    if (subagents > 0) parts.push(withCount(m.chat_subagent_title(), subagents));
    if (steps > 0) parts.push(m.chat_activity_steps({ count: steps }));
    return parts.join(" · ");
  });

  function toggle(): void {
    userToggled = true;
    userIsOpen = !isOpen;
  }
</script>

<div>
  <button
    type="button"
    class="text-muted-foreground hover:text-foreground flex w-full items-center gap-1.5 text-[12px] select-none"
    aria-expanded={isOpen}
    onclick={toggle}
  >
    {#if isOpen}
      <ChevronDownIcon class="size-3.5 shrink-0" />
    {:else}
      <ChevronRightIcon class="size-3.5 shrink-0" />
    {/if}
    <LayersIcon class="size-3.5 shrink-0" />
    <span class="min-w-0 flex-1 truncate text-left">{summary}</span>
  </button>

  {#if isOpen}
    <div class="border-line mt-1.5 rounded-md border p-2.5">
      {@render children()}
    </div>
  {/if}
</div>
