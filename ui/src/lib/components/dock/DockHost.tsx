import { Dynamic } from "@solidjs/web";
import { Show } from "solid-js";
import ChangesTool from "#lib/components/dock/ChangesTool.tsx";
import FilesTool from "#lib/components/dock/FilesTool.tsx";
import TerminalTool from "#lib/components/dock/TerminalTool.tsx";
import { dockTool } from "#lib/dock/registry.ts";
import { dock, type DockContext } from "#lib/stores/dock.ts";
import type { TimelineItem } from "#lib/sync/reducer.ts";
import { m } from "#lib/paraglide/messages.js";
import { cn } from "#lib/utils.ts";
import IconPanelRightClose from "~icons/lucide/panel-right-close";
import IconX from "~icons/lucide/x";

export interface DockHostProps {
  ctx: DockContext;
  items: TimelineItem[];
  mobile?: boolean;
  oncollapse: () => void;
  class?: string;
}

/** The session side pane: one fixed tool at a time inside a shared frame. */
export default function DockHost(props: DockHostProps) {
  const active = () => dock.activeTool;
  const definition = () => {
    const tool = active();
    return tool === null ? null : dockTool(tool);
  };

  return (
    <Show when={definition()}>
      {(current) => (
        <section
          class={cn(
            "bg-card/40 border-border flex h-full min-h-0 flex-col overflow-hidden",
            props.mobile !== true && "border-l",
            props.class ?? "",
          )}
          aria-label={m.session_tools_title()}
        >
          <div class="border-border flex h-11 shrink-0 items-center gap-2 border-b px-2.5">
            <Dynamic component={current().icon} class="text-muted-foreground size-4 shrink-0" />
            <h2 class="min-w-0 flex-1 truncate text-[13px] font-semibold">{current().label()}</h2>
            <button
              type="button"
              class="text-muted-foreground hover:bg-accent hover:text-foreground flex size-7 items-center justify-center rounded-md transition-colors"
              title={m.session_close_panel()}
              aria-label={m.session_close_panel()}
              onClick={props.oncollapse}
            >
              <Show when={props.mobile === true} fallback={<IconPanelRightClose class="size-4" />}>
                <IconX class="size-4" />
              </Show>
            </button>
          </div>

          <div class="relative flex min-h-0 flex-1 flex-col overflow-hidden">
            <div
              class={cn(
                "min-h-0 flex-1 flex-col overflow-hidden",
                active() === "files" ? "flex" : "hidden",
              )}
            >
              <FilesTool ctx={props.ctx} mobile={props.mobile ?? false} />
            </div>
            <div
              class={cn(
                "min-h-0 flex-1 flex-col overflow-hidden",
                active() === "changes" ? "flex" : "hidden",
              )}
            >
              <ChangesTool ctx={props.ctx} items={props.items} />
            </div>
            <div
              class={cn(
                "min-h-0 flex-1 flex-col overflow-hidden",
                active() === "terminal" ? "flex" : "hidden",
              )}
            >
              <TerminalTool ctx={props.ctx} active={active() === "terminal"} />
            </div>
          </div>
        </section>
      )}
    </Show>
  );
}
