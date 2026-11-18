import type { ComponentProps, JSX } from "@solidjs/web";
import { omit, Show } from "solid-js";
import { cn } from "#lib/utils.ts";
import type { ElementRef } from "../position.ts";

/**
 * Badge, ported from `ui/src/lib/components/ui/badge`.
 *
 * Renders a `<span>`, or an `<a>` when `href` is given.
 */

const base =
  "h-5 gap-1 rounded-4xl border border-transparent px-2 py-0.5 text-xs font-medium transition-all has-data-[icon=inline-end]:pr-1.5 has-data-[icon=inline-start]:pl-1.5 [&>svg]:size-3! group/badge inline-flex w-fit shrink-0 items-center justify-center overflow-hidden whitespace-nowrap focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 aria-invalid:border-destructive aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40 [&>svg]:pointer-events-none";

const variants = {
  default: "bg-primary text-primary-foreground [a]:hover:bg-primary/80",
  secondary: "bg-secondary text-secondary-foreground [a]:hover:bg-secondary/80",
  destructive:
    "bg-destructive/10 [a]:hover:bg-destructive/20 focus-visible:ring-destructive/20 dark:focus-visible:ring-destructive/40 text-destructive dark:bg-destructive/20",
  outline: "border-border text-foreground [a]:hover:bg-muted [a]:hover:text-muted-foreground",
  ghost: "hover:bg-muted hover:text-muted-foreground dark:hover:bg-muted/50",
  link: "text-primary underline-offset-4 hover:underline",
} as const;

export type BadgeVariant = keyof typeof variants;

export interface BadgeVariantsOptions {
  variant?: BadgeVariant;
}

/** The class list for a badge, without the caller's overrides. */
export function badgeVariants(options: BadgeVariantsOptions = {}): string {
  return `${base} ${variants[options.variant ?? "default"]}`;
}

interface BadgeCommon {
  variant?: BadgeVariant;
  class?: string;
  ref?: ElementRef;
  children?: JSX.Element;
}

export type BadgeProps =
  | (BadgeCommon & { href?: undefined } & Omit<ComponentProps<"span">, "class" | "children">)
  | (BadgeCommon & { href: string } & Omit<ComponentProps<"a">, "class" | "children">);

export function Root(props: BadgeProps) {
  const className = () => cn(badgeVariants({ variant: props.variant }), props.class);
  const spanRest = () =>
    omit(
      props as ComponentProps<"span"> & BadgeCommon & { href?: string },
      "variant",
      "class",
      "children",
      "href",
      "ref",
    );
  const anchorRest = () =>
    omit(
      props as ComponentProps<"a"> & BadgeCommon & { href?: string },
      "variant",
      "class",
      "children",
      "href",
      "ref",
    );
  const setRef = (element: HTMLElement) => props.ref?.(element);

  return (
    <Show
      when={props.href}
      fallback={
        <span {...spanRest()} ref={setRef} data-slot="badge" class={className()}>
          {props.children}
        </span>
      }
    >
      <a {...anchorRest()} ref={setRef} data-slot="badge" class={className()} href={props.href}>
        {props.children}
      </a>
    </Show>
  );
}

export { Root as Badge };
