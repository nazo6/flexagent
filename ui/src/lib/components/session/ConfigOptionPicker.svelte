<script lang="ts">
  import PickerItem from "./PickerItem.svelte";
  import PickerPill from "./PickerPill.svelte";
  import {
    configChoiceDescription,
    configChoices,
    isModelOption,
    resolveConfigChoice,
    selectedConfigValue,
  } from "#lib/config-options.ts";
  import { m } from "#lib/paraglide/messages.js";
  import type { ConfigOptionInfo, ConfigOptionValue } from "#lib/generated/bindings.ts";
  import BrainIcon from "@lucide/svelte/icons/brain";
  import RefreshCwIcon from "@lucide/svelte/icons/refresh-cw";
  import SearchIcon from "@lucide/svelte/icons/search";
  import SparklesIcon from "@lucide/svelte/icons/sparkles";

  let {
    option,
    value = null,
    onchange,
    showRefresh = false,
    onrefresh,
    disabled = false,
  }: {
    option: ConfigOptionInfo;
    value?: ConfigOptionValue | null;
    onchange: (value: ConfigOptionValue) => void;
    showRefresh?: boolean;
    onrefresh?: () => void;
    disabled?: boolean;
  } = $props();

  let open = $state(false);
  let query = $state("");

  $effect(() => {
    if (!open) query = "";
  });

  const choices = $derived(configChoices(option.options));
  const current = $derived(selectedConfigValue(choices, value ?? option.current_value) ?? "");
  const currentChoice = $derived(choices.find((choice) => choice.value === current) ?? null);
  const filteredChoices = $derived.by(() => {
    const needle = query.trim().toLowerCase();
    if (needle === "") return choices;
    return choices.filter((choice) => {
      const description = configChoiceDescription(choice.raw) ?? "";
      return (
        choice.label.toLowerCase().includes(needle) || description.toLowerCase().includes(needle)
      );
    });
  });
  const showSearch = $derived(choices.length > 8);
</script>

{#snippet icon()}
  {#if isModelOption(option)}
    <SparklesIcon class="text-muted-foreground size-3.5" />
  {:else}
    <BrainIcon class="text-muted-foreground size-3.5" />
  {/if}
{/snippet}

<PickerPill
  label={option.name}
  value={currentChoice?.label ?? option.name}
  {icon}
  {disabled}
  showLabel={!isModelOption(option)}
  bind:open
  contentClass="w-[min(20rem,calc(100vw-1.5rem))] p-0"
>
  <div class="flex flex-col">
    {#if option.description}
      <p class="text-muted-foreground border-b px-3 py-2 text-[11px] leading-relaxed">
        {option.description}
      </p>
    {/if}

    {#if showSearch}
      <div class="flex items-center gap-1.5 border-b px-2.5 py-2">
        <SearchIcon class="text-muted-foreground size-3.5 shrink-0" />
        <input
          bind:value={query}
          class="placeholder:text-muted-foreground min-w-0 flex-1 bg-transparent text-xs outline-none"
          placeholder={m.new_session_search_options()}
        />
      </div>
    {/if}

    <div class="max-h-72 overflow-y-auto p-1">
      {#each filteredChoices as choice (choice.value)}
        <PickerItem
          selected={choice.value === current}
          onselect={() => {
            onchange(resolveConfigChoice(choices, choice.value));
            open = false;
          }}
        >
          <span class="block truncate">{choice.label}</span>
          {#if configChoiceDescription(choice.raw)}
            <span class="text-muted-foreground block truncate text-[10px]">
              {configChoiceDescription(choice.raw)}
            </span>
          {/if}
        </PickerItem>
      {:else}
        <p class="text-muted-foreground px-2 py-3 text-center text-xs">
          {m.new_session_no_matching_models()}
        </p>
      {/each}
    </div>

    {#if showRefresh}
      <button
        type="button"
        class="text-muted-foreground hover:bg-accent hover:text-accent-foreground flex items-center gap-1.5 border-t px-3 py-2 text-xs transition-colors"
        onclick={() => {
          open = false;
          onrefresh?.();
        }}
      >
        <RefreshCwIcon class="size-3.5" />
        {m.new_session_refresh_models()}
      </button>
    {/if}
  </div>
</PickerPill>
