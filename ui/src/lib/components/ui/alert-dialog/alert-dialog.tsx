import type { ComponentProps, JSX } from "@solidjs/web";
import { Portal } from "@solidjs/web";
import { omit, Show } from "solid-js";
import {
  AlertDialog as AlertDialogPrimitive,
  AlertDialogDescription,
  AlertDialogPanel,
  AlertDialogTitle,
} from "terracotta/alert-dialog";
import { cn } from "#lib/utils.ts";
import { type MouseHandler } from "../controllable.ts";
import { createModalState } from "../modal.ts";
import { buttonVariants, type ButtonSize, type ButtonVariant } from "../button/index.ts";
import { makeDialogContext, useDialogState } from "../dialog/dialog-context.ts";
import type { ElementRef } from "../position.ts";

/**
 * Alert dialog, ported from `ui/src/lib/components/ui/alert-dialog`.
 *
 * Same shape as `Dialog` — see the note there about the trigger living outside
 * Terracotta's root. Unlike bits-ui, `Action` does not close the dialog by
 * itself: the screens in `ui/` close it when their async work settles, and a
 * busy state must stay visible until then.
 */

const AlertDialogContext = makeDialogContext();

export interface RootProps {
  open?: boolean;
  defaultOpen?: boolean;
  onOpenChange?: (open: boolean) => void;
  disabled?: boolean;
  children?: JSX.Element;
}

export function Root(props: RootProps) {
  const state = createModalState(props);
  return <AlertDialogContext value={state}>{props.children}</AlertDialogContext>;
}

export type PortalProps = {
  mount?: Element;
  children?: JSX.Element;
};

export function PortalRoot(props: PortalProps) {
  return <Portal mount={props.mount}>{props.children}</Portal>;
}

export interface TriggerProps extends Omit<ComponentProps<"button">, "ref" | "onClick"> {
  ref?: ElementRef<HTMLButtonElement>;
  onClick?: MouseHandler<HTMLButtonElement>;
}

export function Trigger(props: TriggerProps) {
  const state = useDialogState(AlertDialogContext, "AlertDialog.Trigger");
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
      data-slot="alert-dialog-trigger"
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
    <div
      {...rest}
      ref={props.ref}
      data-slot="alert-dialog-overlay"
      class={cn(
        "bg-black/10 supports-backdrop-filter:backdrop-blur-xs fixed inset-0 z-50 animate-in fade-in-0 duration-100",
        props.class,
      )}
    />
  );
}

export interface ContentProps extends Omit<ComponentProps<"div">, "ref"> {
  ref?: ElementRef;
  size?: "default" | "sm";
}

export function Content(props: ContentProps) {
  const state = useDialogState(AlertDialogContext, "AlertDialog.Content");
  const rest = omit(props, "class", "children", "ref", "size");

  return (
    <Show when={state.open()}>
      <Portal>
        <AlertDialogPrimitive
          isOpen
          disabled={state.disabled()}
          onChange={(next) => state.setOpen(next)}
        >
          <Overlay />
          <AlertDialogPanel
            {...rest}
            ref={props.ref}
            data-slot="alert-dialog-content"
            data-size={props.size ?? "default"}
            class={cn(
              "bg-popover text-popover-foreground border-border animate-in fade-in-0 zoom-in-95 gap-5 rounded-xl border p-6 text-sm duration-100 shadow-lg data-[size=default]:max-w-xs data-[size=sm]:max-w-xs data-[size=default]:sm:max-w-lg group/alert-dialog-content fixed top-1/2 left-1/2 z-50 grid w-full -translate-x-1/2 -translate-y-1/2 outline-none",
              props.class,
            )}
          >
            {props.children}
          </AlertDialogPanel>
        </AlertDialogPrimitive>
      </Portal>
    </Show>
  );
}

export type HeaderProps = ComponentProps<"div">;

export function Header(props: HeaderProps) {
  const rest = omit(props, "class", "children");
  return (
    <div
      {...rest}
      data-slot="alert-dialog-header"
      class={cn(
        "grid grid-rows-[auto_1fr] place-items-center gap-1.5 text-center has-data-[slot=alert-dialog-media]:grid-rows-[auto_auto_1fr] has-data-[slot=alert-dialog-media]:gap-x-6 sm:group-data-[size=default]/alert-dialog-content:place-items-start sm:group-data-[size=default]/alert-dialog-content:text-left sm:group-data-[size=default]/alert-dialog-content:has-data-[slot=alert-dialog-media]:grid-rows-[auto_1fr]",
        props.class,
      )}
    >
      {props.children}
    </div>
  );
}

export type FooterProps = ComponentProps<"div">;

export function Footer(props: FooterProps) {
  const rest = omit(props, "class", "children");
  return (
    <div
      {...rest}
      data-slot="alert-dialog-footer"
      class={cn(
        "flex flex-col-reverse gap-2 group-data-[size=sm]/alert-dialog-content:grid group-data-[size=sm]/alert-dialog-content:grid-cols-2 sm:flex-row sm:justify-end",
        props.class,
      )}
    >
      {props.children}
    </div>
  );
}

export type TitleProps = Omit<ComponentProps<"h2">, "ref"> & {
  ref?: ElementRef<HTMLHeadingElement>;
};

export function Title(props: TitleProps) {
  const rest = omit(props, "class", "children", "ref");
  return (
    <AlertDialogTitle
      {...rest}
      ref={props.ref}
      data-slot="alert-dialog-title"
      class={cn(
        "text-lg font-medium sm:group-data-[size=default]/alert-dialog-content:group-has-data-[slot=alert-dialog-media]/alert-dialog-content:col-start-2",
        props.class,
      )}
    >
      {props.children}
    </AlertDialogTitle>
  );
}

export type DescriptionProps = Omit<ComponentProps<"p">, "ref"> & {
  ref?: ElementRef<HTMLParagraphElement>;
};

export function Description(props: DescriptionProps) {
  const rest = omit(props, "class", "children", "ref");
  return (
    <AlertDialogDescription
      {...rest}
      ref={props.ref}
      data-slot="alert-dialog-description"
      class={cn(
        "text-muted-foreground *:[a]:hover:text-foreground text-sm text-balance md:text-pretty *:[a]:underline *:[a]:underline-offset-3",
        props.class,
      )}
    >
      {props.children}
    </AlertDialogDescription>
  );
}

export type MediaProps = ComponentProps<"div">;

export function Media(props: MediaProps) {
  const rest = omit(props, "class", "children");
  return (
    <div
      {...rest}
      data-slot="alert-dialog-media"
      class={cn(
        "bg-muted mb-2 inline-flex size-16 items-center justify-center rounded-md sm:group-data-[size=default]/alert-dialog-content:row-span-2 *:[svg:not([class*='size-'])]:size-8",
        props.class,
      )}
    >
      {props.children}
    </div>
  );
}

export interface ActionProps extends Omit<ComponentProps<"button">, "ref" | "onClick"> {
  ref?: ElementRef<HTMLButtonElement>;
  onClick?: MouseHandler<HTMLButtonElement>;
  variant?: ButtonVariant;
  size?: ButtonSize;
}

export function Action(props: ActionProps) {
  const rest = omit(props, "class", "children", "ref", "onClick", "variant", "size", "type");
  return (
    <button
      {...rest}
      ref={(element) => props.ref?.(element)}
      data-slot="alert-dialog-action"
      type={props.type ?? "button"}
      class={cn(buttonVariants({ variant: props.variant, size: props.size }), props.class)}
      onClick={props.onClick}
    >
      {props.children}
    </button>
  );
}

export interface CancelProps extends Omit<ComponentProps<"button">, "ref" | "onClick"> {
  ref?: ElementRef<HTMLButtonElement>;
  onClick?: MouseHandler<HTMLButtonElement>;
  variant?: ButtonVariant;
  size?: ButtonSize;
}

export function Cancel(props: CancelProps) {
  const state = useDialogState(AlertDialogContext, "AlertDialog.Cancel");
  const rest = omit(props, "class", "children", "ref", "onClick", "variant", "size", "type");

  const handleClick: MouseHandler<HTMLButtonElement> = (event) => {
    props.onClick?.(event);
    if (event.defaultPrevented) return;
    state.close();
  };

  return (
    <button
      {...rest}
      ref={(element) => props.ref?.(element)}
      data-slot="alert-dialog-cancel"
      type={props.type ?? "button"}
      class={cn(
        buttonVariants({ variant: props.variant ?? "outline", size: props.size }),
        props.class,
      )}
      onClick={handleClick}
    >
      {props.children}
    </button>
  );
}
