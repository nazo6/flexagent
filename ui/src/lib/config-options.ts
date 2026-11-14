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

/** Resolve a choice's display label (supports booleans, object values such as `{ providerID, id, name }`, etc.). */
function choiceLabel(raw: ConfigOptionValue): string {
  if (typeof raw === "boolean") return raw ? m.lib_config_enabled() : m.lib_config_disabled();
  if (raw !== null && typeof raw === "object" && !Array.isArray(raw)) {
    const record = raw as Record<string, JsonValue>;
    const name = record.name;
    if (typeof name === "string" && name !== "") return name;
    const provider = record.providerID;
    const id = record.id;
    if (typeof provider === "string" && typeof id === "string") {
      return `${provider}/${id}`;
    }
    if (typeof id === "string" && id !== "") return id;
  }
  return configValueToString(raw);
}

/**
 * Convert `ConfigOptionInfo.options` into Select choices.
 *
 * `null` values are excluded because they cannot be selected. Object values
 * (such as model references `{ providerID, id, name }`) use their JSON string
 * as the Select value and are restored to the original object by
 * [`resolveConfigChoice`] when submitting `set_config`.
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
  const provider = record.providerID;
  const name = record.name;
  if (typeof provider === "string" && provider !== "" && typeof name === "string" && name !== "") {
    return provider;
  }
  return null;
}

/**
 * Stable identity of a model choice.
 *
 * Model values are usually objects such as `{ providerID, id, name }`. The
 * display name can change between agent versions, so favorites must key on
 * `providerID/id` instead of the serialized choice value.
 */
export function modelChoiceKey(raw: ConfigOptionValue): string {
  if (raw !== null && typeof raw === "object" && !Array.isArray(raw)) {
    const record = raw as Record<string, JsonValue>;
    const provider = record.providerID;
    const id = record.id;
    if (typeof provider === "string" && provider !== "" && typeof id === "string" && id !== "") {
      return `${provider}/${id}`;
    }
    if (typeof id === "string" && id !== "") return id;
  }
  return configValueToString(raw);
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
    result[option.key] =
      savedValue !== undefined && selectedConfigValue(choices, savedValue) !== undefined
        ? savedValue
        : option.current_value;
  }
  return result;
}

/** Resolve a value selected in the Select to the `ConfigOptionValue` sent to `set_config`. */
export function resolveConfigChoice(
  choices: ConfigOptionChoice[],
  value: string,
): ConfigOptionValue {
  return choices.find((choice) => choice.value === value)?.raw ?? value;
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
  const value = configValueToString(currentValue);
  return choices.some((choice) => choice.value === value) ? value : undefined;
}
