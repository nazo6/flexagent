import type { ComponentProps } from "@solidjs/web";
import { omit } from "solid-js";
import { cn } from "#lib/utils.ts";

/**
 * Separator, ported from `ui/src/lib/components/ui/separator`.
 *
 * bits-ui's root was a `<div role="separator">`; that is what we render.
 */

export interface SeparatorProps extends ComponentProps<"div"> {
  orientation?: "horizontal" | "vertical";
  /** Render as a decorative element that assistive tech ignores. */
  decorative?: boolean;
}

export function Root(props: SeparatorProps) {
  const rest = omit(props, "class", "children", "orientation", "decorative", "data-slot");
  const orientation = () => props.orientation ?? "horizontal";
  return (
    <div
      {...rest}
      data-slot={props["data-slot"] ?? "separator"}
      data-orientation={orientation()}
      role={props.decorative ? "presentation" : "separator"}
      aria-orientation={props.decorative ? undefined : orientation()}
      class={cn(
        "shrink-0 bg-border data-[orientation=horizontal]:h-px data-[orientation=horizontal]:w-full data-[orientation=vertical]:w-px data-[orientation=vertical]:self-stretch",
        props.class,
      )}
    />
  );
}

export { Root as Separator };
