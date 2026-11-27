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
    onvaluechange = undefined,
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
    /** Called with the new value after the user edits the textarea. */
    onvaluechange?: (value: string) => void;
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

  function handleInput(event: Event): void {
    resize();
    onvaluechange?.((event.currentTarget as HTMLTextAreaElement).value);
  }

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
  oninput={handleInput}
  {onkeydown}
  {onpaste}
  ondragover={onDragOver}
  {ondrop}
  class={cn(
    "placeholder:text-muted-foreground w-full resize-none overflow-y-auto bg-transparent px-1.5 py-1.5 text-base leading-relaxed outline-none disabled:cursor-not-allowed disabled:opacity-50 lg:text-[14px]",
    className,
  )}
  style="min-height: {minHeight}px; max-height: {maxHeight}px;"></textarea>
