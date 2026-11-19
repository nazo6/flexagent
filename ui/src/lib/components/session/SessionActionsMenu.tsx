import { createSignal, Show } from "solid-js";
import * as AlertDialog from "#lib/components/ui/alert-dialog/index.ts";
import * as Dialog from "#lib/components/ui/dialog/index.ts";
import * as DropdownMenu from "#lib/components/ui/dropdown-menu/index.ts";
import { Button } from "#lib/components/ui/button/index.ts";
import { Input } from "#lib/components/ui/input/index.ts";
import { Label } from "#lib/components/ui/label/index.ts";
import type { SessionSummary } from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";
import { sync } from "#lib/stores/app.ts";
import { cn } from "#lib/utils.ts";
import { toast } from "#lib/components/ui/sonner/index.ts";
import IconArchive from "~icons/lucide/archive";
import IconArchiveRestore from "~icons/lucide/archive-restore";
import IconMoreHorizontal from "~icons/lucide/more-horizontal";
import IconPencil from "~icons/lucide/pencil";
import IconPlay from "~icons/lucide/play";
import IconPlus from "~icons/lucide/plus";
import IconSkull from "~icons/lucide/skull";
import IconTrash2 from "~icons/lucide/trash-2";
import { errorMessage } from "#lib/api/error-messages.ts";

export interface SessionActionsMenuProps {
  session: SessionSummary;
  /** Called after deletion completes (for example to navigate away from the open session). */
  onDeleted?: () => void;
  /** Extra classes for the trigger button (for example hover reveal). */
  buttonClass?: string;
  /** Size classes for the trigger icon. */
  iconClass?: string;
  align?: "start" | "center" | "end";
  /** `session` adds header-only actions (new session here, resume, force stop). */
  scope?: "row" | "session";
  canNewSessionHere?: boolean;
  onNewSessionHere?: () => void;
  canResume?: boolean;
  onResume?: () => void;
  onForceStop?: () => void;
}

/** Overflow menu with rename / archive / delete (plus session header actions). */
export default function SessionActionsMenu(props: SessionActionsMenuProps) {
  const archived = () => props.session.archived_at !== null;
  const [confirmOpen, setConfirmOpen] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const [renameOpen, setRenameOpen] = createSignal(false);
  const [renameTitle, setRenameTitle] = createSignal("");
  const [renaming, setRenaming] = createSignal(false);

  function openRename(): void {
    setRenameTitle(props.session.title);
    setRenameOpen(true);
  }

  /** Rename the session (the node emits `SessionTitleChanged`). */
  async function handleRename(event: Event & { currentTarget: HTMLFormElement }): Promise<void> {
    event.preventDefault();
    const title = renameTitle().trim();
    if (title === "" || renaming()) return;
    setRenaming(true);
    try {
      await sync.renameSession(props.session.session_id, title);
      toast.success(m.session_renamed());
      setRenameOpen(false);
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      setRenaming(false);
    }
  }

  /** Archive/unarchive the session (reversible; the event log is retained). */
  async function toggleArchive(): Promise<void> {
    if (busy()) return;
    setBusy(true);
    // Capture the state before the store updates, so the toast describes the action just taken
    const wasArchived = archived();
    try {
      await sync.archiveSession(props.session.session_id, !wasArchived);
      toast.success(wasArchived ? m.session_unarchived() : m.session_archived());
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      setBusy(false);
    }
  }

  /** Delete the session (erasing the chat log; irreversible). */
  async function handleDelete(): Promise<void> {
    if (busy()) return;
    setBusy(true);
    try {
      await sync.deleteSession(props.session.session_id);
      toast.success(m.session_deleted());
      setConfirmOpen(false);
      props.onDeleted?.();
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      {/* Stop the click from navigating when this is inside a sidebar row (link) */}
      <span
        class="inline-flex"
        role="presentation"
        onClick={(event) => {
          event.preventDefault();
          event.stopPropagation();
        }}
      >
        <DropdownMenu.Root>
          <DropdownMenu.Trigger
            class={cn(
              "text-muted-foreground hover:text-foreground focus-visible:ring-ring/40 inline-flex items-center justify-center rounded p-0.5 outline-none transition-colors focus-visible:ring-1",
              props.buttonClass,
            )}
            title={m.session_actions_title()}
          >
            <IconMoreHorizontal class={props.iconClass ?? "size-3.5"} />
          </DropdownMenu.Trigger>
          <DropdownMenu.Content
            align={props.align ?? "end"}
            class={props.scope === "session" ? "w-52" : "w-44"}
          >
            <Show
              when={
                props.scope === "session" &&
                (props.canNewSessionHere === true ||
                  props.canResume === true ||
                  props.onForceStop !== undefined)
              }
            >
              <Show
                when={(props.canNewSessionHere ?? false) && props.onNewSessionHere !== undefined}
              >
                <DropdownMenu.Item onSelect={() => props.onNewSessionHere?.()}>
                  <IconPlus />
                  {m.session_new_here()}
                </DropdownMenu.Item>
              </Show>
              <Show when={(props.canResume ?? false) && props.onResume !== undefined}>
                <DropdownMenu.Item onSelect={() => props.onResume?.()}>
                  <IconPlay />
                  {m.chat_resume()}
                </DropdownMenu.Item>
              </Show>
              <Show when={props.onForceStop}>
                <Show when={(props.canNewSessionHere ?? false) || (props.canResume ?? false)}>
                  <DropdownMenu.Separator />
                </Show>
                <DropdownMenu.Item variant="destructive" onSelect={() => props.onForceStop?.()}>
                  <IconSkull />
                  {m.session_force_stop()}
                </DropdownMenu.Item>
              </Show>
              <DropdownMenu.Separator />
            </Show>
            <DropdownMenu.Item disabled={busy() || renaming()} onSelect={openRename}>
              <IconPencil />
              {m.session_rename_menu()}
            </DropdownMenu.Item>
            <DropdownMenu.Item disabled={busy()} onSelect={() => void toggleArchive()}>
              <Show
                when={archived()}
                fallback={
                  <>
                    <IconArchive />
                    {m.session_archive()}
                  </>
                }
              >
                <IconArchiveRestore />
                {m.session_unarchive()}
              </Show>
            </DropdownMenu.Item>
            <DropdownMenu.Separator />
            <DropdownMenu.Item
              variant="destructive"
              disabled={busy()}
              onSelect={() => setConfirmOpen(true)}
            >
              <IconTrash2 />
              {m.session_delete_menu()}
            </DropdownMenu.Item>
          </DropdownMenu.Content>
        </DropdownMenu.Root>
      </span>

      {/* Rename dialog */}
      <Dialog.Root open={renameOpen()} onOpenChange={setRenameOpen}>
        <Dialog.Content class="sm:max-w-md">
          <form class="flex flex-col gap-4" onSubmit={handleRename}>
            <Dialog.Header>
              <Dialog.Title>{m.session_rename_title()}</Dialog.Title>
              <Dialog.Description>{m.session_rename_description()}</Dialog.Description>
            </Dialog.Header>
            <div class="grid gap-2">
              <Label for="session-title">{m.session_rename_placeholder()}</Label>
              <Input
                id="session-title"
                value={renameTitle()}
                onValueChange={setRenameTitle}
                maxlength={200}
                aria-label={m.session_rename_placeholder()}
              />
            </div>
            <Dialog.Footer>
              <Button
                type="button"
                variant="outline"
                onClick={() => setRenameOpen(false)}
                disabled={renaming()}
              >
                {m.common_cancel()}
              </Button>
              <Button type="submit" disabled={renaming() || renameTitle().trim() === ""}>
                {m.common_save()}
              </Button>
            </Dialog.Footer>
          </form>
        </Dialog.Content>
      </Dialog.Root>

      {/* Delete confirmation dialog */}
      <AlertDialog.Root open={confirmOpen()} onOpenChange={setConfirmOpen}>
        <AlertDialog.Content>
          <AlertDialog.Header>
            <AlertDialog.Title class="text-destructive flex items-center gap-2">
              <IconTrash2 class="size-5" />
              {m.session_delete_confirm_title()}
            </AlertDialog.Title>
            <AlertDialog.Description class="text-sm">
              {m.session_delete_confirm_description({ title: props.session.title })}
            </AlertDialog.Description>
          </AlertDialog.Header>
          <AlertDialog.Footer>
            <AlertDialog.Cancel disabled={busy()}>{m.common_cancel()}</AlertDialog.Cancel>
            <AlertDialog.Action
              class="bg-destructive hover:bg-destructive/90 text-destructive-foreground"
              disabled={busy()}
              onClick={handleDelete}
            >
              {busy() ? m.session_deleting() : m.common_delete()}
            </AlertDialog.Action>
          </AlertDialog.Footer>
        </AlertDialog.Content>
      </AlertDialog.Root>
    </>
  );
}
