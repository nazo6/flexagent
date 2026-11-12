<script lang="ts">
  import DiffPane from "#lib/components/diff/DiffPane.svelte";
  import TerminalView from "#lib/components/terminal/TerminalView.svelte";
  import { sessionPanel, type SessionPanelId } from "#lib/session-panels.ts";
  import type { TimelineItem } from "#lib/sync/reducer.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { cn } from "#lib/utils.ts";
  import ChevronLeftIcon from "@lucide/svelte/icons/chevron-left";
  import XIcon from "@lucide/svelte/icons/x";

  interface Props {
    panel: SessionPanelId;
    sessionId: string;
    items: TimelineItem[];
    mobile?: boolean;
    onclose: () => void;
    class?: string;
  }

  let { panel, sessionId, items, mobile = false, onclose, class: className = "" }: Props = $props();

  const definition = $derived(sessionPanel(panel));
  const Icon = $derived(definition.icon);
</script>

<section
  class={cn(
    "bg-card/40 border-border flex h-full min-h-0 flex-col overflow-hidden",
    !mobile && "border-l",
    className,
  )}
  aria-label={definition.label()}
>
  <div class="border-border flex h-12 shrink-0 items-center gap-2 border-b px-3">
    {#if mobile}
      <button
        type="button"
        class="text-muted-foreground hover:bg-accent hover:text-foreground -ml-1 flex size-7 items-center justify-center rounded-md transition-colors"
        onclick={onclose}
        title={m.session_close_panel()}
        aria-label={m.session_close_panel()}
      >
        <ChevronLeftIcon class="size-4" />
      </button>
    {/if}
    <Icon class="text-muted-foreground size-4 shrink-0" />
    <h2 class="min-w-0 flex-1 truncate text-[13px] font-semibold">{definition.label()}</h2>
    {#if !mobile}
      <button
        type="button"
        class="text-muted-foreground hover:bg-accent hover:text-foreground flex size-7 items-center justify-center rounded-md transition-colors"
        onclick={onclose}
        title={m.session_close_panel()}
        aria-label={m.session_close_panel()}
      >
        <XIcon class="size-4" />
      </button>
    {/if}
  </div>

  {#if panel === "diff"}
    <div class="app-scroll min-h-0 flex-1 overflow-y-auto p-3">
      <DiffPane {sessionId} {items} />
    </div>
  {:else}
    <div class="flex min-h-0 flex-1 flex-col overflow-hidden p-2">
      <TerminalView {sessionId} fill />
    </div>
  {/if}
</section>
