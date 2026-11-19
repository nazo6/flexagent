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
import { makeDialogContext, useDialogState } from "../dialog/dialog-context.ts";

/**
 * Sheet, ported from `ui/src/lib/components/ui/sheet`.
 *
 * A `Dialog` whose panel is pinned to an edge. Every part takes a `dataSlot`
 * override so `drawer/` can reuse the implementation without a second copy of
 * the behaviour (drawer is the same surface with vaul's grab handle and
 * spacing).
 */

export type Side = "top" | "right" | "bottom" | "left";

const SheetContext = makeDialogContext();

export interface RootProps {
  open?: boolean;
  defaultOpen?: boolean;
  onOpenChange?: (open: boolean) => void;
  disabled?: boolean;
  children?: JSX.Element;
}

export function Root(props: RootProps) {
  const state = createModalState(props);
  return <SheetContext value={state}>{props.children}</SheetContext>;
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
  dataSlot?: string;
}

export function Trigger(props: TriggerProps) {
  const state = useDialogState(SheetContext, "Sheet.Trigger");
  const rest = omit(props, "class", "children", "ref", "onClick", "type", "disabled", "dataSlot");

  const handleClick: MouseHandler<HTMLButtonElement> = (event) => {
    props.onClick?.(event);
    if (event.defaultPrevented || props.disabled || state.disabled()) return;
    state.setOpen(true);
  };

  return (
    <button
      {...rest}
      ref={(element) => props.ref?.(element)}
      data-slot={props.dataSlot ?? "sheet-trigger"}
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

export type OverlayProps = Omit<ComponentProps<"div">, "ref"> & {
  ref?: ElementRef;
  dataSlot?: string;
};

export function Overlay(props: OverlayProps) {
  const rest = omit(props, "class", "children", "ref", "dataSlot");
  return (
    <DialogOverlay
      {...rest}
      ref={props.ref}
      data-slot={props.dataSlot ?? "sheet-overlay"}
      class={cn(
        "bg-black/10 supports-backdrop-filter:backdrop-blur-xs fixed inset-0 z-50 duration-100 animate-in fade-in-0",
        props.class,
      )}
    />
  );
}

const SIDE_CLASSES =
  "data-[side=bottom]:inset-x-0 data-[side=bottom]:bottom-0 data-[side=bottom]:h-auto data-[side=bottom]:border-t data-[side=left]:inset-y-0 data-[side=left]:left-0 data-[side=left]:h-full data-[side=left]:w-3/4 data-[side=left]:border-r data-[side=right]:inset-y-0 data-[side=right]:right-0 data-[side=right]:h-full data-[side=right]:w-3/4 data-[side=right]:border-l data-[side=top]:inset-x-0 data-[side=top]:top-0 data-[side=top]:h-auto data-[side=top]:border-b data-[side=left]:sm:max-w-sm data-[side=right]:sm:max-w-sm data-[side=bottom]:slide-in-from-bottom-10 data-[side=left]:slide-in-from-left-10 data-[side=right]:slide-in-from-right-10 data-[side=top]:slide-in-from-top-10";

export interface ContentProps extends Omit<ComponentProps<"div">, "ref"> {
  ref?: ElementRef;
  side?: Side;
  showCloseButton?: boolean;
  /** Drawer only: the grab handle at the top of a bottom sheet. */
  handle?: boolean;
  dataSlot?: string;
  overlayDataSlot?: string;
}

export function Content(props: ContentProps) {
  const state = useDialogState(SheetContext, "Sheet.Content");
  const rest = omit(
    props,
    "class",
    "children",
    "ref",
    "side",
    "showCloseButton",
    "handle",
    "dataSlot",
    "overlayDataSlot",
  );
  const side = () => props.side ?? "right";

  return (
    <Show when={state.open()}>
      <Portal>
        <DialogPrimitive
          isOpen
          disabled={state.disabled()}
          onChange={(next) => state.setOpen(next)}
        >
          <Overlay dataSlot={props.overlayDataSlot} />
          <DialogPanel
            {...rest}
            ref={props.ref}
            data-slot={props.dataSlot ?? "sheet-content"}
            data-side={side()}
            class={cn(
              "bg-popover text-popover-foreground fixed z-50 flex flex-col gap-4 bg-clip-padding text-sm shadow-lg transition duration-200 ease-in-out animate-in fade-in-0",
              SIDE_CLASSES,
              props.class,
            )}
          >
            {props.handle && side() === "bottom" ? (
              <div class="bg-muted mx-auto mt-4 h-1.5 w-[100px] shrink-0 rounded-full" />
            ) : null}
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

export type HeaderProps = ComponentProps<"div"> & { dataSlot?: string };

export function Header(props: HeaderProps) {
  const rest = omit(props, "class", "children", "dataSlot");
  return (
    <div
      {...rest}
      data-slot={props.dataSlot ?? "sheet-header"}
      class={cn("gap-1.5 p-4 flex flex-col", props.class)}
    >
      {props.children}
    </div>
  );
}

export type FooterProps = ComponentProps<"div"> & { dataSlot?: string };

export function Footer(props: FooterProps) {
  const rest = omit(props, "class", "children", "dataSlot");
  return (
    <div
      {...rest}
      data-slot={props.dataSlot ?? "sheet-footer"}
      class={cn("gap-2 p-4 mt-auto flex flex-col", props.class)}
    >
      {props.children}
    </div>
  );
}

export type TitleProps = Omit<ComponentProps<"h2">, "ref"> & {
  ref?: ElementRef<HTMLHeadingElement>;
  dataSlot?: string;
};

export function Title(props: TitleProps) {
  const rest = omit(props, "class", "children", "ref", "dataSlot");
  return (
    <DialogTitle
      {...rest}
      ref={props.ref}
      data-slot={props.dataSlot ?? "sheet-title"}
      class={cn("text-foreground font-medium", props.class)}
    >
      {props.children}
    </DialogTitle>
  );
}

export type DescriptionProps = Omit<ComponentProps<"p">, "ref"> & {
  ref?: ElementRef<HTMLParagraphElement>;
  dataSlot?: string;
};

export function Description(props: DescriptionProps) {
  const rest = omit(props, "class", "children", "ref", "dataSlot");
  return (
    <DialogDescription
      {...rest}
      ref={props.ref}
      data-slot={props.dataSlot ?? "sheet-description"}
      class={cn("text-muted-foreground text-sm", props.class)}
    >
      {props.children}
    </DialogDescription>
  );
}

export interface CloseProps extends Omit<ComponentProps<"button">, "ref" | "onClick"> {
  ref?: ElementRef<HTMLButtonElement>;
  onClick?: MouseHandler<HTMLButtonElement>;
  dataSlot?: string;
}

export function Close(props: CloseProps) {
  const state = useDialogState(SheetContext, "Sheet.Close");
  const rest = omit(props, "class", "children", "ref", "onClick", "type", "dataSlot");

  const handleClick: MouseHandler<HTMLButtonElement> = (event) => {
    props.onClick?.(event);
    if (event.defaultPrevented) return;
    state.close();
  };

  return (
    <button
      {...rest}
      ref={(element) => props.ref?.(element)}
      data-slot={props.dataSlot ?? "sheet-close"}
      type={props.type ?? "button"}
      class={props.class}
      onClick={handleClick}
    >
      {props.children}
    </button>
  );
}
