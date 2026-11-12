<script lang="ts">
  import type { Snippet } from "svelte";
  import type { SubagentItem } from "#lib/chat-groups.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { Badge } from "#lib/components/ui/badge/index.ts";
  import { Button } from "#lib/components/ui/button/index.ts";
  import BotIcon from "@lucide/svelte/icons/bot";
  import ChevronDownIcon from "@lucide/svelte/icons/chevron-down";
  import ChevronRightIcon from "@lucide/svelte/icons/chevron-right";
  import LoaderCircleIcon from "@lucide/svelte/icons/loader-circle";

  let {
    subagent,
    onopen,
    children,
  }: {
    subagent: SubagentItem;
    onopen?: (childSessionId: string) => void;
    children: Snippet;
  } = $props();

  let open = $state(true);

  const running = $derived(subagent.status === "running");
  const statusLabel = $derived.by(() => {
    switch (subagent.status) {
      case "running":
        return m.chat_subagent_running();
      case "requires_action":
        return m.chat_subagent_requires_action();
      case "completed":
        return m.chat_subagent_completed();
      case "failed":
        return m.chat_subagent_failed();
      case "cancelled":
        return m.chat_subagent_cancelled();
      default:
        return subagent.status;
    }
  });
  const title = $derived(
    subagent.title ?? subagent.description ?? subagent.childSessionId.slice(0, 8),
  );
</script>

<div
  class="border-border bg-card/40 rounded-lg border border-dashed"
  style={`margin-left: ${Math.max(0, subagent.depth - 1) * 0.75}rem`}
>
  <div class="flex items-center gap-2 px-3 py-2">
    <button
      type="button"
      class="hover:bg-accent/40 flex min-w-0 flex-1 items-center gap-2 rounded px-1 py-0.5 text-left"
      onclick={() => (open = !open)}
    >
      {#if open}
        <ChevronDownIcon class="text-muted-foreground size-3.5 shrink-0" />
      {:else}
        <ChevronRightIcon class="text-muted-foreground size-3.5 shrink-0" />
      {/if}
      <BotIcon class="text-signal size-3.5 shrink-0" />
      <span class="min-w-0 truncate text-[13px] font-medium">{title}</span>
      {#if running}
        <LoaderCircleIcon class="text-signal size-3.5 shrink-0 animate-spin" />
      {/if}
      <Badge variant={running ? "secondary" : "outline"}>{statusLabel}</Badge>
    </button>
    {#if onopen}
      <Button
        variant="ghost"
        size="sm"
        onclick={() => onopen(subagent.childSessionId)}
        aria-label={m.chat_subagent_open()}
      >
        {m.chat_subagent_open()}
      </Button>
    {/if}
  </div>
  {#if open}
    <div class="border-border border-t px-3 py-3">
      {@render children()}
    </div>
  {/if}
</div>
