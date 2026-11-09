<script lang="ts">
  import { Badge } from "#lib/components/ui/badge/index.ts";
  import { m } from "#lib/paraglide/messages.js";
  import WrenchIcon from "@lucide/svelte/icons/wrench";

  let { lines }: { lines: string[] } = $props();

  let open = $state(true);
  let scrollEl = $state<HTMLPreElement | null>(null);

  // Scroll to the bottom when new lines arrive
  $effect(() => {
    if (lines.length > 0 && scrollEl !== null && open) {
      scrollEl.scrollTop = scrollEl.scrollHeight;
    }
  });
</script>

{#if lines.length > 0}
  <div class="bg-card/60 rounded-lg border">
    <button
      type="button"
      class="flex w-full items-center gap-2 px-3 py-2 text-left text-ui font-medium"
      onclick={() => (open = !open)}
    >
      <WrenchIcon class="size-3.5" />
      Environment Bootstrap Log
      <Badge variant="secondary">
        {lines.length === 1
          ? m.chat_bootstrap_line({ count: lines.length })
          : m.chat_bootstrap_lines({ count: lines.length })}
      </Badge>
      <span class="text-muted-foreground ml-auto">
        {open ? m.chat_collapse() : m.chat_expand()}
      </span>
    </button>
    {#if open}
      <pre
        bind:this={scrollEl}
        class="bg-muted/30 max-h-48 overflow-auto border-t p-2 font-mono text-ui whitespace-pre-wrap">{lines.join(
          "\n",
        )}</pre>
    {/if}
  </div>
{/if}
