<script lang="ts">
  import ChangesTool from "#lib/components/dock/ChangesTool.svelte";
  import FilesTool from "#lib/components/dock/FilesTool.svelte";
  import TerminalTool from "#lib/components/dock/TerminalTool.svelte";
  import { dockTool } from "#lib/dock/registry.ts";
  import { dock, type DockContext } from "#lib/stores/dock.svelte.ts";
  import type { TimelineItem } from "#lib/sync/reducer.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { cn } from "#lib/utils.ts";
  import PanelRightCloseIcon from "@lucide/svelte/icons/panel-right-close";
  import XIcon from "@lucide/svelte/icons/x";

  interface Props {
    ctx: DockContext;
    items: TimelineItem[];
    mobile?: boolean;
    oncollapse: () => void;
    class?: string;
  }

  let { ctx, items, mobile = false, oncollapse, class: className = "" }: Props = $props();

  const active = $derived(dock.activeTool);
  const definition = $derived(active === null ? null : dockTool(active));
</script>

{#if definition !== null}
  {@const Icon = definition.icon}
  <section
    class={cn(
      "bg-card/40 border-border flex h-full min-h-0 flex-col overflow-hidden",
      !mobile && "border-l",
      className,
    )}
    aria-label={m.session_tools_title()}
  >
    <div class="border-border flex h-11 shrink-0 items-center gap-2 border-b px-2.5">
      <Icon class="text-muted-foreground size-4 shrink-0" />
      <h2 class="min-w-0 flex-1 truncate text-[13px] font-semibold">{definition.label()}</h2>
      <button
        type="button"
        class="text-muted-foreground hover:bg-accent hover:text-foreground flex size-7 items-center justify-center rounded-md transition-colors"
        title={m.session_close_panel()}
        aria-label={m.session_close_panel()}
        onclick={oncollapse}
      >
        {#if mobile}
          <XIcon class="size-4" />
        {:else}
          <PanelRightCloseIcon class="size-4" />
        {/if}
      </button>
    </div>

    <div class="relative flex min-h-0 flex-1 flex-col overflow-hidden">
      <div
        class={cn(
          "min-h-0 flex-1 flex-col overflow-hidden",
          active === "files" ? "flex" : "hidden",
        )}
      >
        <FilesTool {ctx} {mobile} />
      </div>
      <div
        class={cn(
          "min-h-0 flex-1 flex-col overflow-hidden",
          active === "changes" ? "flex" : "hidden",
        )}
      >
        <ChangesTool {ctx} {items} />
      </div>
      <div
        class={cn(
          "min-h-0 flex-1 flex-col overflow-hidden",
          active === "terminal" ? "flex" : "hidden",
        )}
      >
        <TerminalTool {ctx} active={active === "terminal"} />
      </div>
    </div>
  </section>
{/if}
