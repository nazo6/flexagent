<script lang="ts">
  import { SESSION_PANELS, type SessionPanelId } from "#lib/session-panels.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { cn } from "#lib/utils.ts";
  import PanelRightCloseIcon from "@lucide/svelte/icons/panel-right-close";

  interface Props {
    active: SessionPanelId | null;
    onselect: (id: SessionPanelId) => void;
    oncollapse: () => void;
  }

  let { active, onselect, oncollapse }: Props = $props();
</script>

<nav
  class="bg-sidebar border-border flex w-11 shrink-0 flex-col items-center border-l py-2"
  aria-label={m.session_tools_title()}
>
  <div class="flex flex-col items-center gap-1">
    {#each SESSION_PANELS as panel (panel.id)}
      {@const Icon = panel.icon}
      <button
        type="button"
        class={cn(
          "flex size-8 items-center justify-center rounded-md transition-colors",
          active === panel.id
            ? "bg-signal-soft text-signal shadow-[inset_2px_0_0_0_var(--signal)]"
            : "text-muted-foreground hover:bg-accent hover:text-foreground",
        )}
        title={panel.label()}
        aria-label={panel.label()}
        aria-pressed={active === panel.id}
        onclick={() => onselect(panel.id)}
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
