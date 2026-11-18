import { For, Show } from "solid-js";
import { basename } from "#lib/dock/paths.ts";
import type { DockFileTab } from "#lib/stores/dock.ts";
import { m } from "#lib/paraglide/messages.js";
import { cn } from "#lib/utils.ts";
import IconFile from "~icons/lucide/file";
import IconX from "~icons/lucide/x";

export interface FileTabsProps {
  tabs: DockFileTab[];
  activePath: string | null;
  onactivate: (path: string) => void;
  onclose: (path: string) => void;
  onpin: (path: string) => void;
}

/** File tab strip of the Files tool (Arrow keys move, double-click pins). */
export default function FileTabs(props: FileTabsProps) {
  const handleKeydown = (event: KeyboardEvent, index: number): void => {
    if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
    event.preventDefault();
    const offset = event.key === "ArrowRight" ? 1 : -1;
    const nextIndex = (index + offset + props.tabs.length) % props.tabs.length;
    const next = props.tabs[nextIndex];
    if (next === undefined) return;
    props.onactivate(next.path);
    requestAnimationFrame(() => {
      document.getElementById(`dock-file-tab-${nextIndex}`)?.focus();
    });
  };

  return (
    <Show when={props.tabs.length > 0}>
      <div
        role="tablist"
        aria-label={m.dock_files_title()}
        class="scrollbar-thin border-border flex h-8 shrink-0 items-center gap-1 overflow-x-auto border-b px-1.5"
      >
        <For each={props.tabs} keyed={(tab) => tab.path}>
          {(tab, index) => {
            const isActive = () => tab().path === props.activePath;
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
                  id={`dock-file-tab-${index()}`}
                  type="button"
                  role="tab"
                  aria-selected={isActive() ? "true" : "false"}
                  tabindex={isActive() ? 0 : -1}
                  class="flex h-full max-w-56 items-center gap-1.5 pr-1 pl-1.5 text-[11px] font-medium outline-none"
                  title={
                    tab().preview ? `${tab().path}\n${m.dock_file_preview_hint()}` : tab().path
                  }
                  onClick={() => props.onactivate(tab().path)}
                  onDblClick={() => props.onpin(tab().path)}
                  onKeyDown={(event) => handleKeydown(event, index())}
                  onMouseDown={(event) => {
                    if (event.button === 1) event.preventDefault();
                  }}
                  onAuxClick={(event) => {
                    if (event.button !== 1) return;
                    event.preventDefault();
                    event.stopPropagation();
                    props.onclose(tab().path);
                  }}
                >
                  <IconFile class="size-3 shrink-0" />
                  <span class={cn("truncate", tab().preview && "italic")}>
                    {basename(tab().path)}
                  </span>
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
                    props.onclose(tab().path);
                  }}
                >
                  <IconX class="size-3" />
                </button>
              </div>
            );
          }}
        </For>
      </div>
    </Show>
  );
}
