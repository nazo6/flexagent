<script lang="ts">
  import PickerItem from "./PickerItem.svelte";
  import PickerPill from "./PickerPill.svelte";
  import {
    configChoiceDescription,
    configChoices,
    isModelOption,
    modelChoiceKey,
    resolveConfigChoice,
    selectedConfigValue,
    type ConfigOptionChoice,
  } from "#lib/config-options.ts";
  import { loadModelFavorites, toggleModelFavorite } from "#lib/session-prefs.ts";
  import { m } from "#lib/paraglide/messages.js";
  import type { ConfigOptionInfo, ConfigOptionValue } from "#lib/generated/bindings.ts";
  import BrainIcon from "@lucide/svelte/icons/brain";
  import RefreshCwIcon from "@lucide/svelte/icons/refresh-cw";
  import SearchIcon from "@lucide/svelte/icons/search";
  import SparklesIcon from "@lucide/svelte/icons/sparkles";

  let {
    option,
    value = undefined,
    onchange,
    showRefresh = false,
    onrefresh,
    disabled = false,
    favoriteScope = "",
    class: className = "",
  }: {
    option: ConfigOptionInfo;
    /** Controlled value; when omitted, the option's current value is used. */
    value?: ConfigOptionValue | null;
    onchange: (value: ConfigOptionValue) => void;
    showRefresh?: boolean;
    onrefresh?: () => void;
    disabled?: boolean;
    /** Agent ID used to scope favorite models (empty disables favorites). */
    favoriteScope?: string;
    class?: string;
  } = $props();

  let open = $state(false);
  let query = $state("");
  /**
   * Optimistic choice picked by the user, kept until the agent reports a new
   * value. It is not cleared by an effect: once the base value changes (the
   * `set_config` result arrives), the derived value below ignores it.
   */
  let optimistic = $state<{ base: ConfigOptionValue | null; value: ConfigOptionValue } | null>(
    null,
  );
  /** Bumped when the favorite list is written so the derived list reloads. */
  let favoritesRevision = $state(0);

  /** Value reported by the agent (or the controlled prop). */
  const base = $derived(value !== undefined ? value : option.current_value);
  const selected = $derived(
    optimistic !== null && Object.is(optimistic.base, base) ? optimistic.value : base,
  );

  // Favorites are a per-device preference scoped to the selected agent.
  const favorites = $derived.by(() => {
    void favoritesRevision;
    return favoriteScope === "" ? [] : loadModelFavorites(favoriteScope);
  });

  const choices = $derived(configChoices(option.options));
  const current = $derived(selectedConfigValue(choices, selected) ?? "");
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
  const favoriteKeys = $derived(new Set(favorites));
  const isFavoriteChoice = (choice: ConfigOptionChoice): boolean =>
    favoriteKeys.has(modelChoiceKey(choice.raw));
  const showStar = $derived(isModelOption(option) && favoriteScope !== "");
  // Only the model picker groups choices; other options keep the agent's order.
  const favoriteChoices = $derived(showStar ? filteredChoices.filter(isFavoriteChoice) : []);
  const otherChoices = $derived(
    showStar ? filteredChoices.filter((choice) => !isFavoriteChoice(choice)) : filteredChoices,
  );
  const showSearch = $derived(choices.length > 8);

  function toggleFavorite(choice: ConfigOptionChoice): void {
    if (favoriteScope === "") return;
    toggleModelFavorite(favoriteScope, modelChoiceKey(choice.raw));
    favoritesRevision += 1;
  }

  /** Programmatic closes do not emit `onOpenChange`, so reset the search here. */
  function closePicker(): void {
    open = false;
    query = "";
  }
</script>

{#snippet icon()}
  {#if isModelOption(option)}
    <SparklesIcon class="text-muted-foreground size-3.5" />
  {:else}
    <BrainIcon class="text-muted-foreground size-3.5" />
  {/if}
{/snippet}

{#snippet choiceRow(choice: ConfigOptionChoice)}
  {@const favorited = isFavoriteChoice(choice)}
  <PickerItem
    selected={choice.value === current}
    favorite={showStar
      ? {
          active: favorited,
          label: favorited ? m.composer_model_favorite_remove() : m.composer_model_favorite_add(),
          onToggle: () => toggleFavorite(choice),
        }
      : null}
    onselect={() => {
      optimistic = { base, value: choice.raw };
      onchange(resolveConfigChoice(choices, choice.value));
      closePicker();
    }}
  >
    <span class="block truncate">{choice.label}</span>
    {#if configChoiceDescription(choice.raw)}
      <span class="text-muted-foreground block truncate text-[11px]">
        {configChoiceDescription(choice.raw)}
      </span>
    {/if}
  </PickerItem>
{/snippet}

<PickerPill
  label={option.name}
  value={currentChoice?.label ?? option.name}
  {icon}
  {disabled}
  bind:open
  onopenchange={(next) => {
    if (!next) query = "";
  }}
  class={className}
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
          class="placeholder:text-muted-foreground min-w-0 flex-1 bg-transparent text-[13px] outline-none"
          placeholder={m.composer_search_options()}
        />
      </div>
    {/if}

    <div class="max-h-72 overflow-y-auto p-1">
      {#if favoriteChoices.length > 0}
        <p class="text-muted-foreground px-2 py-1 text-[11px] font-medium">
          {m.composer_model_favorites()}
        </p>
        {#each favoriteChoices as choice (choice.value)}
          {@render choiceRow(choice)}
        {/each}
      {/if}
      {#if otherChoices.length > 0}
        {#if favoriteChoices.length > 0}
          <p class="text-muted-foreground mt-1 border-t px-2 pt-2 pb-1 text-[11px] font-medium">
            {m.composer_models_other()}
          </p>
        {/if}
        {#each otherChoices as choice (choice.value)}
          {@render choiceRow(choice)}
        {/each}
      {/if}
      {#if filteredChoices.length === 0}
        <p class="text-muted-foreground px-2 py-3 text-center text-[13px]">
          {m.composer_no_matching_options()}
        </p>
      {/if}
    </div>

    {#if showRefresh}
      <button
        type="button"
        class="text-muted-foreground hover:bg-accent hover:text-accent-foreground flex items-center gap-1.5 border-t px-3 py-2 text-[13px] transition-colors"
        onclick={() => {
          closePicker();
          onrefresh?.();
        }}
      >
        <RefreshCwIcon class="size-3.5" />
        {m.composer_refresh_options()}
      </button>
    {/if}
  </div>
</PickerPill>
