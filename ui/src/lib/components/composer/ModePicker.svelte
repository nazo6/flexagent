<script lang="ts">
  import PickerItem from "./PickerItem.svelte";
  import PickerPill from "./PickerPill.svelte";
  import type { ModeInfo } from "#lib/generated/bindings.ts";
  import { m } from "#lib/paraglide/messages.js";
  import SlidersHorizontalIcon from "@lucide/svelte/icons/sliders-horizontal";

  let {
    modes,
    value = null,
    allowDefault = false,
    loading = false,
    disabled = false,
    onchange,
    class: className = "",
  }: {
    /** Modes reported by the agent (empty while the probe has not answered). */
    modes: ModeInfo[];
    /** Current mode ID (`null` = unspecified). */
    value?: string | null;
    /** Offer an explicit "default" choice (new sessions send `null`). */
    allowDefault?: boolean;
    loading?: boolean;
    disabled?: boolean;
    onchange: (modeId: string | null) => void;
    class?: string;
  } = $props();

  const items = $derived(
    modes.map((mode) => ({
      value: mode.mode_id,
      label: mode.name.trim() === "" ? mode.mode_id : mode.name,
      description: mode.description,
    })),
  );

  let open = $state(false);

  const selected = $derived(items.find((item) => item.value === value) ?? null);

  /** Show the ID when the agent no longer reports the selected mode. */
  const currentLabel = $derived(
    selected?.label ??
      (value === null ? (allowDefault ? m.new_session_mode_default_value() : "") : value),
  );
</script>

{#snippet icon()}
  <SlidersHorizontalIcon class="text-muted-foreground size-3.5" />
{/snippet}

<PickerPill
  label={m.chat_mode_label()}
  value={currentLabel}
  {icon}
  {loading}
  disabled={disabled || loading}
  showLabel
  class={className}
  bind:open
  contentClass="w-[min(20rem,calc(100vw-1.5rem))]"
>
  <div class="max-h-72 overflow-y-auto p-1">
    {#if allowDefault}
      <PickerItem
        selected={value === null}
        onselect={() => {
          onchange(null);
          open = false;
        }}
      >
        <span class="block truncate">{m.new_session_mode_default()}</span>
      </PickerItem>
    {/if}
    {#each items as item (item.value)}
      <PickerItem
        selected={item.value === value}
        onselect={() => {
          onchange(item.value);
          open = false;
        }}
      >
        <span class="block truncate">{item.label}</span>
        {#if item.description}
          <span class="text-muted-foreground block truncate text-[11px]">
            {item.description}
          </span>
        {/if}
      </PickerItem>
    {/each}
  </div>
</PickerPill>
