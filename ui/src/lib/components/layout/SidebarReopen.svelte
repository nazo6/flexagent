<script lang="ts">
  import ProblemBadge from "#lib/components/ProblemBadge.svelte";
  import { layoutState } from "#lib/stores/layout.svelte.ts";
  import { m } from "#lib/paraglide/messages.js";
  import PanelLeftOpenIcon from "@lucide/svelte/icons/panel-left-open";

  let { attentionCount }: { attentionCount: number } = $props();
</script>

<!-- A floating control reopens the collapsed sidebar without reserving a
     full-height rail. Session pages place this control in their own header
     instead, so only one toggle is ever visible. -->
<div class="pointer-events-none absolute top-1 left-1 z-20 flex flex-col items-center gap-1.5">
  <button
    type="button"
    class="hover:bg-accent text-muted-foreground hover:text-foreground pointer-events-auto relative flex size-7 items-center justify-center rounded-md border border-border bg-background/85 backdrop-blur"
    onclick={() => layoutState.setSidebarCollapsed(false)}
    title={m.shell_show_sidebar()}
    aria-label={m.shell_show_sidebar()}
  >
    <PanelLeftOpenIcon class="size-4" />
    {#if attentionCount > 0}
      <span
        class="bg-destructive text-destructive-foreground absolute -top-1 -right-1 flex h-4 min-w-4 items-center justify-center rounded-full px-1 text-[10px] leading-none font-bold"
      >
        {attentionCount > 99 ? "99+" : attentionCount}
      </span>
    {/if}
  </button>
  <ProblemBadge class="pointer-events-auto size-7 px-0" />
</div>
