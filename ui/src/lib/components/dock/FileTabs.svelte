<script lang="ts">
  import { basename } from "#lib/dock/paths.ts";
  import type { DockFileTab } from "#lib/stores/dock.svelte.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { cn } from "#lib/utils.ts";
  import FileIcon from "@lucide/svelte/icons/file";
  import XIcon from "@lucide/svelte/icons/x";

  interface Props {
    tabs: DockFileTab[];
    activePath: string | null;
    onactivate: (path: string) => void;
    onclose: (path: string) => void;
  }

  let { tabs, activePath, onactivate, onclose }: Props = $props();

  function handleKeydown(event: KeyboardEvent, index: number): void {
    if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
    event.preventDefault();
    const offset = event.key === "ArrowRight" ? 1 : -1;
    const nextIndex = (index + offset + tabs.length) % tabs.length;
    const next = tabs[nextIndex];
    if (next === undefined) return;
    onactivate(next.path);
    requestAnimationFrame(() => {
      document.getElementById(`dock-file-tab-${nextIndex}`)?.focus();
    });
  }
</script>

{#if tabs.length > 0}
  <div
    role="tablist"
    aria-label={m.dock_files_title()}
    class="scrollbar-thin border-border flex h-8 shrink-0 items-center gap-1 overflow-x-auto border-b px-1.5"
  >
    {#each tabs as tab, index (tab.path)}
      {@const isActive = tab.path === activePath}
      <div
        role="presentation"
        class={cn(
          "group flex h-6 shrink-0 items-center rounded-md border transition-colors",
          isActive
            ? "border-signal/30 bg-signal-soft text-foreground"
            : "border-transparent text-muted-foreground hover:bg-accent/60 hover:text-foreground",
        )}
      >
        <button
          id={`dock-file-tab-${index}`}
          type="button"
          role="tab"
          aria-selected={isActive}
          tabindex={isActive ? 0 : -1}
          class="flex h-full max-w-56 items-center gap-1.5 pr-1 pl-1.5 text-[11px] font-medium outline-none"
          title={tab.path}
          onclick={() => onactivate(tab.path)}
          onkeydown={(event) => handleKeydown(event, index)}
          onmousedown={(event) => {
            if (event.button === 1) event.preventDefault();
          }}
          onauxclick={(event) => {
            if (event.button !== 1) return;
            event.preventDefault();
            event.stopPropagation();
            onclose(tab.path);
          }}
        >
          <FileIcon class="size-3 shrink-0" />
          <span class="truncate">{basename(tab.path)}</span>
        </button>
        <button
          type="button"
          class={cn(
            "mr-0.5 flex size-4 shrink-0 items-center justify-center rounded-sm transition-opacity hover:bg-accent",
            isActive
              ? "opacity-60 hover:opacity-100"
              : "opacity-0 group-hover:opacity-60 group-focus-within:opacity-60",
          )}
          title={m.dock_close_tab()}
          aria-label={m.dock_close_tab()}
          onclick={(event) => {
            event.stopPropagation();
            onclose(tab.path);
          }}
        >
          <XIcon class="size-3" />
        </button>
      </div>
    {/each}
  </div>
{/if}
