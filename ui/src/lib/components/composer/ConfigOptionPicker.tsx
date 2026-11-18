import { createMemo, createSignal, For, Show } from "solid-js";
import PickerItem from "./PickerItem.tsx";
import PickerPill from "./PickerPill.tsx";
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
import IconBrain from "~icons/lucide/brain";
import IconRefreshCw from "~icons/lucide/refresh-cw";
import IconSearch from "~icons/lucide/search";
import IconSparkles from "~icons/lucide/sparkles";

export interface ConfigOptionPickerProps {
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
}

/** Selector for one agent config option (model / thought level / ...). */
export default function ConfigOptionPicker(props: ConfigOptionPickerProps) {
  const [open, setOpen] = createSignal(false);
  const [query, setQuery] = createSignal("");
  /**
   * Optimistic choice picked by the user, kept until the agent reports a new
   * value: once the base value changes (the `set_config` result arrives), the
   * derived value below ignores it.
   */
  const [optimistic, setOptimistic] = createSignal<{
    base: ConfigOptionValue | null;
    value: ConfigOptionValue;
  } | null>(null);
  /** Bumped when the favorite list is written so the derived list reloads. */
  const [favoritesRevision, setFavoritesRevision] = createSignal(0);

  /** Value reported by the agent (or the controlled prop). */
  const base = () =>
    props.value !== undefined ? (props.value ?? null) : props.option.current_value;
  const selected = () => {
    const optimisticValue = optimistic();
    return optimisticValue !== null && Object.is(optimisticValue.base, base())
      ? optimisticValue.value
      : base();
  };

  // Favorites are a per-device preference scoped to the selected agent.
  const favorites = createMemo(() => {
    void favoritesRevision();
    const scope = props.favoriteScope ?? "";
    return scope === "" ? [] : loadModelFavorites(scope);
  });

  const choices = createMemo(() => configChoices(props.option.options));
  const current = () => selectedConfigValue(choices(), selected()) ?? "";
  const currentChoice = createMemo(
    () => choices().find((choice) => choice.value === current()) ?? null,
  );
  const filteredChoices = createMemo(() => {
    const needle = query().trim().toLowerCase();
    if (needle === "") return choices();
    return choices().filter((choice) => {
      const description = configChoiceDescription(choice.raw) ?? "";
      return (
        choice.label.toLowerCase().includes(needle) || description.toLowerCase().includes(needle)
      );
    });
  });
  const favoriteKeys = createMemo(() => new Set(favorites()));
  const isFavoriteChoice = (choice: ConfigOptionChoice): boolean =>
    favoriteKeys().has(modelChoiceKey(choice.raw));
  const showStar = () => isModelOption(props.option) && (props.favoriteScope ?? "") !== "";
  // Only the model picker groups choices; other options keep the agent's order.
  const favoriteChoices = createMemo(() =>
    showStar() ? filteredChoices().filter(isFavoriteChoice) : [],
  );
  const otherChoices = createMemo(() =>
    showStar()
      ? filteredChoices().filter((choice) => !isFavoriteChoice(choice))
      : filteredChoices(),
  );
  const showSearch = () => choices().length > 8;

  function toggleFavorite(choice: ConfigOptionChoice): void {
    const scope = props.favoriteScope ?? "";
    if (scope === "") return;
    toggleModelFavorite(scope, modelChoiceKey(choice.raw));
    setFavoritesRevision((value) => value + 1);
  }

  /** Programmatic closes do not emit `onOpenChange`, so reset the search here. */
  function closePicker(): void {
    setOpen(false);
    setQuery("");
  }

  const choiceRow = (choice: ConfigOptionChoice) => {
    const favorited = isFavoriteChoice(choice);
    return (
      <PickerItem
        selected={choice.value === current()}
        favorite={
          showStar()
            ? {
                active: favorited,
                label: favorited
                  ? m.composer_model_favorite_remove()
                  : m.composer_model_favorite_add(),
                onToggle: () => toggleFavorite(choice),
              }
            : null
        }
        onselect={() => {
          setOptimistic({ base: base(), value: choice.raw });
          props.onchange(resolveConfigChoice(choices(), choice.value));
          closePicker();
        }}
      >
        <span class="block truncate">{choice.label}</span>
        <Show when={configChoiceDescription(choice.raw)}>
          {(description) => (
            <span class="text-muted-foreground block truncate text-[11px]">{description()}</span>
          )}
        </Show>
      </PickerItem>
    );
  };

  return (
    <PickerPill
      label={props.option.name}
      value={currentChoice()?.label ?? props.option.name}
      icon={
        <Show
          when={isModelOption(props.option)}
          fallback={<IconBrain class="text-muted-foreground size-3.5" />}
        >
          <IconSparkles class="text-muted-foreground size-3.5" />
        </Show>
      }
      disabled={props.disabled ?? false}
      open={open()}
      onopenchange={(next) => {
        setOpen(next);
        if (!next) setQuery("");
      }}
      class={props.class}
      contentClass="w-[min(20rem,calc(100vw-1.5rem))] p-0"
    >
      <div class="flex flex-col">
        <Show when={props.option.description}>
          {(description) => (
            <p class="text-muted-foreground border-b px-3 py-2 text-[11px] leading-relaxed">
              {description()}
            </p>
          )}
        </Show>

        <Show when={showSearch()}>
          <div class="flex items-center gap-1.5 border-b px-2.5 py-2">
            <IconSearch class="text-muted-foreground size-3.5 shrink-0" />
            <input
              value={query()}
              onInput={(event) => setQuery(event.currentTarget.value)}
              class="placeholder:text-muted-foreground min-w-0 flex-1 bg-transparent text-[13px] outline-none"
              placeholder={m.composer_search_options()}
            />
          </div>
        </Show>

        <div class="max-h-72 overflow-y-auto p-1">
          <Show when={favoriteChoices().length > 0}>
            <p class="text-muted-foreground px-2 py-1 text-[11px] font-medium">
              {m.composer_model_favorites()}
            </p>
            <For each={favoriteChoices()}>{(choice) => choiceRow(choice)}</For>
          </Show>
          <Show when={otherChoices().length > 0}>
            <Show when={favoriteChoices().length > 0}>
              <p class="text-muted-foreground mt-1 border-t px-2 pt-2 pb-1 text-[11px] font-medium">
                {m.composer_models_other()}
              </p>
            </Show>
            <For each={otherChoices()}>{(choice) => choiceRow(choice)}</For>
          </Show>
          <Show when={filteredChoices().length === 0}>
            <p class="text-muted-foreground px-2 py-3 text-center text-[13px]">
              {m.composer_no_matching_options()}
            </p>
          </Show>
        </div>

        <Show when={props.showRefresh ?? false}>
          <button
            type="button"
            class="text-muted-foreground hover:bg-accent hover:text-accent-foreground flex items-center gap-1.5 border-t px-3 py-2 text-[13px] transition-colors"
            onClick={() => {
              closePicker();
              props.onrefresh?.();
            }}
          >
            <IconRefreshCw class="size-3.5" />
            {m.composer_refresh_options()}
          </button>
        </Show>
      </div>
    </PickerPill>
  );
}
