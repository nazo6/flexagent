import type { ComponentProps } from "@solidjs/web";
import { omit } from "solid-js";
import { cn } from "#lib/utils.ts";
import type { ElementRef } from "../position.ts";

/**
 * Scroll area, our own implementation.
 *
 * bits-ui's scroll area rendered viewport/scrollbar/thumb/corner elements and
 * measured the thumb itself. The Desk UI does not need a custom drag handle,
 * so this uses the browser's scrolling with a styled scrollbar, which keeps
 * momentum, trackpad behaviour and accessibility for free.
 *
 * `Scrollbar`, `Thumb` and `Corner` still render (hidden) elements carrying
 * their `data-slot`, so screen markup and screenshot selectors stay the same.
 */

export type ScrollAreaOrientation = "vertical" | "horizontal" | "both";

const SCROLLBAR_STYLES =
  "[scrollbar-width:thin] [scrollbar-color:var(--line-strong)_transparent] [&::-webkit-scrollbar]:w-2 [&::-webkit-scrollbar]:h-2 [&::-webkit-scrollbar-thumb]:rounded-full [&::-webkit-scrollbar-thumb]:bg-line-strong [&::-webkit-scrollbar-track]:bg-transparent";

const OVERFLOW: Record<ScrollAreaOrientation, string> = {
  vertical: "overflow-x-hidden overflow-y-auto",
  horizontal: "overflow-y-hidden overflow-x-auto",
  both: "overflow-auto",
};

export interface RootProps extends ComponentProps<"div"> {
  orientation?: ScrollAreaOrientation;
  /** Svelte callers wrote `bind:viewportRef`; this is the setter half. */
  viewportRef?: ElementRef;
  scrollbarXClasses?: string;
  scrollbarYClasses?: string;
}

export function Root(props: RootProps) {
  const rest = omit(
    props,
    "class",
    "children",
    "orientation",
    "viewportRef",
    "scrollbarXClasses",
    "scrollbarYClasses",
  );
  const orientation = () => props.orientation ?? "vertical";

  return (
    <div {...rest} data-slot="scroll-area" class={cn("relative", props.class)}>
      <Viewport orientation={orientation()} ref={props.viewportRef}>
        {props.children}
      </Viewport>
      {orientation() === "vertical" || orientation() === "both" ? (
        <Scrollbar orientation="vertical" class={props.scrollbarYClasses} />
      ) : null}
      {orientation() === "horizontal" || orientation() === "both" ? (
        <Scrollbar orientation="horizontal" class={props.scrollbarXClasses} />
      ) : null}
      <Corner />
    </div>
  );
}

export interface ViewportProps extends Omit<ComponentProps<"div">, "ref"> {
  ref?: ElementRef;
  orientation?: ScrollAreaOrientation;
}

export function Viewport(props: ViewportProps) {
  const rest = omit(props, "class", "children", "ref", "orientation");
  return (
    <div
      {...rest}
      ref={(element) => props.ref?.(element)}
      data-slot="scroll-area-viewport"
      class={cn(
        "size-full rounded-[inherit] transition-[color,box-shadow] outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50 focus-visible:outline-1",
        SCROLLBAR_STYLES,
        OVERFLOW[props.orientation ?? "vertical"],
        props.class,
      )}
    >
      {props.children}
    </div>
  );
}

export interface ScrollbarProps extends ComponentProps<"div"> {
  orientation?: "vertical" | "horizontal";
}

/** The native scrollbar is always visible; this part is structural. */
export function Scrollbar(props: ScrollbarProps) {
  const rest = omit(props, "class", "children", "orientation");
  return (
    <div
      {...rest}
      data-slot="scroll-area-scrollbar"
      data-orientation={props.orientation ?? "vertical"}
      class={cn("hidden", props.class)}
      aria-hidden="true"
    >
      {props.children}
    </div>
  );
}

export type ThumbProps = ComponentProps<"div">;

export function Thumb(props: ThumbProps) {
  const rest = omit(props, "class", "children");
  return (
    <div
      {...rest}
      data-slot="scroll-area-thumb"
      class={cn("rounded-full relative flex-1 bg-border", props.class)}
      aria-hidden="true"
    />
  );
}

export type CornerProps = ComponentProps<"div">;

export function Corner(props: CornerProps) {
  const rest = omit(props, "class", "children");
  return <div {...rest} data-slot="scroll-area-corner" class={cn("hidden", props.class)} />;
}
