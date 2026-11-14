<script lang="ts">
  import * as Sheet from "#lib/components/ui/sheet/index.ts";
  import { DOCK_LAUNCHERS, type DockLauncherId } from "#lib/dock/registry.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { cn } from "#lib/utils.ts";

  interface Props {
    open?: boolean;
    active: DockLauncherId | null;
    onselect: (id: DockLauncherId) => void;
  }

  let { open = $bindable(false), active, onselect }: Props = $props();
</script>

<Sheet.Root bind:open>
  <Sheet.Content side="bottom" class="p-0">
    <Sheet.Header>
      <Sheet.Title>{m.session_tools_title()}</Sheet.Title>
      <Sheet.Description>{m.session_tools_description()}</Sheet.Description>
    </Sheet.Header>

    <div class="flex flex-col gap-1 px-3 pb-4">
      {#each DOCK_LAUNCHERS as tool (tool.id)}
        {@const Icon = tool.icon}
        <button
          type="button"
          class={cn(
            "flex items-center gap-3 rounded-lg border px-3 py-2.5 text-left text-[13px] transition-colors",
            active === tool.id
              ? "border-signal/30 bg-signal-soft text-foreground"
              : "border-border bg-card hover:bg-accent",
          )}
          aria-pressed={active === tool.id}
          onclick={() => {
            onselect(tool.id);
            open = false;
          }}
        >
          <Icon class="size-4 shrink-0" />
          <span class="min-w-0 flex-1 truncate">{tool.label()}</span>
        </button>
      {/each}
    </div>
  </Sheet.Content>
</Sheet.Root>
