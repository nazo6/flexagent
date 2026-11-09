<script lang="ts">
  import { Button } from "#lib/components/ui/button/index.ts";
  import ArrowUpIcon from "@lucide/svelte/icons/arrow-up";
  import LoaderCircleIcon from "@lucide/svelte/icons/loader-circle";

  let {
    busy = false,
    disabled = false,
    title,
    label = title,
    onclick,
  }: {
    /** Shows a spinner while the prompt is being submitted. */
    busy?: boolean;
    disabled?: boolean;
    /** Tooltip (may explain why the button is disabled). */
    title: string;
    /** Accessible name; defaults to the tooltip. */
    label?: string;
    onclick: () => void;
  } = $props();
</script>

<Button size="icon-sm" class="rounded-full" {disabled} {title} aria-label={label} {onclick}>
  {#if busy}
    <LoaderCircleIcon class="size-4 animate-spin" />
  {:else}
    <ArrowUpIcon class="size-4" />
  {/if}
</Button>
