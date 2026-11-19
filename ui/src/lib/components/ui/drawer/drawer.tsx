import type { ComponentProps } from "@solidjs/web";
import { cn } from "#lib/utils.ts";
import * as Sheet from "../sheet/sheet.tsx";

/**
 * Drawer, ported from `ui/src/lib/components/ui/drawer`.
 *
 * `vaul-svelte` is a Svelte-only package, so the drawer is the sheet surface
 * with vaul's spacing and grab handle: same `Dialog` behaviour underneath.
 * Snap points and drag-to-dismiss are not reproduced — the drawer opens,
 * closes, and traps focus like the sheet.
 */

export const Root = Sheet.Root;
/** vaul's nested root only shared state with its parent; `Root` covers it. */
export const NestedRoot = Sheet.Root;
export const Portal = Sheet.PortalRoot;

export interface TriggerProps extends Omit<Sheet.TriggerProps, "dataSlot"> {}

export function Trigger(props: TriggerProps) {
  return <Sheet.Trigger {...props} dataSlot="drawer-trigger" />;
}

export interface OverlayProps extends Omit<Sheet.OverlayProps, "dataSlot"> {}

export function Overlay(props: OverlayProps) {
  return <Sheet.Overlay {...props} dataSlot="drawer-overlay" />;
}

export interface ContentProps extends Omit<Sheet.ContentProps, "dataSlot" | "overlayDataSlot"> {}

export function Content(props: ContentProps) {
  return (
    <Sheet.Content
      handle
      {...props}
      side={props.side ?? "bottom"}
      dataSlot="drawer-content"
      overlayDataSlot="drawer-overlay"
      class={cn(
        "h-auto group/drawer-content data-[side=bottom]:rounded-t-xl data-[side=bottom]:border-t data-[side=top]:rounded-b-xl data-[side=top]:border-b data-[side=left]:rounded-r-xl data-[side=left]:border-r data-[side=right]:rounded-l-xl data-[side=right]:border-l data-[side=bottom]:max-h-[80vh] data-[side=top]:max-h-[80vh]",
        props.class,
      )}
    />
  );
}

export interface HeaderProps extends Omit<ComponentProps<"div">, "class"> {
  class?: string;
}

export function Header(props: HeaderProps) {
  return (
    <Sheet.Header
      {...props}
      dataSlot="drawer-header"
      class={cn(
        "gap-0.5 p-4 md:gap-1.5 md:text-left group-data-[side=bottom]/drawer-content:text-center group-data-[side=top]/drawer-content:text-center",
        props.class,
      )}
    />
  );
}

export type FooterProps = ComponentProps<"div">;

export function Footer(props: FooterProps) {
  return <Sheet.Footer {...props} dataSlot="drawer-footer" />;
}

export type TitleProps = Omit<Sheet.TitleProps, "dataSlot">;

export function Title(props: TitleProps) {
  return <Sheet.Title {...props} dataSlot="drawer-title" />;
}

export type DescriptionProps = Omit<Sheet.DescriptionProps, "dataSlot">;

export function Description(props: DescriptionProps) {
  return <Sheet.Description {...props} dataSlot="drawer-description" />;
}

export interface CloseProps extends Omit<Sheet.CloseProps, "dataSlot"> {}

export function Close(props: CloseProps) {
  return <Sheet.Close {...props} dataSlot="drawer-close" />;
}

// The part props are inherited from the sheet implementation.
export type RootProps = Sheet.RootProps;
export type PortalProps = Sheet.PortalProps;
export type { Side } from "../sheet/sheet.tsx";
