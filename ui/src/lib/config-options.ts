import type { JsonValue } from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";

/** A config option (`ConfigOptionInfo`) choice (Select value / display label / submitted value). */
export interface ConfigOptionChoice {
  /** Stable string representation passed to `Select.Item` (objects become JSON strings). */
  value: string;
  /** Display label. */
  label: string;
  /** Original JSON value sent to `set_config`. */
  raw: JsonValue;
}

/** Convert a `JsonValue` to a string for a Select value (non-strings are JSON-serialized). */
export function configValueToString(value: JsonValue): string {
  return typeof value === "string" ? value : JSON.stringify(value);
}

/** Resolve a choice's display label (supports booleans, opencode2's `{ providerID, id, name }`, etc.). */
function choiceLabel(raw: JsonValue): string {
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
export function configChoices(options: JsonValue): ConfigOptionChoice[] {
  if (!Array.isArray(options)) return [];
  return options.flatMap((raw) =>
    raw === null ? [] : [{ value: configValueToString(raw), label: choiceLabel(raw), raw }],
  );
}

/** Resolve a value selected in the Select to the `JsonValue` sent to `set_config`. */
export function resolveConfigChoice(choices: ConfigOptionChoice[], value: string): JsonValue {
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
  currentValue: JsonValue,
): string | undefined {
  const value = configValueToString(currentValue);
  return choices.some((choice) => choice.value === value) ? value : undefined;
}
