import { Dynamic } from "@solidjs/web";
import { For } from "solid-js";
import { DOCK_LAUNCHERS, type DockLauncherId } from "#lib/dock/registry.ts";
import { m } from "#lib/paraglide/messages.js";
import { cn } from "#lib/utils.ts";
import IconPanelRightClose from "~icons/lucide/panel-right-close";

export interface DockRailProps {
  active: DockLauncherId | null;
  onSelect: (id: DockLauncherId) => void;
  onCollapse: () => void;
}

/** Tool rail shown to the right of the session (desktop). */
export default function DockRail(props: DockRailProps) {
  return (
    <nav
      class="bg-sidebar border-border flex w-11 shrink-0 flex-col items-center border-l py-2"
      aria-label={m.session_tools_title()}
    >
      <div class="flex flex-col items-center gap-1">
        <For each={DOCK_LAUNCHERS}>
          {(tool) => (
            <button
              type="button"
              class={cn(
                "flex size-8 items-center justify-center rounded-md transition-colors",
                props.active === tool.id
                  ? "bg-signal-soft text-signal"
                  : "text-muted-foreground hover:bg-accent hover:text-foreground",
              )}
              title={tool.label()}
              aria-label={tool.label()}
              aria-pressed={props.active === tool.id ? "true" : "false"}
              onClick={() => props.onSelect(tool.id)}
            >
              <Dynamic component={tool.icon} class="size-4" />
            </button>
          )}
        </For>
      </div>

      <button
        type="button"
        class="text-muted-foreground hover:bg-accent hover:text-foreground mt-auto flex size-8 items-center justify-center rounded-md transition-colors"
        title={m.session_tools_hide()}
        aria-label={m.session_tools_hide()}
        onClick={() => props.onCollapse()}
      >
        <IconPanelRightClose class="size-4" />
      </button>
    </nav>
  );
}
