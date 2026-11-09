<script lang="ts">
  import * as AlertDialog from "#lib/components/ui/alert-dialog/index.ts";
  import * as DropdownMenu from "#lib/components/ui/dropdown-menu/index.ts";
  import type { SessionSummary } from "#lib/generated/bindings.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { sync } from "#lib/stores/app.svelte.ts";
  import { cn } from "#lib/utils.ts";
  import { toast } from "svelte-sonner";
  import ArchiveIcon from "@lucide/svelte/icons/archive";
  import ArchiveRestoreIcon from "@lucide/svelte/icons/archive-restore";
  import MoreHorizontalIcon from "@lucide/svelte/icons/more-horizontal";
  import Trash2Icon from "@lucide/svelte/icons/trash-2";

  interface Props {
    session: SessionSummary;
    /** Called after deletion completes (for example to navigate away from the open session). */
    onDeleted?: () => void;
    /** Extra classes for the trigger button (for example hover reveal). */
    buttonClass?: string;
    /** Size classes for the trigger icon. */
    iconClass?: string;
    align?: "start" | "center" | "end";
  }

  let { session, onDeleted, buttonClass, iconClass = "size-3.5", align = "end" }: Props = $props();

  const archived = $derived(session.archived_at !== null);
  let confirmOpen = $state(false);
  let busy = $state(false);

  /** Archive/unarchive the session (reversible; the event log is retained). */
  async function toggleArchive() {
    if (busy) return;
    busy = true;
    // Capture the state before the store updates, so the toast describes the action just taken
    const wasArchived = archived;
    try {
      await sync.archiveSession(session.session_id, !wasArchived);
      toast.success(wasArchived ? m.session_unarchived() : m.session_archived());
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      busy = false;
    }
  }

  /** Delete the session (erasing the chat log; irreversible). */
  async function handleDelete() {
    if (busy) return;
    busy = true;
    try {
      await sync.deleteSession(session.session_id);
      toast.success(m.session_deleted());
      confirmOpen = false;
      onDeleted?.();
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      busy = false;
    }
  }
</script>

<!-- Stop the click from navigating when this is inside a sidebar row (link) -->
<span
  class="inline-flex"
  role="presentation"
  onclick={(event) => {
    event.preventDefault();
    event.stopPropagation();
  }}
>
  <DropdownMenu.Root>
    <DropdownMenu.Trigger
      class={cn(
        "text-muted-foreground hover:text-foreground focus-visible:ring-ring/40 inline-flex items-center justify-center rounded p-0.5 outline-none transition-colors focus-visible:ring-1",
        buttonClass,
      )}
      title={m.session_actions_title()}
    >
      <MoreHorizontalIcon class={iconClass} />
    </DropdownMenu.Trigger>
    <DropdownMenu.Content {align} class="w-44">
      <DropdownMenu.Item disabled={busy} onSelect={() => void toggleArchive()}>
        {#if archived}
          <ArchiveRestoreIcon />
          {m.session_unarchive()}
        {:else}
          <ArchiveIcon />
          {m.session_archive()}
        {/if}
      </DropdownMenu.Item>
      <DropdownMenu.Separator />
      <DropdownMenu.Item
        variant="destructive"
        disabled={busy}
        onSelect={() => (confirmOpen = true)}
      >
        <Trash2Icon />
        {m.session_delete_menu()}
      </DropdownMenu.Item>
    </DropdownMenu.Content>
  </DropdownMenu.Root>
</span>

<!-- Delete confirmation dialog -->
<AlertDialog.Root bind:open={confirmOpen}>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title class="flex items-center gap-2 text-destructive">
        <Trash2Icon class="size-5" />
        {m.session_delete_confirm_title()}
      </AlertDialog.Title>
      <AlertDialog.Description class="text-sm">
        {m.session_delete_confirm_description({ title: session.title })}
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel disabled={busy}>{m.common_cancel()}</AlertDialog.Cancel>
      <AlertDialog.Action
        class="bg-destructive hover:bg-destructive/90 text-destructive-foreground"
        disabled={busy}
        onclick={handleDelete}
      >
        {busy ? m.session_deleting() : m.common_delete()}
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
