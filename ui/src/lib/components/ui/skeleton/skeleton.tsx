import type { ComponentProps } from "@solidjs/web";
import { omit } from "solid-js";
import { cn } from "#lib/utils.ts";

/** Skeleton, ported from `ui/src/lib/components/ui/skeleton`. */

export type SkeletonProps = ComponentProps<"div">;

export function Root(props: SkeletonProps) {
  const rest = omit(props, "class", "children");
  return (
    <div
      {...rest}
      data-slot="skeleton"
      class={cn("bg-muted rounded-md animate-pulse", props.class)}
    />
  );
}

export { Root as Skeleton };
