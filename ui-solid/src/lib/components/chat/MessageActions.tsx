import { Button } from "#lib/components/ui/button/index.ts";
import { m } from "#lib/paraglide/messages.js";
import { toast } from "#lib/components/ui/sonner/index.ts";
import { cn } from "#lib/utils.ts";
import IconCopy from "~icons/lucide/copy";
import IconGitFork from "~icons/lucide/git-fork";
import IconUndo2 from "~icons/lucide/undo-2";

export interface MessageActionsProps {
  /** Plain text copied by the Copy action. */
  text: string;
  /** `null` hides the Fork action entirely. */
  canFork: boolean | null;
  /** Why Fork is disabled (shown as a tooltip). */
  forkReason?: string | null;
  /** `null` hides the Revert action entirely. */
  canRevert?: boolean | null;
  /** Why Revert is disabled (shown as a tooltip). */
  revertReason?: string | null;
  onrevert?: () => void;
  /** Keep the actions visible even on pointer devices (used for the latest message). */
  prominent?: boolean;
  /** Metadata shown at the right end of the boundary row. */
  meta?: string | null;
  /** Full value shown when hovering the metadata (for example a full date). */
  metaTitle?: string | null;
  onfork?: () => void;
}

/**
 * Action row below one message: copy, plus revert / fork when the agent allows
 * it.
 *
 * It is the normal-flow boundary below a message: its own height becomes the
 * space under the message, so the layout follows its content instead of a
 * reserved magic number. It is always present on touch screens and fades in on
 * hover / focus on pointer devices that can hover.
 */
export default function MessageActions(props: MessageActionsProps) {
  const forkTitle = () =>
    props.canFork === true ? m.chat_action_fork() : (props.forkReason ?? m.chat_action_fork_unavailable());
  const revertTitle = () =>
    props.canRevert === true
      ? m.chat_action_revert()
      : (props.revertReason ?? m.chat_action_revert_unavailable());

  async function copy(): Promise<void> {
    try {
      await navigator.clipboard.writeText(props.text);
      toast.success(m.chat_action_copied());
    } catch {
      toast.error(m.settings_copy_failed());
    }
  }

  /**
   * A hidden copy of the metadata balances the row, so the action cluster keeps
   * the same centered position whether or not a message shows metadata. Only the
   * hidden copy shrinks, so on narrow rows the visible metadata stays readable.
   */
  const metaLabel = (hidden: boolean) => (
    <span
      class={cn(
        "text-muted-foreground desk-mono pointer-events-none max-w-[45%] min-w-0 truncate text-[11px]",
        !hidden && "shrink-0",
        hidden && "invisible",
      )}
      aria-hidden={hidden ? "true" : undefined}
      title={hidden ? undefined : (props.metaTitle ?? props.meta ?? undefined)}
    >
      {props.meta}
    </span>
  );

  return (
    <div
      class={cn("message-actions flex w-full items-center gap-2 py-2", props.prominent && "message-actions-prominent")}
    >
      {props.meta ? metaLabel(true) : null}
      <span class="message-actions-content bg-border/60 h-px flex-1" />
      <div class="message-actions-content border-border/60 flex items-center gap-0.5 rounded-md border bg-transparent p-0">
        <Button
          size="xs"
          variant="ghost"
          class="text-muted-foreground hover:text-foreground gap-1 px-2 font-normal"
          title={m.chat_action_copy()}
          aria-label={m.chat_action_copy()}
          onClick={() => void copy()}
        >
          <IconCopy class="size-3" />
          {m.chat_action_copy()}
        </Button>
        {props.canRevert !== null && props.canRevert !== undefined ? (
          <Button
            size="xs"
            variant="ghost"
            class="text-muted-foreground hover:text-foreground gap-1 px-2 font-normal"
            title={revertTitle()}
            aria-label={revertTitle()}
            disabled={!props.canRevert}
            onClick={() => props.onrevert?.()}
          >
            <IconUndo2 class="size-3" />
            {m.chat_action_revert()}
          </Button>
        ) : null}
        {props.canFork !== null ? (
          <Button
            size="xs"
            variant="ghost"
            class="text-muted-foreground hover:text-foreground gap-1 px-2 font-normal"
            title={forkTitle()}
            aria-label={forkTitle()}
            disabled={!props.canFork}
            onClick={() => props.onfork?.()}
          >
            <IconGitFork class="size-3" />
            {m.chat_action_fork_short()}
          </Button>
        ) : null}
      </div>
      <span class="message-actions-content bg-border/60 h-px flex-1" />
      {props.meta ? metaLabel(false) : null}
    </div>
  );
}
