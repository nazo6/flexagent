<script lang="ts">
  import { m } from "#lib/paraglide/messages.js";
  import { problems } from "#lib/stores/problems.svelte.ts";
  import { cn } from "#lib/utils.ts";
  import CircleAlertIcon from "@lucide/svelte/icons/circle-alert";
  import TriangleAlertIcon from "@lucide/svelte/icons/triangle-alert";

  interface Props {
    /** Show the text label next to the count. */
    label?: boolean;
    class?: string;
  }

  let { label = false, class: className = "" }: Props = $props();

  const count = $derived(problems.problems.length);
  const hasError = $derived(problems.problems.some((problem) => problem.severity === "error"));
</script>

{#if count > 0}
  <button
    type="button"
    class={cn(
      "inline-flex items-center justify-center gap-1.5 rounded-md border px-2 py-1 text-[11px] font-medium transition-colors",
      hasError
        ? "border-danger/30 bg-danger-soft/60 text-danger hover:bg-danger-soft"
        : "border-warn/30 bg-warn-soft/60 text-warn hover:bg-warn-soft",
      className,
    )}
    title={m.problem_banner_label()}
    aria-label={`${m.problem_banner_label()} (${count})`}
    onclick={() => problems.openCenter()}
  >
    {#if hasError}
      <CircleAlertIcon class="size-3.5 shrink-0" />
    {:else}
      <TriangleAlertIcon class="size-3.5 shrink-0" />
    {/if}
    <span class="font-mono leading-none">{count}</span>
    {#if label}
      <span class="truncate">{m.problem_banner_label()}</span>
    {/if}
  </button>
{/if}
