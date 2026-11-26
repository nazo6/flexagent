<script lang="ts">
  import { Button } from "#lib/components/ui/button/index.ts";
  import PaperclipIcon from "@lucide/svelte/icons/paperclip";

  let {
    label,
    onfiles,
  }: {
    /** Accessible name / title of the button and its file input. */
    label: string;
    /** Called with the picked images; the input is cleared so the same file can be re-added. */
    onfiles: (files: FileList) => void;
  } = $props();

  let input = $state<HTMLInputElement | null>(null);

  function onchange(event: Event): void {
    const target = event.currentTarget as HTMLInputElement;
    if (target.files !== null && target.files.length > 0) onfiles(target.files);
    target.value = "";
  }
</script>

<input
  bind:this={input}
  type="file"
  accept="image/*"
  multiple
  class="hidden"
  aria-label={label}
  {onchange}
/>
<Button
  variant="ghost"
  size="icon-sm"
  class="touch-target-lg rounded-full"
  title={label}
  aria-label={label}
  onclick={() => input?.click()}
>
  <PaperclipIcon class="size-4" />
</Button>
