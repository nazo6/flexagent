<script lang="ts">
  import { DOCK_LAUNCHERS, type DockToolId } from "#lib/dock/registry.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { cn } from "#lib/utils.ts";
  import PanelRightCloseIcon from "@lucide/svelte/icons/panel-right-close";

  interface Props {
    active: DockToolId | null;
    onselect: (id: DockToolId) => void;
    oncollapse: () => void;
  }

  let { active, onselect, oncollapse }: Props = $props();
</script>

<nav
  class="bg-sidebar border-border flex w-11 shrink-0 flex-col items-center border-l py-2"
  aria-label={m.session_tools_title()}
>
  <div class="flex flex-col items-center gap-1">
    {#each DOCK_LAUNCHERS as tool (tool.id)}
      {@const Icon = tool.icon}
      <button
        type="button"
        class={cn(
          "flex size-8 items-center justify-center rounded-md transition-colors",
          active === tool.id
            ? "bg-signal-soft text-signal"
            : "text-muted-foreground hover:bg-accent hover:text-foreground",
        )}
        title={tool.label()}
        aria-label={tool.label()}
        aria-pressed={active === tool.id}
        onclick={() => onselect(tool.id)}
      >
        <Icon class="size-4" />
      </button>
    {/each}
  </div>

  <button
    type="button"
    class="text-muted-foreground hover:bg-accent hover:text-foreground mt-auto flex size-8 items-center justify-center rounded-md transition-colors"
    title={m.session_tools_hide()}
    aria-label={m.session_tools_hide()}
    onclick={oncollapse}
  >
    <PanelRightCloseIcon class="size-4" />
  </button>
</nav>
