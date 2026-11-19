import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";
import * as Popover from "#lib/components/ui/popover/index.ts";
import { cn } from "#lib/utils.ts";
import IconMoreHorizontal from "~icons/lucide/more-horizontal";

export interface ComposerMoreButtonProps {
  /** Accessible name and tooltip of the trigger. */
  label: string;
  /** Show a dot because a hidden option is set. */
  active?: boolean;
  open?: boolean;
  /** Called whenever the popover opens or closes (`bind:open` in the Svelte app). */
  onOpenChange?: (open: boolean) => void;
  contentClass?: string;
  children?: JSX.Element;
}

/** Round "more options" button plus popover. */
export default function ComposerMoreButton(props: ComposerMoreButtonProps) {
  return (
    <Popover.Root open={props.open} onOpenChange={(next) => props.onOpenChange?.(next)}>
      <Popover.Trigger
        class={cn(
          "text-muted-foreground hover:bg-muted hover:text-foreground focus-visible:ring-ring/40 relative inline-flex size-7 shrink-0 items-center justify-center rounded-full outline-none transition-colors focus-visible:ring-2",
          props.active ? "text-signal" : undefined,
        )}
        title={props.label}
        aria-label={props.label}
      >
        <IconMoreHorizontal class="size-4" />
        <Show when={props.active ?? false}>
          <span class="bg-signal absolute top-1 right-1 size-1.5 rounded-full" />
        </Show>
      </Popover.Trigger>
      <Popover.Content align="end" class={cn("w-72", props.contentClass)}>
        {props.children}
      </Popover.Content>
    </Popover.Root>
  );
}
