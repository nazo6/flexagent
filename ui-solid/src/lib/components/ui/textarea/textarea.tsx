import type { ComponentProps } from "@solidjs/web";
import { omit } from "solid-js";
import { cn } from "#lib/utils.ts";
import type { ElementRef } from "../position.ts";
import type { InputHandler } from "../controllable.ts";

/** Textarea, ported from `ui/src/lib/components/ui/textarea`. */

export interface TextareaProps extends Omit<
  ComponentProps<"textarea">,
  "value" | "ref" | "onInput"
> {
  value?: string;
  onValueChange?: (value: string) => void;
  onInput?: InputHandler<HTMLTextAreaElement>;
  ref?: ElementRef<HTMLTextAreaElement>;
}

export function Root(props: TextareaProps) {
  const rest = omit(props, "class", "value", "onValueChange", "ref");

  const handleInput: InputHandler<HTMLTextAreaElement> = (event) => {
    props.onValueChange?.(event.currentTarget.value);
    props.onInput?.(event);
  };

  return (
    <textarea
      {...rest}
      ref={(element) => props.ref?.(element)}
      data-slot="textarea"
      class={cn(
        "bg-surface-raised border-input focus-visible:border-ring focus-visible:ring-ring/50 aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40 aria-invalid:border-destructive dark:aria-invalid:border-destructive/50 rounded-md border px-2.5 py-2 text-base shadow-xs transition-[color,box-shadow] focus-visible:ring-3 aria-invalid:ring-3 md:text-sm flex field-sizing-content min-h-16 w-full outline-none placeholder:text-muted-foreground disabled:cursor-not-allowed disabled:opacity-50",
        props.class,
      )}
      value={props.value ?? ""}
      onInput={handleInput}
    />
  );
}

export { Root as Textarea };
