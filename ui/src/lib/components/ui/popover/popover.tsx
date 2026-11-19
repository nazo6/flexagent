import type { ComponentProps, JSX } from "@solidjs/web";
import { Portal } from "@solidjs/web";
import { createSignal, omit, Show } from "solid-js";
import { Popover as PopoverPrimitive, PopoverButton, PopoverPanel } from "terracotta/popover";
import { cn } from "#lib/utils.ts";
import { createControllable, toBoolean, type MouseHandler } from "../controllable.ts";
import {
  createPanelTree,
  composeRef,
  PanelTreeContext,
  registerPanelElement,
  useAnchoredPanel,
  type ElementRef,
  type PanelAlign,
} from "../position.ts";
import { ANCHORED_PANEL_ATTRIBUTE, dismissOnOutsidePointer } from "../position.ts";
import { PopoverContext, usePopoverState } from "./popover-context.ts";

/**
 * Popover, ported from `ui/src/lib/components/ui/popover`.
 *
 * The panel is portaled to `<body>` and positioned by `useAnchoredPanel`, so it
 * escapes the scroll containers and stacking contexts of the app shell.
 */

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
    () => props.onOpenChange,
    () => props.defaultOpen ?? false,
  );
  const [anchor, setAnchorElement] = createSignal<HTMLElement>();

  return (
    <PopoverContext
      value={{ open, setOpen, anchor, setAnchor: (element) => setAnchorElement(() => element) }}
    >
      <PopoverPrimitive
        isOpen={open()}
        onChange={setOpen}
        disabled={props.disabled === undefined ? undefined : toBoolean(props.disabled)}
      >
        {props.children}
      </PopoverPrimitive>
    </PopoverContext>
  );
}

export interface TriggerProps extends Omit<ComponentProps<"button">, "ref" | "onClick" | "type"> {
  ref?: ElementRef<HTMLButtonElement>;
  onClick?: MouseHandler<HTMLButtonElement>;
  type?: "button" | "submit" | "reset";
}

export function Trigger(props: TriggerProps) {
  const state = usePopoverState("Popover.Trigger");
  const rest = omit(props, "class", "children", "ref", "onClick", "disabled", "type");

  return (
    <PopoverButton
      {...rest}
      ref={composeRef(props.ref, (element: HTMLButtonElement) => state.setAnchor(element))}
      data-slot="popover-trigger"
      type={props.type ?? "button"}
      disabled={props.disabled === undefined ? undefined : toBoolean(props.disabled)}
      class={props.class}
      onClick={props.onClick}
    >
      {props.children}
    </PopoverButton>
  );
}

export type PortalProps = {
  mount?: Element;
  children?: JSX.Element;
};

/** The panel is already portaled; this exists for API compatibility. */
export function PortalRoot(props: PortalProps) {
  return <Portal mount={props.mount}>{props.children}</Portal>;
}

export interface ContentProps extends Omit<ComponentProps<"div">, "ref"> {
  ref?: ElementRef;
  side?: "top" | "right" | "bottom" | "left";
  align?: PanelAlign;
  sideOffset?: number;
  alignOffset?: number;
}

export function Content(props: ContentProps) {
  const state = usePopoverState("Popover.Content");
  const tree = createPanelTree();
  const rest = omit(
    props,
    "class",
    "children",
    "ref",
    "side",
    "align",
    "sideOffset",
    "alignOffset",
  );

  dismissOnOutsidePointer({
    open: state.open,
    anchor: state.anchor,
    tree,
    onDismiss: () => state.setOpen(false),
  });

  const panel = useAnchoredPanel({
    anchor: state.anchor,
    open: state.open,
    side: () => props.side ?? "bottom",
    align: () => props.align ?? "start",
    sideOffset: () => props.sideOffset ?? 6,
    alignOffset: () => props.alignOffset ?? 0,
  });

  return (
    <Show when={state.open()}>
      <Portal>
        <PopoverPanel
          {...rest}
          ref={composeRef(panel.ref, (element: HTMLElement) => {
            registerPanelElement(tree, element);
            props.ref?.(element);
          })}
          {...{ [ANCHORED_PANEL_ATTRIBUTE]: "" }}
          data-slot="popover-content"
          class={cn(
            "bg-popover text-popover-foreground ring-foreground/10 animate-in fade-in-0 zoom-in-95 z-50 overflow-y-auto rounded-lg p-1 shadow-md ring-1 duration-100 outline-none",
            props.class,
          )}
        >
          <PanelTreeContext value={tree}>{props.children}</PanelTreeContext>
        </PopoverPanel>
      </Portal>
    </Show>
  );
}
