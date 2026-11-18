import type { ComponentProps } from "@solidjs/web";
import { For, Match, Show, Switch, omit } from "solid-js";
import { Toast, Toaster as ToasterRegion } from "terracotta/toast";
import { cn } from "#lib/utils.ts";
import IconCircleCheck from "~icons/lucide/circle-check";
import IconInfo from "~icons/lucide/info";
import IconLoader from "~icons/lucide/loader-2";
import IconOctagonX from "~icons/lucide/octagon-x";
import IconTriangleAlert from "~icons/lucide/triangle-alert";
import { useToastQueue, type ToastItem, type ToastKind } from "./toast.ts";

/**
 * Toaster, replacing `svelte-sonner`'s component.
 *
 * `richColors` and `position` keep their svelte-sonner names because the app
 * shell passes them. Terracotta's `Toaster` is the live region; each `Toast` is
 * one entry of the queue.
 */

export type ToasterPosition =
  | "top-left"
  | "top-center"
  | "top-right"
  | "bottom-left"
  | "bottom-center"
  | "bottom-right";

const POSITIONS: Record<ToasterPosition, string> = {
  "top-left": "fixed top-4 left-4 flex-col",
  "top-center": "fixed top-4 left-1/2 -translate-x-1/2 flex-col",
  "top-right": "fixed top-4 right-4 flex-col",
  "bottom-left": "fixed bottom-4 left-4 flex-col-reverse",
  "bottom-center": "fixed bottom-4 left-1/2 -translate-x-1/2 flex-col-reverse",
  "bottom-right": "fixed bottom-4 right-4 flex-col-reverse",
};

const ACCENT: Record<ToastKind, string> = {
  default: "text-muted-foreground",
  success: "text-ok",
  error: "text-danger",
  info: "text-signal",
  warning: "text-warn",
  loading: "text-muted-foreground",
};

const SOFT: Record<ToastKind, string> = {
  default: "",
  success: "bg-ok-soft border-ok/30",
  error: "bg-danger-soft border-danger/30",
  info: "bg-signal-soft border-signal/30",
  warning: "bg-warn-soft border-warn/30",
  loading: "",
};

function ToastIcon(props: { kind: ToastKind }) {
  return (
    <Switch fallback={<IconInfo class="size-4 shrink-0 text-signal" />}>
      <Match when={props.kind === "success"}>
        <IconCircleCheck class="size-4 shrink-0 text-ok" />
      </Match>
      <Match when={props.kind === "error"}>
        <IconOctagonX class="size-4 shrink-0 text-danger" />
      </Match>
      <Match when={props.kind === "warning"}>
        <IconTriangleAlert class="size-4 shrink-0 text-warn" />
      </Match>
      <Match when={props.kind === "loading"}>
        <IconLoader class="size-4 shrink-0 animate-spin" />
      </Match>
    </Switch>
  );
}

export interface ToasterProps extends Omit<ComponentProps<"div">, "children"> {
  /** Where the stack sits. Defaults to the browser-style bottom right. */
  position?: ToasterPosition;
  /** Tint each toast by kind, as svelte-sonner's `richColors` did. */
  richColors?: boolean;
}

export function Toaster(props: ToasterProps) {
  const queue = useToastQueue();
  const rest = omit(props, "class", "position", "richColors");

  return (
    <ToasterRegion
      {...rest}
      data-slot="toaster"
      class={cn(
        "pointer-events-none z-[100] flex w-[356px] max-w-[calc(100vw-2rem)] gap-2",
        POSITIONS[props.position ?? "bottom-right"],
        props.class,
      )}
    >
      <For each={queue()}>
        {(item) => <ToastRow item={item.data} richColors={props.richColors} />}
      </For>
    </ToasterRegion>
  );
}

function ToastRow(props: { item: ToastItem; richColors?: boolean }) {
  return (
    <Toast
      data-slot="toast"
      data-kind={props.item.kind}
      class={cn(
        "bg-popover text-popover-foreground border-line pointer-events-auto flex items-start gap-2 rounded-lg border px-3 py-2.5 text-[13px] shadow-lg",
        props.richColors ? SOFT[props.item.kind] : undefined,
      )}
    >
      <span class={cn("mt-px flex", ACCENT[props.item.kind])}>
        <ToastIcon kind={props.item.kind} />
      </span>
      <div class="flex min-w-0 flex-col gap-0.5">
        <span class="break-words">{props.item.message}</span>
        <Show when={props.item.description}>
          <span class="text-muted-foreground text-xs break-words">{props.item.description}</span>
        </Show>
      </div>
    </Toast>
  );
}
