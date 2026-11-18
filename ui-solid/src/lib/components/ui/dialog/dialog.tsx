import type { ComponentProps, JSX } from "@solidjs/web";
import { Portal } from "@solidjs/web";
import { createEffect, omit, Show } from "solid-js";
import {
  Dialog as DialogPrimitive,
  DialogDescription,
  DialogOverlay,
  DialogPanel,
  DialogTitle,
} from "terracotta/dialog";
import { cn } from "#lib/utils.ts";
import IconX from "~icons/lucide/x";
import { createControllable, type MouseHandler } from "../controllable.ts";
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
 * - The panel is rendered where `Content` sits, not portaled: Terracotta marks
 *   everything outside the dialog root `inert`, and a panel in another subtree
 *   would be caught by that. It is `position: fixed`, so it still escapes
 *   `overflow` and paints above the page. A caller that must escape a stacking
 *   context wraps the whole `<Dialog.Root>` in `<Dialog.Portal>`.
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
  const [open, setOpen] = createControllable(
    () => props.open,
    props.onOpenChange,
    props.defaultOpen ?? false,
  );

  let restoreFocus: HTMLElement | null = null;

  const change = (next: boolean) => {
    if (next === open()) return;
    if (next) {
      restoreFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    }
    const target = next ? null : restoreFocus;
    if (!next) restoreFocus = null;
    setOpen(next);
    // After the flush: the closed dialog has released `inert` by then.
    if (target) queueMicrotask(() => target.focus());
  };

  const state = { open, setOpen: change, close: () => change(false) };

  // bits-ui locked the page for us; Terracotta leaves it to the app.
  createEffect(
    () => open(),
    (isOpen) => {
      if (!isOpen) return;
      const previous = document.body.style.overflow;
      document.body.style.overflow = "hidden";
      return () => {
        document.body.style.overflow = previous;
      };
    },
  );

  return <DialogContext value={state}>{props.children}</DialogContext>;
}

export type PortalProps = {
  /** Where to mount the portal; defaults to `document.body`. */
  mount?: Element;
  children?: JSX.Element;
};

/** Wraps a whole dialog root, not its content: see the note on `Content`. */
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
    if (!props.disabled) state.setOpen(true);
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
      <DialogPrimitive isOpen onChange={(next) => state.setOpen(next)}>
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

export type TitleProps = ComponentProps<"h2">;

export function Title(props: TitleProps) {
  const rest = omit(props, "class", "children", "ref");
  return (
    <DialogTitle
      {...rest}
      data-slot="dialog-title"
      class={cn("leading-none font-medium", props.class)}
    >
      {props.children}
    </DialogTitle>
  );
}

export type DescriptionProps = ComponentProps<"p">;

export function Description(props: DescriptionProps) {
  const rest = omit(props, "class", "children", "ref");
  return (
    <DialogDescription
      {...rest}
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
