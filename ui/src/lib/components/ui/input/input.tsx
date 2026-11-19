import type { ComponentProps } from "@solidjs/web";
import { omit } from "solid-js";
import { cn } from "#lib/utils.ts";
import type { ElementRef } from "../position.ts";
import type { InputHandler } from "../controllable.ts";

/**
 * Input, ported from `ui/src/lib/components/ui/input`.
 *
 * Svelte callers wrote `bind:value`; the Solid port writes `value` +
 * `onValueChange` (or a native `onInput`). Both work: the native handler is
 * forwarded untouched, and `onValueChange` receives the string value.
 */

export interface InputProps extends Omit<ComponentProps<"input">, "value" | "ref" | "onInput"> {
  value?: string | number;
  onValueChange?: (value: string) => void;
  onInput?: InputHandler<HTMLInputElement>;
  ref?: ElementRef<HTMLInputElement>;
}

export function Root(props: InputProps) {
  const rest = omit(props, "class", "value", "onValueChange", "ref");

  const handleInput: InputHandler<HTMLInputElement> = (event) => {
    props.onValueChange?.(event.currentTarget.value);
    props.onInput?.(event);
  };

  return (
    <input
      {...rest}
      {...(props.value !== undefined ? { value: props.value } : {})}
      ref={(element) => props.ref?.(element)}
      data-slot="input"
      class={cn(
        "bg-surface-raised border-input focus-visible:border-ring focus-visible:ring-ring/50 aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40 aria-invalid:border-destructive dark:aria-invalid:border-destructive/50 h-9 rounded-md border px-2.5 py-1 text-base shadow-xs transition-[color,box-shadow] file:h-7 file:text-sm file:font-medium focus-visible:ring-3 aria-invalid:ring-3 md:text-sm w-full min-w-0 outline-none file:inline-flex file:border-0 file:bg-transparent file:text-foreground placeholder:text-muted-foreground disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-50",
        props.class,
      )}
      onInput={handleInput}
    />
  );
}

export { Root as Input };
