<script lang="ts">
  /**
   * Reasoning block.
   *
   * It opens by itself while the agent is thinking and folds away once the
   * finalized `agent_thought` event arrives, unless the reader toggled it (same
   * idiom as `ToolCallGroup`). The body scrolls on its own so a long reasoning
   * stream does not push the rest of the conversation around.
   */
  import MarkdownText from "#lib/components/MarkdownText.svelte";
  import { m } from "#lib/paraglide/messages.js";
  import type { TimelineItem } from "#lib/sync/reducer.ts";
  import BrainIcon from "@lucide/svelte/icons/brain";
  import ChevronDownIcon from "@lucide/svelte/icons/chevron-down";
  import ChevronRightIcon from "@lucide/svelte/icons/chevron-right";

  type ThoughtItem = Extract<TimelineItem, { kind: "thought" }>;

  let { item }: { item: ThoughtItem } = $props();

  let userToggled = $state(false);
  let userIsOpen = $state(false);
  const isOpen = $derived(userToggled ? userIsOpen : item.streaming);

  /** Ticks while streaming so the elapsed time in the summary stays current. */
  let now = $state(Date.now());
  $effect(() => {
    if (!item.streaming) return;
    const timer = setInterval(() => (now = Date.now()), 500);
    return () => clearInterval(timer);
  });

  /** Elapsed reasoning time (null when replayed from history without live deltas). */
  const seconds = $derived.by(() => {
    if (item.startedAt === null) return null;
    const end = item.endedAt ?? (item.streaming ? now : item.startedAt);
    return Math.max(0, Math.round((end - item.startedAt) / 1000));
  });
  const duration = $derived.by(() => {
    if (seconds === null) return null;
    if (seconds < 60) return m.chat_thought_seconds({ seconds });
    return m.chat_thought_minutes({ minutes: Math.floor(seconds / 60), seconds: seconds % 60 });
  });

  /** First non-empty line, shown in the collapsed summary. */
  const preview = $derived.by(() => {
    if (isOpen) return null;
    const line = item.text
      .split("\n")
      .find((candidate) => candidate.trim() !== "")
      ?.trim();
    if (line === undefined) return null;
    // The summary is plain text, so drop the inline emphasis markers.
    const plain = line
      .replace(/[*_`#>]/g, "")
      .replace(/^[-+]\s+/, "")
      .trim();
    return plain === "" ? null : plain;
  });

  let body = $state<HTMLDivElement | null>(null);

  // Keep the reasoning body pinned to its own bottom while it streams in.
  $effect(() => {
    const element = body;
    const streaming = item.streaming;
    const text = item.text;
    if (element === null || !streaming || text === "") return;
    element.scrollTop = element.scrollHeight;
  });

  function toggle(): void {
    userToggled = true;
    userIsOpen = !isOpen;
  }
</script>

<div>
  <button
    type="button"
    class="text-muted-foreground hover:text-foreground flex w-full items-center gap-1.5 text-[13px] select-none"
    aria-expanded={isOpen}
    onclick={toggle}
  >
    {#if isOpen}
      <ChevronDownIcon class="size-3.5 shrink-0" />
    {:else}
      <ChevronRightIcon class="size-3.5 shrink-0" />
    {/if}
    <BrainIcon class="size-3.5 shrink-0" />
    <span class="shrink-0 font-medium">
      {item.streaming ? m.chat_thought_streaming() : m.chat_thought_process()}
    </span>
    {#if duration !== null}
      <span class="text-muted-foreground/70 shrink-0 text-[11px] tabular-nums">{duration}</span>
    {/if}
    {#if item.streaming}
      <span class="text-signal shrink-0 animate-pulse">…</span>
    {:else if preview !== null}
      <span class="text-muted-foreground/60 min-w-0 flex-1 truncate text-left">{preview}</span>
    {/if}
  </button>

  {#if isOpen}
    <div
      bind:this={body}
      class="text-muted-foreground border-line mt-1.5 max-h-64 overflow-y-auto border-l-2 pl-3 text-[13px] leading-relaxed"
    >
      <MarkdownText text={item.text} streaming={item.streaming} />
    </div>
  {/if}
</div>
