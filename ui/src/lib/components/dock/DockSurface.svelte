<script lang="ts">
  import type { Snippet } from "svelte";
  import ChangesTool from "#lib/components/dock/ChangesTool.svelte";
  import FilesTool from "#lib/components/dock/FilesTool.svelte";
  import TerminalTool from "#lib/components/dock/TerminalTool.svelte";
  import { dockTool } from "#lib/dock/registry.ts";
  import { dock, type DockContext } from "#lib/stores/dock.svelte.ts";
  import type { TimelineItem } from "#lib/sync/reducer.ts";
  import { m } from "#lib/paraglide/messages.js";
  import PanelRightCloseIcon from "@lucide/svelte/icons/panel-right-close";

  interface Props {
    ctx: DockContext;
    items: TimelineItem[];
    onclose: () => void;
    /** Sheet containers pass their own close glyph; the split pane keeps the panel icon. */
    closeIcon?: Snippet;
  }

  let { ctx, items, onclose, closeIcon }: Props = $props();

  const active = $derived(dock.activeTool);
  const definition = $derived(active === null ? null : dockTool(active));
</script>

{#if definition !== null}
  {@const Icon = definition.icon}
  <section
    class="bg-card/40 flex h-full min-h-0 flex-col overflow-hidden"
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
        onclick={onclose}
      >
        {#if closeIcon}
          {@render closeIcon()}
        {:else}
          <PanelRightCloseIcon class="size-4" />
        {/if}
      </button>
    </div>

    <div class="relative flex min-h-0 flex-1 flex-col overflow-hidden">
      <div
        class="min-h-0 flex-1 flex-col overflow-hidden"
        class:flex={active === "files"}
        class:hidden={active !== "files"}
      >
        <FilesTool {ctx} />
      </div>
      <div
        class="min-h-0 flex-1 flex-col overflow-hidden"
        class:flex={active === "changes"}
        class:hidden={active !== "changes"}
      >
        <ChangesTool {ctx} {items} />
      </div>
      <div
        class="min-h-0 flex-1 flex-col overflow-hidden"
        class:flex={active === "terminal"}
        class:hidden={active !== "terminal"}
      >
        <TerminalTool {ctx} active={active === "terminal"} />
      </div>
    </div>
  </section>
{/if}
