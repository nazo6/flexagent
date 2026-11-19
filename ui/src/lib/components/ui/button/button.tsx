import type { ComponentProps, JSX } from "@solidjs/web";
import type { ClassValue } from "cn";
import { omit, Show } from "solid-js";
import { cn } from "#lib/utils.ts";
import type { ElementRef } from "../position.ts";

/**
 * Button, ported from `ui/src/lib/components/ui/button`.
 *
 * Variant names and classes are unchanged; the only difference is that the
 * component decides between `<button>` and `<a>` up front, as the Svelte one
 * did.
 */

const base =
  "focus-visible:border-ring focus-visible:ring-ring/50 aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40 aria-invalid:border-destructive dark:aria-invalid:border-destructive/50 rounded-md border border-transparent bg-clip-padding text-sm font-medium focus-visible:ring-3 aria-invalid:ring-3 active:not-aria-[haspopup]:translate-y-px aria-disabled:pointer-events-none aria-disabled:opacity-50 [&_svg:not([class*='size-'])]:size-4 group/button inline-flex shrink-0 items-center justify-center whitespace-nowrap transition-all outline-none select-none disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0";

const variants = {
  default: "bg-primary text-primary-foreground hover:bg-primary/80",
  outline:
    "border-border bg-background hover:bg-muted hover:text-foreground dark:bg-input/30 dark:border-input dark:hover:bg-input/50 aria-expanded:bg-muted aria-expanded:text-foreground",
  secondary:
    "bg-secondary text-secondary-foreground hover:bg-[color-mix(in_oklch,var(--secondary),var(--foreground)_5%)] aria-expanded:bg-secondary aria-expanded:text-secondary-foreground",
  ghost:
    "hover:bg-muted hover:text-foreground dark:hover:bg-muted/50 aria-expanded:bg-muted aria-expanded:text-foreground",
  destructive:
    "bg-destructive/10 hover:bg-destructive/20 focus-visible:ring-destructive/20 dark:focus-visible:ring-destructive/40 dark:bg-destructive/20 text-destructive focus-visible:border-destructive/40 dark:hover:bg-destructive/30",
  link: "text-primary underline-offset-4 hover:underline",
} as const;

const sizes = {
  default:
    "h-8 gap-1.5 px-2.5 text-[13px] in-data-[slot=button-group]:rounded-md has-data-[icon=inline-end]:pr-2 has-data-[icon=inline-start]:pl-2",
  xs: "h-6 gap-1 rounded-[min(var(--radius-md),8px)] px-2 text-xs in-data-[slot=button-group]:rounded-md has-data-[icon=inline-end]:pr-1.5 has-data-[icon=inline-start]:pl-1.5 [&_svg:not([class*='size-'])]:size-3",
  sm: "h-7 gap-1 rounded-[min(var(--radius-md),10px)] px-2.5 text-[13px] in-data-[slot=button-group]:rounded-md has-data-[icon=inline-end]:pr-1.5 has-data-[icon=inline-start]:pl-1.5",
  lg: "h-9 gap-1.5 px-2.5 text-[13px] has-data-[icon=inline-end]:pr-2 has-data-[icon=inline-start]:pl-2",
  icon: "size-8",
  "icon-xs":
    "size-6 rounded-[min(var(--radius-md),8px)] in-data-[slot=button-group]:rounded-md [&_svg:not([class*='size-'])]:size-3",
  "icon-sm": "size-7 rounded-[min(var(--radius-md),10px)] in-data-[slot=button-group]:rounded-md",
  "icon-lg": "size-9",
} as const;

export type ButtonVariant = keyof typeof variants;
export type ButtonSize = keyof typeof sizes;

export interface ButtonVariantsOptions {
  variant?: ButtonVariant;
  size?: ButtonSize;
}

/** The class list for a button, without the caller's overrides. */
export function buttonVariants(options: ButtonVariantsOptions = {}): string {
  return `${base} ${variants[options.variant ?? "default"]} ${sizes[options.size ?? "default"]}`;
}

interface ButtonCommon {
  variant?: ButtonVariant;
  size?: ButtonSize;
  class?: ClassValue;
  ref?: ElementRef;
  /** Disables the button, or removes the link and its tab stop for `<a>`. */
  disabled?: boolean;
  children?: JSX.Element;
}

export type ButtonProps =
  | (ButtonCommon & {
      href?: undefined;
      type?: "button" | "submit" | "reset";
    } & Omit<ComponentProps<"button">, "class" | "children" | "ref" | "type">)
  | (ButtonCommon & {
      href: string;
    } & Omit<ComponentProps<"a">, "class" | "children" | "ref" | "type">);

export function Root(props: ButtonProps) {
  const className = () =>
    cn(buttonVariants({ variant: props.variant, size: props.size }), props.class);

  const setRef = (element: HTMLElement) => props.ref?.(element);
  const disabled = () => (props as Omit<ComponentProps<"button">, "class">).disabled;
  const type = () => (props as { type?: "button" | "submit" | "reset" }).type;

  return (
    <Show
      when={props.href !== undefined}
      fallback={
        <button
          {...omit(
            props as ButtonCommon & ComponentProps<"button">,
            "variant",
            "size",
            "class",
            "children",
            "ref",
            "type",
          )}
          ref={setRef}
          data-slot="button"
          class={className()}
          type={type() ?? "button"}
        >
          {props.children}
        </button>
      }
    >
      <a
        {...omit(
          props as ButtonCommon & ComponentProps<"a"> & { disabled?: boolean },
          "variant",
          "size",
          "class",
          "children",
          "ref",
          "type",
          "disabled",
        )}
        {...(disabled() ? { role: "link", tabindex: -1 } : {})}
        ref={setRef}
        data-slot="button"
        class={className()}
        href={disabled() ? undefined : props.href}
        aria-disabled={disabled() ? "true" : undefined}
      >
        {props.children}
      </a>
    </Show>
  );
}

export { Root as Button };
