<script lang="ts">
  import { cn } from "#lib/utils.ts";

  let {
    value = $bindable(""),
    placeholder = "",
    disabled = false,
    rows = 3,
    minHeight = 76,
    maxHeight = 200,
    class: className = "",
    onkeydown = undefined,
    onpaste = undefined,
    ondrop = undefined,
    ariaLabel = "",
  }: {
    /** Prompt text (two-way bound). */
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
    onkeydown?: (event: KeyboardEvent) => void;
    onpaste?: (event: ClipboardEvent) => void;
    ondrop?: (event: DragEvent) => void;
  } = $props();

  let textarea = $state<HTMLTextAreaElement | null>(null);

  function resize(): void {
    if (!textarea) return;
    textarea.style.height = "auto";
    textarea.style.height = `${Math.min(textarea.scrollHeight, maxHeight)}px`;
  }

  // Keep the height in sync when the value is changed programmatically
  // (for example when a slash command completes the input).
  $effect(() => {
    void value;
    resize();
  });

  function onDragOver(event: DragEvent): void {
    event.preventDefault();
  }
</script>

<textarea
  bind:this={textarea}
  bind:value
  {placeholder}
  aria-label={ariaLabel || placeholder}
  {disabled}
  {rows}
  oninput={resize}
  {onkeydown}
  {onpaste}
  ondragover={onDragOver}
  {ondrop}
  class={cn(
    "placeholder:text-muted-foreground w-full resize-none overflow-y-auto bg-transparent px-2 py-3 text-sm leading-relaxed outline-none disabled:cursor-not-allowed disabled:opacity-50",
    className,
  )}
  style="min-height: {minHeight}px; max-height: {maxHeight}px;"></textarea>
