import type { ConfigOptionInfo, ConfigOptionValue, JsonValue } from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";

/** A config option (`ConfigOptionInfo`) choice (Select value / display label / submitted value). */
export interface ConfigOptionChoice {
  /** Stable string representation passed to `Select.Item` (objects become JSON strings). */
  value: string;
  /** Display label. */
  label: string;
  /** Original value sent to `set_config`. */
  raw: ConfigOptionValue;
}

/** Convert a config value to a string for a Select value (non-strings are JSON-serialized). */
export function configValueToString(value: ConfigOptionValue | null): string {
  return typeof value === "string" ? value : JSON.stringify(value);
}

/** Provider part of a `provider/model` value, when present. */
function providerFromValue(value: unknown): string | null {
  if (typeof value !== "string") return null;
  const separator = value.indexOf("/");
  if (separator <= 0 || separator === value.length - 1) return null;
  return value.slice(0, separator);
}

/**
 * Stable identity of a config value.
 *
 * ACP reports select options as `{ value, name, description }`. Matching on
 * the wire value keeps saved preferences and favorites working even when the
 * agent changes only the display label.
 */
export function configValueKey(value: ConfigOptionValue | null): string {
  if (value !== null && typeof value === "object" && !Array.isArray(value)) {
    const raw = (value as Record<string, JsonValue>).value;
    if (typeof raw === "string" && raw !== "") return raw;
  }
  return configValueToString(value);
}

/** Resolve a choice's display label (supports booleans and ACP choices). */
function choiceLabel(raw: ConfigOptionValue): string {
  if (typeof raw === "boolean") return raw ? m.lib_config_enabled() : m.lib_config_disabled();
  if (raw !== null && typeof raw === "object" && !Array.isArray(raw)) {
    const record = raw as Record<string, JsonValue>;
    const value = record.value;
    const name = record.name;
    if (typeof name === "string" && name !== "") {
      const provider = providerFromValue(value);
      if (provider !== null && name.startsWith(`${provider}/`)) {
        return name.slice(provider.length + 1);
      }
      return name;
    }
    if (typeof value === "string" && value !== "") return value;
  }
  return configValueToString(raw);
}

/**
 * Convert `ConfigOptionInfo.options` into Select choices.
 *
 * `null` values are excluded because they cannot be selected. Object values
 * (such as ACP choices `{ value, name }`) use their JSON string as the Select
 * value and are restored to the original object by [`resolveConfigChoice`]
 * when submitting `set_config`.
 */
export function configChoices(options: ConfigOptionValue[]): ConfigOptionChoice[] {
  return options.map((raw) => ({
    value: configValueToString(raw),
    label: choiceLabel(raw),
    raw,
  }));
}

/**
 * Secondary label for a choice (for example the provider of a model).
 *
 * Returns `null` when there is nothing useful to show. The primary label is
 * still [`configChoices`]'s `label`.
 */
export function configChoiceDescription(raw: ConfigOptionValue): string | null {
  if (raw === null || typeof raw !== "object" || Array.isArray(raw)) return null;
  const record = raw as Record<string, JsonValue>;
  const description = record.description;
  if (typeof description === "string" && description.trim() !== "") return description;
  const name = record.name;
  const provider = providerFromValue(record.value);
  if (provider !== null && typeof name === "string" && name.startsWith(`${provider}/`)) {
    return provider;
  }
  return null;
}

/**
 * Stable identity of a model choice.
 *
 * Model values are ACP choices `{ value, name }`. The display name can change
 * between agent versions, so favorites must key on the wire value instead of
 * the serialized choice.
 */
export function modelChoiceKey(raw: ConfigOptionValue): string {
  return configValueKey(raw);
}

/** Whether the option is the agent's model selector. */
export function isModelOption(option: ConfigOptionInfo): boolean {
  // Some agents omit the ACP category; `model` is the conventional key.
  return option.category === "model" || (option.category === null && option.key === "model");
}

/** Whether the option is the session mode selector. */
function isModeOption(option: ConfigOptionInfo): boolean {
  return option.category === "mode" || (option.category === null && option.key === "mode");
}

/**
 * Selectable config options in display order.
 *
 * The model comes first, then the thought level, then any agent-specific
 * options. Options without choices (booleans / unsupported kinds) are hidden.
 */
function configOptionRank(option: ConfigOptionInfo): number {
  if (isModelOption(option)) return 0;
  if (option.category === "thought_level") return 1;
  return 2;
}

/**
 * Whether an option belongs in the new-session composer.
 *
 * ACP `mode` options are controlled by the dedicated initial-mode setting
 * (`CreateSessionRequest.mode`), so showing them as config pills as well would
 * send two conflicting values.
 */
function isComposerConfigOption(option: ConfigOptionInfo): boolean {
  return option.options.length > 0 && !isModeOption(option);
}

export function orderedConfigOptions(options: readonly ConfigOptionInfo[]): ConfigOptionInfo[] {
  return options
    .filter(isComposerConfigOption)
    .toSorted((left, right) => configOptionRank(left) - configOptionRank(right));
}

/**
 * Build the initial config map from probed options and per-project, per-agent
 * memory.
 *
 * A saved value is used only when it is still one of the agent's choices;
 * otherwise the agent's current value is used. Options without choices are
 * omitted (the agent keeps its default).
 */
export function resolveInitialConfig(
  options: readonly ConfigOptionInfo[],
  saved: Readonly<Record<string, ConfigOptionValue>>,
): Record<string, ConfigOptionValue> {
  const result: Record<string, ConfigOptionValue> = {};
  for (const option of options) {
    if (!isComposerConfigOption(option)) continue;
    const choices = configChoices(option.options);
    const savedValue = saved[option.key];
    const savedChoice =
      savedValue === undefined
        ? undefined
        : choices.find((choice) => configValueKey(choice.raw) === configValueKey(savedValue));
    result[option.key] = savedChoice?.raw ?? option.current_value;
  }
  return result;
}

/** Resolve a value selected in the Select to the `ConfigOptionValue` sent to `set_config`. */
export function resolveConfigChoice(
  choices: ConfigOptionChoice[],
  value: string,
): ConfigOptionValue {
  const selected =
    choices.find((candidate) => candidate.value === value) ??
    choices.find((candidate) => configValueKey(candidate.raw) === value);
  return selected?.raw ?? value;
}

/**
 * Resolve `ConfigOptionInfo.current_value` to a Select value string.
 *
 * Returns `undefined` for `null` (unset) or values not present in the choices
 * so the Select shows its placeholder (no selection) instead of the raw JSON
 * string.
 */
export function selectedConfigValue(
  choices: ConfigOptionChoice[],
  currentValue: ConfigOptionValue | null,
): string | undefined {
  const key = configValueKey(currentValue);
  return choices.find((choice) => configValueKey(choice.raw) === key)?.value;
}
