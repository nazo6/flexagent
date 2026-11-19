import type { ComponentProps, JSX } from "@solidjs/web";
import { Portal } from "@solidjs/web";
import { omit, Show } from "solid-js";
import {
  Dialog as DialogPrimitive,
  DialogDescription,
  DialogOverlay,
  DialogPanel,
  DialogTitle,
} from "terracotta/dialog";
import { cn } from "#lib/utils.ts";
import IconX from "~icons/lucide/x";
import { type MouseHandler } from "../controllable.ts";
import { createModalState } from "../modal.ts";
import type { ElementRef } from "../position.ts";
import { Button } from "../button/index.ts";
import { makeDialogContext, useDialogState } from "./dialog-context.ts";

/**
 * Dialog, ported from `ui/src/lib/components/ui/dialog`.
 *
 * Terracotta's `Dialog` unmounts its whole subtree while closed, including a
 * trigger placed inside it — its own examples keep the trigger outside. Our
 * `Root` therefore only owns the state, the trigger/close buttons are plain
 * elements wired to it, and `Content` mounts a Terracotta dialog around the
 * overlay and panel while it is open.
 *
 * Consequences worth knowing:
 *
 * - `Content` portals the whole Terracotta dialog (overlay + panel) to `<body>`.
 *   This keeps the panel out of surrounding links and transformed or clipped
 *   ancestors. Terracotta's modal focus walks up from the dialog root to mark
 *   the rest of the page `inert`, so a portaled dialog still traps focus.
 * - Focus returns to whatever was focused when the dialog opened; that is done
 *   here because a dialog that unmounts on close runs no close effect.
 */

const DialogContext = makeDialogContext();

export interface RootProps {
  open?: boolean;
  defaultOpen?: boolean;
  onOpenChange?: (open: boolean) => void;
  disabled?: boolean;
  children?: JSX.Element;
}

export function Root(props: RootProps) {
  const state = createModalState(props);
  return <DialogContext value={state}>{props.children}</DialogContext>;
}

export type PortalProps = {
  /** Where to mount the portal; defaults to `document.body`. */
  mount?: Element;
  children?: JSX.Element;
};

/** Wraps a whole dialog root in a custom-mount portal (`Content` portals by itself). */
export function PortalRoot(props: PortalProps) {
  return <Portal mount={props.mount}>{props.children}</Portal>;
}

export interface TriggerProps extends Omit<ComponentProps<"button">, "ref" | "onClick"> {
  ref?: ElementRef<HTMLButtonElement>;
  onClick?: MouseHandler<HTMLButtonElement>;
}

export function Trigger(props: TriggerProps) {
  const state = useDialogState(DialogContext, "Dialog.Trigger");
  const rest = omit(props, "class", "children", "ref", "onClick", "type", "disabled");

  const handleClick: MouseHandler<HTMLButtonElement> = (event) => {
    props.onClick?.(event);
    if (event.defaultPrevented || props.disabled || state.disabled()) return;
    state.setOpen(true);
  };

  return (
    <button
      {...rest}
      ref={(element) => props.ref?.(element)}
      data-slot="dialog-trigger"
      type={props.type ?? "button"}
      disabled={props.disabled}
      aria-haspopup="dialog"
      aria-expanded={state.open() ? "true" : "false"}
      class={props.class}
      onClick={handleClick}
    >
      {props.children}
    </button>
  );
}

export type OverlayProps = Omit<ComponentProps<"div">, "ref"> & { ref?: ElementRef };

export function Overlay(props: OverlayProps) {
  const rest = omit(props, "class", "children", "ref");
  return (
    <DialogOverlay
      {...rest}
      ref={props.ref}
      data-slot="dialog-overlay"
      class={cn(
        "bg-black/10 supports-backdrop-filter:backdrop-blur-xs fixed inset-0 isolate z-50 animate-in fade-in-0 duration-100",
        props.class,
      )}
    />
  );
}

export interface ContentProps extends Omit<ComponentProps<"div">, "ref"> {
  ref?: ElementRef;
  showCloseButton?: boolean;
}

export function Content(props: ContentProps) {
  const state = useDialogState(DialogContext, "Dialog.Content");
  const rest = omit(props, "class", "children", "ref", "showCloseButton");

  return (
    <Show when={state.open()}>
      <Portal>
        <DialogPrimitive
          isOpen
          disabled={state.disabled()}
          onChange={(next) => state.setOpen(next)}
        >
          <Overlay />
          <DialogPanel
            {...rest}
            ref={props.ref}
            data-slot="dialog-content"
            class={cn(
              "bg-popover text-popover-foreground border-border animate-in fade-in-0 zoom-in-95 grid max-w-[calc(100%_-_2rem)] gap-5 rounded-xl border p-6 text-sm duration-100 sm:max-w-md fixed top-1/2 left-1/2 z-50 w-full -translate-x-1/2 -translate-y-1/2 shadow-lg outline-none",
              props.class,
            )}
          >
            {props.children}
            {props.showCloseButton === false ? null : (
              <Button
                variant="ghost"
                size="icon-sm"
                class="absolute top-4 right-4"
                onClick={() => state.close()}
              >
                <IconX />
                <span class="sr-only">Close</span>
              </Button>
            )}
          </DialogPanel>
        </DialogPrimitive>
      </Portal>
    </Show>
  );
}

export type HeaderProps = ComponentProps<"div">;

export function Header(props: HeaderProps) {
  const rest = omit(props, "class", "children");
  return (
    <div {...rest} data-slot="dialog-header" class={cn("gap-2 flex flex-col", props.class)}>
      {props.children}
    </div>
  );
}

export type FooterProps = ComponentProps<"div"> & { showCloseButton?: boolean };

export function Footer(props: FooterProps) {
  const state = useDialogState(DialogContext, "Dialog.Footer");
  const rest = omit(props, "class", "children", "showCloseButton");
  return (
    <div
      {...rest}
      data-slot="dialog-footer"
      class={cn("gap-2 flex flex-col-reverse gap-2 sm:flex-row sm:justify-end", props.class)}
    >
      {props.children}
      {props.showCloseButton ? (
        <Button variant="outline" onClick={() => state.close()}>
          Close
        </Button>
      ) : null}
    </div>
  );
}

export type TitleProps = Omit<ComponentProps<"h2">, "ref"> & {
  ref?: ElementRef<HTMLHeadingElement>;
};

export function Title(props: TitleProps) {
  const rest = omit(props, "class", "children", "ref");
  return (
    <DialogTitle
      {...rest}
      ref={props.ref}
      data-slot="dialog-title"
      class={cn("leading-none font-medium", props.class)}
    >
      {props.children}
    </DialogTitle>
  );
}

export type DescriptionProps = Omit<ComponentProps<"p">, "ref"> & {
  ref?: ElementRef<HTMLParagraphElement>;
};

export function Description(props: DescriptionProps) {
  const rest = omit(props, "class", "children", "ref");
  return (
    <DialogDescription
      {...rest}
      ref={props.ref}
      data-slot="dialog-description"
      class={cn(
        "text-muted-foreground *:[a]:hover:text-foreground text-sm *:[a]:underline *:[a]:underline-offset-3",
        props.class,
      )}
    >
      {props.children}
    </DialogDescription>
  );
}

export interface CloseProps extends Omit<ComponentProps<"button">, "ref" | "onClick"> {
  ref?: ElementRef<HTMLButtonElement>;
  onClick?: MouseHandler<HTMLButtonElement>;
}

export function Close(props: CloseProps) {
  const state = useDialogState(DialogContext, "Dialog.Close");
  const rest = omit(props, "class", "children", "ref", "onClick", "type");

  const handleClick: MouseHandler<HTMLButtonElement> = (event) => {
    props.onClick?.(event);
    if (event.defaultPrevented) return;
    state.close();
  };

  return (
    <button
      {...rest}
      ref={(element) => props.ref?.(element)}
      data-slot="dialog-close"
      type={props.type ?? "button"}
      class={props.class}
      onClick={handleClick}
    >
      {props.children}
    </button>
  );
}
