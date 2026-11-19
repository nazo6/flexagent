import type { JSX } from "@solidjs/web";
import { children, createEffect, Show } from "solid-js";
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
  onOpenChange?: (open: boolean) => void;
  children?: JSX.Element;
}

const ITEM_SELECTOR = "[data-picker-item]:not([disabled])";

type Edge = "first" | "last";

/** Pill-shaped trigger plus popover used by the composer selectors. */
export default function PickerPill(props: PickerPillProps) {
  const value = () => props.value ?? "";
  // Resolve the optional icon slot once: a JSX-valued prop builds a new tree on
  // every read.
  const icon = children(() => props.icon);
  let panel: HTMLElement | undefined;
  let pendingEdge: Edge | null = null;

  const items = (): HTMLElement[] =>
    panel ? Array.from(panel.querySelectorAll<HTMLElement>(ITEM_SELECTOR)) : [];

  function focusEdge(edge: Edge): void {
    const list = items();
    const target = edge === "first" ? list[0] : list.at(-1);
    target?.focus();
  }

  function focusFrom(key: string): void {
    const list = items();
    if (list.length === 0) return;
    const active = document.activeElement;
    const current = active instanceof HTMLElement ? list.indexOf(active) : -1;
    switch (key) {
      case "ArrowDown":
        list[current < 0 ? 0 : (current + 1) % list.length]?.focus();
        break;
      case "ArrowUp":
        list[current < 0 ? list.length - 1 : (current - 1 + list.length) % list.length]?.focus();
        break;
      case "Home":
        list[0]?.focus();
        break;
      case "End":
        list.at(-1)?.focus();
        break;
    }
  }

  function onTriggerKeyDown(event: KeyboardEvent): void {
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    event.preventDefault();
    event.stopPropagation();
    if (!props.open) {
      pendingEdge = event.key === "ArrowDown" ? "first" : "last";
      props.onOpenChange?.(true);
      return;
    }
    focusEdge(event.key === "ArrowDown" ? "first" : "last");
  }

  function onListKeyDown(event: KeyboardEvent): void {
    if (
      event.key !== "ArrowDown" &&
      event.key !== "ArrowUp" &&
      event.key !== "Home" &&
      event.key !== "End"
    ) {
      return;
    }
    const target = event.target;
    // Home / End move the caret while the reader types in the search field.
    const editingText = target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement;
    if (editingText && (event.key === "Home" || event.key === "End")) return;
    event.preventDefault();
    event.stopPropagation();
    focusFrom(event.key);
  }

  // ArrowDown / ArrowUp on the trigger opens the picker; focus lands in the
  // list once its portal has mounted.
  createEffect(
    () => props.open ?? false,
    (open) => {
      if (!open || pendingEdge === null) return;
      const edge = pendingEdge;
      pendingEdge = null;
      queueMicrotask(() => focusEdge(edge));
    },
  );

  return (
    <Popover.Root open={props.open} onOpenChange={(next) => props.onOpenChange?.(next)}>
      <Popover.Trigger
        disabled={props.disabled ?? false}
        aria-haspopup="listbox"
        onKeyDown={onTriggerKeyDown}
        class={cn(
          "border-border bg-chip hover:bg-chip-hover hover:text-foreground focus-visible:ring-ring/40 inline-flex h-7 max-w-60 min-w-0 items-center gap-1.5 rounded-full border px-2.5 text-xs transition-colors outline-none focus-visible:ring-2 disabled:pointer-events-none disabled:opacity-50",
          props.class,
        )}
        title={value() === "" ? props.label : `${props.label}: ${value()}`}
        aria-label={value() === "" ? props.label : `${props.label}: ${value()}`}
      >
        {icon()}
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
      <Popover.Content
        ref={(element) => {
          panel = element;
        }}
        role="listbox"
        aria-label={props.label}
        onKeyDown={onListKeyDown}
        class={cn("w-72", props.contentClass)}
      >
        {props.children}
      </Popover.Content>
    </Popover.Root>
  );
}
