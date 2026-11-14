<script lang="ts">
  import { dockTool } from "#lib/dock/registry.ts";
  import type { DockTab } from "#lib/stores/dock.svelte.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { cn } from "#lib/utils.ts";
  import PanelRightCloseIcon from "@lucide/svelte/icons/panel-right-close";
  import PlusIcon from "@lucide/svelte/icons/plus";
  import XIcon from "@lucide/svelte/icons/x";

  interface Props {
    tabs: DockTab[];
    activeTabId: string | null;
    mobile?: boolean;
    onactivate: (id: string) => void;
    onclose: (id: string) => void;
    oncollapse: () => void;
    /** Opens the tool picker (mobile only). */
    ontools: () => void;
  }

  let {
    tabs,
    activeTabId,
    mobile = false,
    onactivate,
    onclose,
    oncollapse,
    ontools,
  }: Props = $props();

  function selectRelative(index: number, offset: number): void {
    const nextIndex = (index + offset + tabs.length) % tabs.length;
    const next = tabs[nextIndex];
    if (next === undefined) return;
    onactivate(next.id);
    requestAnimationFrame(() => {
      document.getElementById(`dock-tab-${nextIndex}`)?.focus();
    });
  }

  function handleKeydown(event: KeyboardEvent, index: number): void {
    if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
    event.preventDefault();
    selectRelative(index, event.key === "ArrowRight" ? 1 : -1);
  }
</script>

<div class="border-border flex h-11 shrink-0 items-center gap-1 border-b px-1.5">
  <div
    role="tablist"
    aria-label={m.session_tools_title()}
    class="scrollbar-thin flex h-full min-w-0 flex-1 items-center gap-1 overflow-x-auto py-1"
  >
    {#each tabs as tab, index (tab.id)}
      {@const definition = dockTool(tab.tool)}
      {@const Icon = definition.icon}
      {@const isActive = tab.id === activeTabId}
      <div
        role="presentation"
        class={cn(
          "group flex h-7 shrink-0 items-center rounded-md border transition-colors",
          isActive
            ? "border-signal/30 bg-signal-soft text-foreground"
            : "border-transparent text-muted-foreground hover:bg-accent/60 hover:text-foreground",
        )}
      >
        <button
          id={`dock-tab-${index}`}
          type="button"
          role="tab"
          aria-selected={isActive}
          aria-controls={`dock-panel-${index}`}
          tabindex={isActive ? 0 : -1}
          class="flex h-full items-center gap-1.5 pr-1 pl-2 text-[12px] font-medium outline-none"
          onclick={() => onactivate(tab.id)}
          onkeydown={(event) => handleKeydown(event, index)}
        >
          <Icon class="size-3.5 shrink-0" />
          <span class="max-w-40 truncate">{definition.label()}</span>
        </button>
        <button
          type="button"
          class={cn(
            "mr-1 flex size-4 shrink-0 items-center justify-center rounded-sm transition-opacity hover:bg-accent",
            isActive
              ? "opacity-60 hover:opacity-100"
              : "opacity-0 group-hover:opacity-60 group-focus-within:opacity-60",
          )}
          title={m.dock_close_tab()}
          aria-label={m.dock_close_tab()}
          onclick={(event) => {
            event.stopPropagation();
            onclose(tab.id);
          }}
        >
          <XIcon class="size-3" />
        </button>
      </div>
    {/each}
  </div>

  {#if mobile}
    <button
      type="button"
      class="text-muted-foreground hover:bg-accent hover:text-foreground flex size-7 shrink-0 items-center justify-center rounded-md transition-colors"
      title={m.session_tools_title()}
      aria-label={m.session_tools_title()}
      onclick={ontools}
    >
      <PlusIcon class="size-3.5" />
    </button>
  {/if}

  <button
    type="button"
    class="text-muted-foreground hover:bg-accent hover:text-foreground flex size-7 shrink-0 items-center justify-center rounded-md transition-colors"
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
