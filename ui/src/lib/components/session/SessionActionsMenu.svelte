<script lang="ts">
  import * as AlertDialog from "#lib/components/ui/alert-dialog/index.ts";
  import * as Dialog from "#lib/components/ui/dialog/index.ts";
  import * as DropdownMenu from "#lib/components/ui/dropdown-menu/index.ts";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { Input } from "#lib/components/ui/input/index.ts";
  import { Label } from "#lib/components/ui/label/index.ts";
  import type { SessionSummary } from "#lib/generated/bindings.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { sync } from "#lib/stores/app.svelte.ts";
  import { cn } from "#lib/utils.ts";
  import { toast } from "svelte-sonner";
  import ArchiveIcon from "@lucide/svelte/icons/archive";
  import ArchiveRestoreIcon from "@lucide/svelte/icons/archive-restore";
  import MoreHorizontalIcon from "@lucide/svelte/icons/more-horizontal";
  import PencilIcon from "@lucide/svelte/icons/pencil";
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
  let renameOpen = $state(false);
  let renameTitle = $state("");
  let renaming = $state(false);

  function openRename(): void {
    renameTitle = session.title;
    renameOpen = true;
  }

  /** Rename the session (the node emits `SessionTitleChanged`). */
  async function handleRename(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    const title = renameTitle.trim();
    if (title === "" || renaming) return;
    renaming = true;
    try {
      await sync.renameSession(session.session_id, title);
      toast.success(m.session_renamed());
      renameOpen = false;
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      renaming = false;
    }
  }

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
      <DropdownMenu.Item disabled={busy || renaming} onSelect={openRename}>
        <PencilIcon />
        {m.session_rename_menu()}
      </DropdownMenu.Item>
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

<!-- Rename dialog -->
<Dialog.Root bind:open={renameOpen}>
  <Dialog.Content class="sm:max-w-md">
    <form class="flex flex-col gap-4" onsubmit={handleRename}>
      <Dialog.Header>
        <Dialog.Title>{m.session_rename_title()}</Dialog.Title>
        <Dialog.Description>{m.session_rename_description()}</Dialog.Description>
      </Dialog.Header>
      <div class="grid gap-2">
        <Label for="session-title">{m.session_rename_placeholder()}</Label>
        <Input
          id="session-title"
          bind:value={renameTitle}
          maxlength={200}
          aria-label={m.session_rename_placeholder()}
        />
      </div>
      <Dialog.Footer>
        <Button
          type="button"
          variant="outline"
          onclick={() => (renameOpen = false)}
          disabled={renaming}
        >
          {m.common_cancel()}
        </Button>
        <Button type="submit" disabled={renaming || renameTitle.trim() === ""}>
          {m.common_save()}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>

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
