<script lang="ts">
  import ChangesTool from "#lib/components/dock/ChangesTool.svelte";
  import DockTabBar from "#lib/components/dock/DockTabBar.svelte";
  import TerminalTool from "#lib/components/dock/TerminalTool.svelte";
  import { dockTool } from "#lib/dock/registry.ts";
  import { dock, type DockContext } from "#lib/stores/dock.svelte.ts";
  import type { TimelineItem } from "#lib/sync/reducer.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { cn } from "#lib/utils.ts";

  interface Props {
    ctx: DockContext;
    items: TimelineItem[];
    mobile?: boolean;
    oncollapse: () => void;
    class?: string;
  }

  let { ctx, items, mobile = false, oncollapse, class: className = "" }: Props = $props();
</script>

<section
  class={cn(
    "bg-card/40 border-border flex h-full min-h-0 flex-col overflow-hidden",
    !mobile && "border-l",
    className,
  )}
  aria-label={m.session_tools_title()}
>
  <DockTabBar
    tabs={dock.tabs}
    activeTabId={dock.activeTabId}
    {mobile}
    onactivate={(id) => dock.activateTab(id)}
    onclose={(id) => dock.closeTab(id)}
    {oncollapse}
  />

  <div class="relative flex min-h-0 flex-1 flex-col overflow-hidden">
    {#each dock.tabs as tab, index (tab.id)}
      {@const isActive = tab.id === dock.activeTabId}
      {@const keepMounted = dockTool(tab.tool).keepMounted}
      <div
        id={`dock-panel-${index}`}
        role="tabpanel"
        aria-labelledby={`dock-tab-${index}`}
        class={cn(
          "min-h-0 flex-1 flex-col overflow-hidden",
          isActive ? "flex" : "hidden",
        )}
      >
        {#if keepMounted || isActive}
          {#if tab.tool === "terminal"}
            <TerminalTool {ctx} active={isActive} />
          {:else if tab.tool === "changes"}
            <ChangesTool {ctx} {items} />
          {/if}
        {/if}
      </div>
    {/each}
  </div>
</section>
