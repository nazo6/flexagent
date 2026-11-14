<script lang="ts">
  import TerminalView from "#lib/components/terminal/TerminalView.svelte";
  import type { DockContext } from "#lib/stores/dock.svelte.ts";
  import { MAX_TERMINALS, dock } from "#lib/stores/dock.svelte.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { cn } from "#lib/utils.ts";
  import PlusIcon from "@lucide/svelte/icons/plus";
  import SquareTerminalIcon from "@lucide/svelte/icons/square-terminal";
  import XIcon from "@lucide/svelte/icons/x";

  let { ctx, active }: { ctx: DockContext; active: boolean } = $props();

  const terminals = $derived(dock.terminals);
  const activeTerminal = $derived(dock.activeTerminal);
  const atCap = $derived(terminals.length >= MAX_TERMINALS);

  /** Kill hooks registered by each mounted TerminalView (used when a tab closes). */
  const killers = new Map<string, () => void>();

  // Drop hooks for tabs that belong to another workspace.
  $effect(() => {
    const ids = new Set(terminals.map((terminal) => terminal.id));
    for (const id of killers.keys()) {
      if (!ids.has(id)) killers.delete(id);
    }
  });

  function close(id: string): void {
    killers.get(id)?.();
    killers.delete(id);
    dock.closeTerminal(id);
  }

  function handleKeydown(event: KeyboardEvent, index: number): void {
    if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
    event.preventDefault();
    const offset = event.key === "ArrowRight" ? 1 : -1;
    const nextIndex = (index + offset + terminals.length) % terminals.length;
    const next = terminals[nextIndex];
    if (next === undefined) return;
    dock.activateTerminal(next.id);
    requestAnimationFrame(() => {
      document.getElementById(`dock-terminal-tab-${nextIndex}`)?.focus();
    });
  }
</script>

<div class="flex min-h-0 flex-1 flex-col overflow-hidden">
  <div class="border-border flex h-8 shrink-0 items-center gap-1 border-b px-1.5">
    <div
      role="tablist"
      aria-label={m.dock_terminal_title()}
      class="scrollbar-thin flex min-w-0 flex-1 items-center gap-1 overflow-x-auto"
    >
      {#each terminals as tab, index (tab.id)}
        {@const isActive = tab.id === activeTerminal}
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
            id={`dock-terminal-tab-${index}`}
            type="button"
            role="tab"
            aria-selected={isActive}
            tabindex={isActive ? 0 : -1}
            class="flex h-full max-w-56 items-center gap-1.5 pr-1 pl-1.5 text-[11px] font-medium outline-none"
            title={m.dock_terminal_tab({ n: tab.seq })}
            onclick={() => dock.activateTerminal(tab.id)}
            onkeydown={(event) => handleKeydown(event, index)}
            onmousedown={(event) => {
              if (event.button === 1) event.preventDefault();
            }}
            onauxclick={(event) => {
              if (event.button !== 1) return;
              event.preventDefault();
              event.stopPropagation();
              close(tab.id);
            }}
          >
            <SquareTerminalIcon class="size-3 shrink-0" />
            <span class="truncate">{m.dock_terminal_tab({ n: tab.seq })}</span>
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
              close(tab.id);
            }}
          >
            <XIcon class="size-3" />
          </button>
        </div>
      {/each}
    </div>

    <button
      type="button"
      class="text-muted-foreground hover:bg-accent hover:text-foreground flex size-6 shrink-0 items-center justify-center rounded-md transition-colors disabled:opacity-40"
      title={m.dock_terminal_new()}
      aria-label={m.dock_terminal_new()}
      disabled={atCap}
      onclick={() => dock.openTerminal()}
    >
      <PlusIcon class="size-3.5" />
    </button>
  </div>

  {#if terminals.length === 0}
    <div class="flex flex-1 flex-col items-center justify-center gap-3 p-6 text-center">
      <SquareTerminalIcon class="text-muted-foreground size-8" />
      <div class="space-y-1">
        <p class="text-[13px] font-medium">{m.dock_terminal_empty()}</p>
        <p class="text-muted-foreground text-[12px]">{m.dock_terminal_empty_hint()}</p>
      </div>
      <button
        type="button"
        class="bg-primary text-primary-foreground hover:bg-primary/90 rounded-md px-3 py-1.5 text-[12px] font-medium transition-colors"
        onclick={() => dock.openTerminal()}
      >
        {m.dock_terminal_new()}
      </button>
    </div>
  {:else}
    <div class="relative min-h-0 flex-1">
      {#each terminals as tab (tab.id)}
        <div
          class={cn(
            "absolute inset-0 min-h-0 flex-col p-2",
            tab.id === activeTerminal && active ? "flex" : "hidden",
          )}
        >
          <TerminalView
            sessionId={ctx.sessionId}
            nodeId={ctx.nodeId}
            path={ctx.localPath}
            terminalId={tab.id}
            fill
            active={active && tab.id === activeTerminal}
            onregister={(kill) => killers.set(tab.id, kill)}
          />
        </div>
      {/each}
    </div>
  {/if}
</div>
