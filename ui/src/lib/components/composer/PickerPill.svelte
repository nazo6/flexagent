<script lang="ts" module>
  let pickerInstance = 0;
</script>

<script lang="ts">
  import * as Drawer from "#lib/components/ui/drawer/index.ts";
  import * as Popover from "#lib/components/ui/popover/index.ts";
  import { isCoarsePointer } from "#lib/pointer.ts";
  import { mobileLayout } from "#lib/media.svelte.ts";
  import { registerOverlay } from "#lib/overlays.ts";
  import { cn } from "#lib/utils.ts";
  import ChevronDownIcon from "@lucide/svelte/icons/chevron-down";
  import LoaderCircleIcon from "@lucide/svelte/icons/loader-circle";
  import type { Snippet } from "svelte";

  let {
    label,
    value = "",
    icon = undefined,
    disabled = false,
    loading = false,
    class: className = "",
    contentClass = "",
    open = $bindable(false),
    onopenchange = undefined,
    children,
  }: {
    /** Accessible name (for example "Project" or "Model"). */
    label: string;
    /** Current value shown in the trigger. */
    value?: string;
    /** Optional leading icon snippet. */
    icon?: Snippet;
    disabled?: boolean;
    loading?: boolean;
    class?: string;
    contentClass?: string;
    open?: boolean;
    /** Called whenever the popover opens or closes. */
    onopenchange?: (open: boolean) => void;
    children?: Snippet;
  } = $props();

  const overlayId = `picker-${++pickerInstance}`;
  const title = $derived(value === "" ? label : `${label}: ${value}`);
  const triggerClass = $derived(
    cn(
      "border-border bg-chip hover:bg-chip-hover hover:text-foreground focus-visible:ring-ring/40 touch-target motion-press inline-flex h-7 min-w-0 max-w-60 items-center gap-1.5 rounded-full border px-2.5 text-xs max-lg:text-[13px] outline-none focus-visible:ring-2 disabled:pointer-events-none disabled:opacity-50",
      className,
    ),
  );

  function setOpen(next: boolean): void {
    open = next;
    onopenchange?.(next);
  }

  // A bottom sheet on phones is easier to reach and dismiss than a popover
  // anchored to a small pill. Back closes it like any other sheet.
  $effect(() => {
    if (!mobileLayout.matches || !open) return;
    return registerOverlay(overlayId, () => setOpen(false));
  });
</script>

{#snippet triggerContent()}
  {@render icon?.()}
  {#if value === ""}
    <span class="text-muted-foreground truncate">{label}</span>
  {:else}
    <span class="truncate">{value}</span>
  {/if}
  {#if loading}
    <LoaderCircleIcon class="text-muted-foreground size-3 shrink-0 animate-spin" />
  {:else}
    <ChevronDownIcon class="text-muted-foreground size-3 shrink-0" />
  {/if}
{/snippet}

{#if mobileLayout.matches}
  <button
    type="button"
    class={triggerClass}
    {title}
    aria-label={title}
    aria-haspopup="dialog"
    aria-expanded={open}
    {disabled}
    onclick={() => setOpen(true)}
  >
    {@render triggerContent()}
  </button>
  <Drawer.Root bind:open onOpenChange={setOpen} shouldScaleBackground={false}>
    <Drawer.Content class="max-h-[85dvh] gap-0">
      <Drawer.Header class="px-4 pt-2 pb-3">
        <Drawer.Title class="text-sm font-semibold">{label}</Drawer.Title>
        <Drawer.Description class="sr-only">{label}</Drawer.Description>
      </Drawer.Header>
      <div
        class="min-h-0 flex-1 overflow-y-auto px-2"
        style="padding-bottom: max(1rem, var(--safe-bottom));"
      >
        {@render children?.()}
      </div>
    </Drawer.Content>
  </Drawer.Root>
{:else}
  <Popover.Root bind:open onOpenChange={onopenchange}>
    <Popover.Trigger
      {disabled}
      class={triggerClass}
      {title}
      aria-label={title}
      aria-haspopup="dialog"
    >
      {@render triggerContent()}
    </Popover.Trigger>
    <Popover.Content
      class={cn("w-72", contentClass)}
      onOpenAutoFocus={(event) => {
        // Touch devices focus the first tabbable element (the search field for long
        // option lists), which pops up the on-screen keyboard. Keep the focus on the
        // trigger there and let the user tap the search field when they want to.
        if (isCoarsePointer()) event.preventDefault();
      }}
    >
      {@render children?.()}
    </Popover.Content>
  </Popover.Root>
{/if}
