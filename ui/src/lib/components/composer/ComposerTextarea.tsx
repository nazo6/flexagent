import { createEffect, createSignal } from "solid-js";
import { cn } from "#lib/utils.ts";

export interface ComposerTextareaProps {
  /** Prompt text (owned by the caller). */
  value?: string;
  placeholder?: string;
  /** Accessible name of the textarea (defaults to the placeholder). */
  ariaLabel?: string;
  disabled?: boolean;
  rows?: number;
  /** Minimum height in pixels. */
  minHeight?: number;
  /** Height cap in pixels; the textarea scrolls beyond it. */
  maxHeight?: number;
  class?: string;
  onKeyDown?: (event: KeyboardEvent) => void;
  onPaste?: (event: ClipboardEvent) => void;
  onDrop?: (event: DragEvent) => void;
  /** Called with the edited text (`bind:value` in the Svelte app). */
  onValueChange?: (value: string) => void;
}

/**
 * Auto-growing prompt textarea.
 *
 * The caller owns the value; edits are reported through `onValueChange` and the
 * height follows both edits and programmatic changes (for example when a slash
 * command completes the input).
 */
export default function ComposerTextarea(props: ComposerTextareaProps) {
  const [textarea, setTextarea] = createSignal<HTMLTextAreaElement>();
  const minHeight = () => props.minHeight ?? 76;
  const maxHeight = () => props.maxHeight ?? 200;

  function resize(element: HTMLTextAreaElement | undefined, max: number): void {
    if (element === undefined) return;
    element.style.height = "auto";
    element.style.height = `${Math.min(element.scrollHeight, max)}px`;
  }

  // Keep the height in sync when the value is changed programmatically.
  createEffect(
    () => ({ value: props.value ?? "", element: textarea(), max: maxHeight() }),
    ({ element, max }) => {
      resize(element, max);
    },
  );

  function onDragOver(event: DragEvent): void {
    event.preventDefault();
  }

  return (
    <textarea
      ref={setTextarea}
      value={props.value ?? ""}
      placeholder={props.placeholder ?? ""}
      aria-label={
        props.ariaLabel !== undefined && props.ariaLabel !== ""
          ? props.ariaLabel
          : (props.placeholder ?? "")
      }
      disabled={props.disabled ?? false}
      rows={props.rows ?? 3}
      onInput={(event) => {
        props.onValueChange?.(event.currentTarget.value);
        resize(event.currentTarget, maxHeight());
      }}
      onKeyDown={(event) => props.onKeyDown?.(event)}
      onPaste={(event) => props.onPaste?.(event)}
      onDragOver={onDragOver}
      onDrop={(event) => props.onDrop?.(event)}
      class={cn(
        "placeholder:text-muted-foreground w-full resize-none overflow-y-auto bg-transparent px-2 py-2 text-sm leading-relaxed outline-none disabled:cursor-not-allowed disabled:opacity-50",
        props.class,
      )}
      style={{ "min-height": `${minHeight()}px`, "max-height": `${maxHeight()}px` }}
    />
  );
}
