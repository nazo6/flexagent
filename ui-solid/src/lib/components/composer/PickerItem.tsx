import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";
import { cn } from "#lib/utils.ts";
import IconCheck from "~icons/lucide/check";
import IconStar from "~icons/lucide/star";

export interface PickerItemProps {
  selected?: boolean;
  disabled?: boolean;
  onselect?: () => void;
  /**
   * Optional trailing favorite toggle rendered next to (not inside) the row
   * button, so the two controls are never nested.
   */
  favorite?: { active: boolean; label: string; onToggle: () => void } | null;
  class?: string;
  children?: JSX.Element;
}

/** One row of a picker popover (project / directory / agent / model). */
export default function PickerItem(props: PickerItemProps) {
  return (
    <div class="flex w-full items-center gap-1">
      <button
        type="button"
        disabled={props.disabled ?? false}
        class={cn(
          "hover:bg-accent focus-visible:bg-accent flex min-w-0 flex-1 items-center gap-2 rounded-md px-2 py-1.5 text-left text-[13px] transition-colors outline-none disabled:pointer-events-none disabled:opacity-50",
          props.class,
        )}
        onClick={() => props.onselect?.()}
      >
        <span class="min-w-0 flex-1">{props.children}</span>
        <Show when={props.selected ?? false}>
          <IconCheck class="text-signal size-3.5 shrink-0" />
        </Show>
      </button>
      <Show when={props.favorite}>
        {(favorite) => (
          <button
            type="button"
            class={cn(
              "hover:bg-accent flex size-7 shrink-0 items-center justify-center rounded-md transition-colors",
              favorite().active
                ? "text-signal"
                : "text-muted-foreground hover:text-foreground",
            )}
            title={favorite().label}
            aria-label={favorite().label}
            aria-pressed={favorite().active ? "true" : "false"}
            onClick={(event) => {
              // Toggling a favorite must not select the row or close the popover.
              event.stopPropagation();
              favorite().onToggle();
            }}
          >
            <IconStar class={cn("size-3.5", favorite().active && "fill-current")} />
          </button>
        )}
      </Show>
    </div>
  );
}
