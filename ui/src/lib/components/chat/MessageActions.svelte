<script lang="ts">
  import { Button } from "#lib/components/ui/button/index.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { toast } from "svelte-sonner";
  import CopyIcon from "@lucide/svelte/icons/copy";
  import GitForkIcon from "@lucide/svelte/icons/git-fork";

  interface Props {
    /** Plain text copied by the Copy action. */
    text: string;
    /** `null` hides the Fork action entirely. */
    canFork: boolean | null;
    /** Why Fork is disabled (shown as a tooltip). */
    forkReason?: string | null;
    /** Keep the actions visible even on pointer devices (used for the latest message). */
    prominent?: boolean;
    /** Metadata shown at the right end of the boundary row. */
    meta?: string | null;
    /** Full value shown when hovering the metadata (for example a full date). */
    metaTitle?: string | null;
    onfork?: () => void;
  }

  let {
    text,
    canFork,
    forkReason = null,
    prominent = false,
    meta = null,
    metaTitle = null,
    onfork,
  }: Props = $props();

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

<!--
  The action row lives in the gap between messages, on the message boundary.
  It is always present on touch screens and fades in on hover / focus on
  pointer devices that can hover.
-->
<div
  class="message-actions absolute inset-x-0 -bottom-9 z-10 flex h-9 items-center gap-2"
  class:message-actions-prominent={prominent}
>
  <span class="message-actions-content bg-border/60 h-px flex-1"></span>
  <div
    class="message-actions-content bg-popover border-border flex items-center gap-0.5 rounded-md border p-0 shadow-sm"
  >
    <Button
      size="xs"
      variant="ghost"
      class="gap-1 px-2"
      title={m.chat_action_copy()}
      aria-label={m.chat_action_copy()}
      onclick={() => void copy()}
    >
      <CopyIcon class="size-3" />
      {m.chat_action_copy()}
    </Button>
    {#if canFork !== null}
      <Button
        size="xs"
        variant="ghost"
        class="gap-1 px-2"
        title={forkTitle}
        aria-label={forkTitle}
        disabled={!canFork}
        onclick={() => onfork?.()}
      >
        <GitForkIcon class="size-3" />
        {m.chat_action_fork_short()}
      </Button>
    {/if}
  </div>
  <span class="message-actions-content bg-border/60 h-px flex-1"></span>
  {#if meta}
    <span
      class="text-muted-foreground desk-mono pointer-events-none max-w-[45%] shrink-0 truncate text-[11px]"
      title={metaTitle ?? meta}
    >
      {meta}
    </span>
  {/if}
</div>
