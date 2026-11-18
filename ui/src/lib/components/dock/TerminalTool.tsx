import { For, Show, createEffect } from "solid-js";
import TerminalView from "#lib/components/terminal/TerminalView.tsx";
import type { DockContext } from "#lib/stores/dock.ts";
import { MAX_TERMINALS, dock } from "#lib/stores/dock.ts";
import { m } from "#lib/paraglide/messages.js";
import { cn } from "#lib/utils.ts";
import IconPlus from "~icons/lucide/plus";
import IconSquareTerminal from "~icons/lucide/square-terminal";
import IconX from "~icons/lucide/x";

export interface TerminalToolProps {
  ctx: DockContext;
  active: boolean;
}

/** Shell tabs of the dock; every tab keeps its PTY alive while it is mounted. */
export default function TerminalTool(props: TerminalToolProps) {
  const terminals = () => dock.terminals;
  const activeTerminal = () => dock.activeTerminal;
  const atCap = () => terminals().length >= MAX_TERMINALS;

  /** Kill hooks registered by each mounted TerminalView (used when a tab closes). */
  const killers = new Map<string, () => void>();

  // Drop hooks for tabs that belong to another workspace.
  createEffect(
    () => terminals().map((terminal) => terminal.id),
    (ids) => {
      const known = new Set(ids);
      for (const id of killers.keys()) {
        if (!known.has(id)) killers.delete(id);
      }
    },
  );

  function close(id: string): void {
    killers.get(id)?.();
    killers.delete(id);
    dock.closeTerminal(id);
  }

  function handleKeydown(event: KeyboardEvent, index: number): void {
    if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
    event.preventDefault();
    const offset = event.key === "ArrowRight" ? 1 : -1;
    const nextIndex = (index + offset + terminals().length) % terminals().length;
    const next = terminals()[nextIndex];
    if (next === undefined) return;
    dock.activateTerminal(next.id);
    requestAnimationFrame(() => {
      document.getElementById(`dock-terminal-tab-${nextIndex}`)?.focus();
    });
  }

  return (
    <div class="flex min-h-0 flex-1 flex-col overflow-hidden">
      <div class="border-border flex h-8 shrink-0 items-center gap-1 border-b px-1.5">
        <div
          role="tablist"
          aria-label={m.dock_terminal_title()}
          class="scrollbar-thin flex min-w-0 flex-1 items-center gap-1 overflow-x-auto"
        >
          <For each={terminals()} keyed={(tab) => tab.id}>
            {(tab, index) => {
              const isActive = () => tab().id === activeTerminal();
              return (
                <div
                  role="presentation"
                  class={cn(
                    "group flex h-6 shrink-0 items-center rounded-md border transition-colors",
                    isActive()
                      ? "border-signal/30 bg-signal-soft text-foreground"
                      : "border-transparent text-muted-foreground hover:bg-accent/60 hover:text-foreground",
                  )}
                >
                  <button
                    id={`dock-terminal-tab-${index()}`}
                    type="button"
                    role="tab"
                    aria-selected={isActive() ? "true" : "false"}
                    tabindex={isActive() ? 0 : -1}
                    class="flex h-full max-w-56 items-center gap-1.5 pr-1 pl-1.5 text-[11px] font-medium outline-none"
                    title={m.dock_terminal_tab({ n: tab().seq })}
                    onClick={() => dock.activateTerminal(tab().id)}
                    onKeyDown={(event) => handleKeydown(event, index())}
                    onMouseDown={(event) => {
                      if (event.button === 1) event.preventDefault();
                    }}
                    onAuxClick={(event) => {
                      if (event.button !== 1) return;
                      event.preventDefault();
                      event.stopPropagation();
                      close(tab().id);
                    }}
                  >
                    <IconSquareTerminal class="size-3 shrink-0" />
                    <span class="truncate">{m.dock_terminal_tab({ n: tab().seq })}</span>
                  </button>
                  <button
                    type="button"
                    class={cn(
                      "mr-0.5 flex size-4 shrink-0 items-center justify-center rounded-sm transition-opacity hover:bg-accent",
                      isActive()
                        ? "opacity-60 hover:opacity-100"
                        : "opacity-0 group-hover:opacity-60 group-focus-within:opacity-60",
                    )}
                    title={m.dock_close_tab()}
                    aria-label={m.dock_close_tab()}
                    onClick={(event) => {
                      event.stopPropagation();
                      close(tab().id);
                    }}
                  >
                    <IconX class="size-3" />
                  </button>
                </div>
              );
            }}
          </For>
        </div>

        <button
          type="button"
          class="text-muted-foreground hover:bg-accent hover:text-foreground flex size-6 shrink-0 items-center justify-center rounded-md transition-colors disabled:opacity-40"
          title={m.dock_terminal_new()}
          aria-label={m.dock_terminal_new()}
          disabled={atCap()}
          onClick={() => dock.openTerminal()}
        >
          <IconPlus class="size-3.5" />
        </button>
      </div>

      <Show
        when={terminals().length > 0}
        fallback={
          <div class="flex flex-1 flex-col items-center justify-center gap-3 p-6 text-center">
            <IconSquareTerminal class="text-muted-foreground size-8" />
            <div class="space-y-1">
              <p class="text-[13px] font-medium">{m.dock_terminal_empty()}</p>
              <p class="text-muted-foreground text-[12px]">{m.dock_terminal_empty_hint()}</p>
            </div>
            <button
              type="button"
              class="bg-primary text-primary-foreground hover:bg-primary/90 rounded-md px-3 py-1.5 text-[12px] font-medium transition-colors"
              onClick={() => dock.openTerminal()}
            >
              {m.dock_terminal_new()}
            </button>
          </div>
        }
      >
        <div class="relative min-h-0 flex-1">
          <For each={terminals()} keyed={(tab) => tab.id}>
            {(tab) => (
              <div
                class={cn(
                  "absolute inset-0 min-h-0 flex-col p-2",
                  tab().id === activeTerminal() && props.active ? "flex" : "hidden",
                )}
              >
                <TerminalView
                  sessionId={props.ctx.sessionId}
                  nodeId={props.ctx.nodeId}
                  path={props.ctx.localPath}
                  terminalId={tab().id}
                  fill
                  active={props.active && tab().id === activeTerminal()}
                  onregister={(kill) => killers.set(tab().id, kill)}
                />
              </div>
            )}
          </For>
        </div>
      </Show>
    </div>
  );
}
