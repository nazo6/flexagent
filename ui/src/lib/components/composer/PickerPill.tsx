import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";
import * as Popover from "#lib/components/ui/popover/index.ts";
import { cn } from "#lib/utils.ts";
import IconChevronDown from "~icons/lucide/chevron-down";
import IconLoaderCircle from "~icons/lucide/loader-circle";

export interface PickerPillProps {
  /** Accessible name (for example "Project" or "Model"). */
  label: string;
  /** Current value shown in the trigger. */
  value?: string;
  /** Optional leading icon. */
  icon?: JSX.Element;
  disabled?: boolean;
  loading?: boolean;
  class?: string;
  contentClass?: string;
  open?: boolean;
  /** Called whenever the popover opens or closes. */
  onopenchange?: (open: boolean) => void;
  children?: JSX.Element;
}

/** Pill-shaped trigger plus popover used by the composer selectors. */
export default function PickerPill(props: PickerPillProps) {
  const value = () => props.value ?? "";

  return (
    <Popover.Root open={props.open} onOpenChange={(next) => props.onopenchange?.(next)}>
      <Popover.Trigger
        disabled={props.disabled ?? false}
        class={cn(
          "border-border bg-chip hover:bg-chip-hover hover:text-foreground focus-visible:ring-ring/40 inline-flex h-7 max-w-60 min-w-0 items-center gap-1.5 rounded-full border px-2.5 text-xs transition-colors outline-none focus-visible:ring-2 disabled:pointer-events-none disabled:opacity-50",
          props.class,
        )}
        title={value() === "" ? props.label : `${props.label}: ${value()}`}
        aria-label={value() === "" ? props.label : `${props.label}: ${value()}`}
      >
        {props.icon}
        <Show
          when={value() !== ""}
          fallback={<span class="text-muted-foreground truncate">{props.label}</span>}
        >
          <span class="truncate">{value()}</span>
        </Show>
        <Show
          when={props.loading ?? false}
          fallback={<IconChevronDown class="text-muted-foreground size-3 shrink-0" />}
        >
          <IconLoaderCircle class="text-muted-foreground size-3 shrink-0 animate-spin" />
        </Show>
      </Popover.Trigger>
      <Popover.Content class={cn("w-72", props.contentClass)}>{props.children}</Popover.Content>
    </Popover.Root>
  );
}
