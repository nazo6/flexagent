<script lang="ts">
  import * as Popover from "#lib/components/ui/popover/index.ts";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { cn } from "#lib/utils.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { toast } from "svelte-sonner";
  import CopyIcon from "@lucide/svelte/icons/copy";
  import GitForkIcon from "@lucide/svelte/icons/git-fork";
  import MoreHorizontalIcon from "@lucide/svelte/icons/more-horizontal";

  interface Props {
    /** Plain text copied by the Copy action. */
    text: string;
    /** Side of the message where the action bar is anchored. */
    align?: "left" | "right";
    /** `null` hides the Fork action entirely. */
    canFork: boolean | null;
    /** Why Fork is disabled (shown as a tooltip). */
    forkReason?: string | null;
    onfork?: () => void;
  }

  let { text, align = "right", canFork, forkReason = null, onfork }: Props = $props();

  let menuOpen = $state(false);

  const anchorClass = $derived(align === "right" ? "right-0" : "left-0");
  const forkTitle = $derived(
    canFork === true ? m.chat_action_fork() : (forkReason ?? m.chat_action_fork_unavailable()),
  );

  async function copy(): Promise<void> {
    try {
      await navigator.clipboard.writeText(text);
      toast.success(m.chat_action_copied());
    } catch {
      toast.error(m.settings_copy_failed());
    }
  }
</script>

<div
  class={cn(
    "message-actions-desktop absolute -top-3 z-10 items-center gap-0.5 rounded-md border border-border bg-popover p-0.5 shadow-sm",
    anchorClass,
  )}
>
  <Button
    size="icon-xs"
    variant="ghost"
    title={m.chat_action_copy()}
    aria-label={m.chat_action_copy()}
    onclick={() => void copy()}
  >
    <CopyIcon class="size-3.5" />
  </Button>
  {#if canFork !== null}
    <Button
      size="icon-xs"
      variant="ghost"
      title={forkTitle}
      aria-label={forkTitle}
      disabled={!canFork}
      onclick={() => onfork?.()}
    >
      <GitForkIcon class="size-3.5" />
    </Button>
  {/if}
</div>

<div class={cn("message-actions-touch absolute -top-3 z-10", anchorClass)}>
  <Popover.Root bind:open={menuOpen}>
    <Popover.Trigger
      class="text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:ring-ring/40 inline-flex size-6 items-center justify-center rounded-md border border-border bg-popover outline-none transition-colors focus-visible:ring-2"
      title={m.chat_action_actions()}
      aria-label={m.chat_action_actions()}
    >
      <MoreHorizontalIcon class="size-3.5" />
    </Popover.Trigger>
    <Popover.Content align={align === "right" ? "end" : "start"} class="w-52 p-1">
      <button
        type="button"
        class="hover:bg-accent flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-[13px]"
        onclick={async () => {
          await copy();
          menuOpen = false;
        }}
      >
        <CopyIcon class="size-3.5" />
        {m.chat_action_copy()}
      </button>
      {#if canFork !== null}
        <button
          type="button"
          class="hover:bg-accent flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-[13px] disabled:pointer-events-none disabled:opacity-50"
          disabled={!canFork}
          title={forkTitle}
          onclick={() => {
            onfork?.();
            menuOpen = false;
          }}
        >
          <GitForkIcon class="size-3.5" />
          {m.chat_action_fork()}
        </button>
        {#if canFork === false && forkReason}
          <p class="text-muted-foreground px-2 py-1 text-[11px] leading-snug">{forkReason}</p>
        {/if}
      {/if}
    </Popover.Content>
  </Popover.Root>
</div>
