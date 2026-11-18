import type { ComponentProps } from "@solidjs/web";
import { omit } from "solid-js";
import { cn } from "#lib/utils.ts";

/**
 * Label, ported from `ui/src/lib/components/ui/label`.
 *
 * bits-ui's Label only added "clicking it focuses the control"; the browser
 * does that for a `<label>` with a `for`, so this is a plain element.
 */

export type LabelProps = ComponentProps<"label">;

export function Root(props: LabelProps) {
  const rest = omit(props, "class", "children");
  return (
    <label
      {...rest}
      data-slot="label"
      class={cn(
        "gap-2 text-sm leading-none font-medium group-data-[disabled=true]:opacity-50 peer-disabled:opacity-50 flex items-center select-none group-data-[disabled=true]:pointer-events-none peer-disabled:cursor-not-allowed",
        props.class,
      )}
    >
      {props.children}
    </label>
  );
}

export { Root as Label };
