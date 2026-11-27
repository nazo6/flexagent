<script lang="ts" module>
  let pickerInstance = 0;
</script>

<script lang="ts">
  import type { Snippet } from "svelte";
  import * as Drawer from "#lib/components/ui/drawer/index.ts";
  import * as Popover from "#lib/components/ui/popover/index.ts";
  import { isCoarsePointer } from "#lib/pointer.ts";
  import { registerOverlay } from "#lib/overlays.ts";
  import { uiTraits } from "#lib/ui-traits.svelte.ts";
  import { cn } from "#lib/utils.ts";

  interface Props {
    /** Accessible name (for example "Project" or "Model"). */
    label: string;
    /** Trigger label, usually `label: value`. */
    title: string;
    triggerClass: string;
    contentClass?: string;
    disabled?: boolean;
    open?: boolean;
    /** Called whenever the picker opens or closes. */
    onopenchange?: (open: boolean) => void;
    /** Trigger content; the primitive owns the button and the surface. */
    trigger: Snippet;
    children?: Snippet;
  }

  let {
    label,
    title,
    triggerClass,
    contentClass = "",
    disabled = false,
    open = $bindable(false),
    onopenchange = undefined,
    trigger,
    children,
  }: Props = $props();

  pickerInstance += 1;
  const overlayId = `picker-${pickerInstance}`;

  function setOpen(next: boolean): void {
    open = next;
    onopenchange?.(next);
  }

  // A bottom sheet on phones is easier to reach and dismiss than a popover
  // anchored to a small pill. A narrow desktop window keeps the popover, so
  // this follows the input model, not the window width alone.
  $effect(() => {
    if (!uiTraits.sheetPickers || !open) return;
    return registerOverlay(overlayId, () => setOpen(false));
  });
</script>

{#if uiTraits.sheetPickers}
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
    {@render trigger()}
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
      {@render trigger()}
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
