import type { ComponentProps, JSX } from "@solidjs/web";
import type { ClassValue } from "cn";
import { Portal } from "@solidjs/web";
import { createContext, createSignal, omit, Show, useContext } from "solid-js";
import { Tooltip as TooltipPrimitive, TooltipPanel, TooltipTrigger } from "terracotta/tooltip";
import { cn } from "#lib/utils.ts";
import { createControllable, toBoolean, type MouseHandler } from "../controllable.ts";
import { composeRef, useAnchoredPanel, type ElementRef, type PanelAlign } from "../position.ts";
import { TooltipConfigContext } from "./tooltip-context.ts";

/**
 * Tooltip, ported from `ui/src/lib/components/ui/tooltip`.
 *
 * The panel is portaled and positioned against the trigger; the arrow is
 * placed by `useAnchoredPanel`. `Provider` only carries the open delay, which
 * is what the sidebar uses (`delayDuration={400}`).
 */

interface TooltipState {
  open: () => boolean;
  setOpen: (open: boolean) => void;
  anchor: () => HTMLElement | undefined;
  setAnchor: (element: HTMLElement) => void;
}

const TooltipContext = createContext<TooltipState | null>(null);

function useTooltip(component: string): TooltipState {
  const state = useContext(TooltipContext);
  if (!state) throw new Error(`<${component}> must be used inside a <Tooltip.Root>`);
  return state;
}

export interface ProviderProps {
  delayDuration?: number;
  children?: JSX.Element;
}

export function Provider(props: ProviderProps) {
  return (
    <TooltipConfigContext value={{ delayDuration: () => props.delayDuration ?? 0 }}>
      {props.children}
    </TooltipConfigContext>
  );
}

export interface RootProps {
  open?: boolean;
  defaultOpen?: boolean;
  onOpenChange?: (open: boolean) => void;
  /** Overrides the provider's delay for this tooltip. */
  openDelay?: number;
  closeDelay?: number;
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
  const delay = useContext(TooltipConfigContext);

  return (
    <TooltipContext
      value={{ open, setOpen, anchor, setAnchor: (el) => setAnchorElement(() => el) }}
    >
      <TooltipPrimitive
        isOpen={open()}
        onChange={setOpen}
        openDelay={props.openDelay ?? delay?.delayDuration()}
        closeDelay={props.closeDelay}
        disabled={props.disabled}
      >
        {props.children}
      </TooltipPrimitive>
    </TooltipContext>
  );
}

export interface TriggerProps extends Omit<
  ComponentProps<"button">,
  "ref" | "onClick" | "type" | "class"
> {
  /** Render a link instead of a button (the sidebar wraps anchors). */
  as?: "button" | "a";
  ref?: ElementRef<HTMLElement>;
  class?: ClassValue;
  onClick?: MouseHandler<HTMLElement>;
  type?: "button" | "submit" | "reset";
  href?: string;
  target?: string;
  rel?: string;
}

export function Trigger(props: TriggerProps) {
  const state = useTooltip("Tooltip.Trigger");
  // Resolve both refs when the element mounts: reading `props.ref` at the top
  // level makes a JSX lazily-built ref callback look like a reactive source.
  const setRef = (element: HTMLElement) => {
    props.ref?.(element);
    state.setAnchor(element);
  };

  return (
    <Show
      when={props.as === "a"}
      fallback={
        <TooltipTrigger
          {...omit(
            props as ComponentProps<"button"> & {
              as?: "button" | "a";
              href?: string;
              target?: string;
              rel?: string;
            },
            "class",
            "children",
            "ref",
            "as",
            "disabled",
            "type",
            "onClick",
            "href",
            "target",
            "rel",
          )}
          ref={setRef}
          data-slot="tooltip-trigger"
          type={props.type ?? "button"}
          disabled={props.disabled === undefined ? undefined : toBoolean(props.disabled)}
          class={cn(props.class)}
          onClick={props.onClick}
        >
          {props.children}
        </TooltipTrigger>
      }
    >
      <TooltipTrigger
        as="a"
        {...omit(
          props as ComponentProps<"a"> & { as?: "button" | "a"; disabled?: boolean },
          "class",
          "children",
          "ref",
          "as",
          "disabled",
          "type",
          "onClick",
        )}
        ref={setRef}
        data-slot="tooltip-trigger"
        class={cn(props.class)}
        onClick={props.onClick}
      >
        {props.children}
      </TooltipTrigger>
    </Show>
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
  arrowClasses?: string;
}

export function Content(props: ContentProps) {
  const state = useTooltip("Tooltip.Content");
  const [arrow, setArrow] = createSignal<HTMLElement>();
  const rest = omit(
    props,
    "class",
    "children",
    "ref",
    "side",
    "align",
    "sideOffset",
    "arrowClasses",
  );

  const panel = useAnchoredPanel({
    anchor: state.anchor,
    open: state.open,
    side: () => props.side ?? "top",
    align: () => props.align ?? "center",
    sideOffset: () => props.sideOffset ?? 0,
    arrow,
  });

  return (
    <Show when={state.open()}>
      <Portal>
        <TooltipPanel
          {...rest}
          ref={composeRef(panel.ref, props.ref)}
          data-slot="tooltip-content"
          class={cn(
            "animate-in fade-in-0 zoom-in-95 data-[side=bottom]:slide-in-from-top-2 data-[side=left]:slide-in-from-right-2 data-[side=right]:slide-in-from-left-2 data-[side=top]:slide-in-from-bottom-2 inline-flex items-center gap-1.5 rounded-md px-3 py-1.5 text-xs has-data-[slot=kbd]:pr-1.5 **:data-[slot=kbd]:relative **:data-[slot=kbd]:isolate **:data-[slot=kbd]:z-50 **:data-[slot=kbd]:rounded-sm z-50 w-fit max-w-xs bg-foreground text-background",
            props.class,
          )}
        >
          {props.children}
          <div
            ref={setArrow}
            class={cn(
              "size-2.5 rotate-45 rounded-[2px] bg-foreground fill-foreground",
              props.arrowClasses,
            )}
          />
        </TooltipPanel>
      </Portal>
    </Show>
  );
}
