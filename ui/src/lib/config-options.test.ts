import { describe, expect, it } from "vitest";
import type { ConfigOptionInfo } from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";
import {
  configChoiceDescription,
  configChoices,
  configValueToString,
  isModelOption,
  orderedConfigOptions,
  resolveConfigChoice,
  resolveInitialConfig,
  selectedConfigValue,
  type ConfigOptionChoice,
} from "./config-options";

const modelOption: ConfigOptionInfo = {
  key: "model",
  name: "Model",
  description: null,
  category: "model",
  current_value: "gpt-5",
  options: ["gpt-5", "gpt-4"],
};

const thoughtOption: ConfigOptionInfo = {
  key: "thought_level",
  name: "Thought level",
  description: null,
  category: "thought_level",
  current_value: "medium",
  options: ["low", "medium", "high"],
};

const booleanOption: ConfigOptionInfo = {
  key: "auto_compact",
  name: "Auto compact",
  description: null,
  category: null,
  current_value: true,
  options: [],
};

describe("configValueToString", () => {
  it("keeps strings as-is", () => {
    expect(configValueToString("plan")).toBe("plan");
  });

  it("serializes non-string values to JSON", () => {
    expect(configValueToString(42)).toBe("42");
    expect(configValueToString(true)).toBe("true");
    expect(configValueToString(null)).toBe("null");
    expect(
      configValueToString({
        providerID: "opencode",
        id: "gpt-5",
        name: "GPT-5",
      }),
    ).toBe('{"providerID":"opencode","id":"gpt-5","name":"GPT-5"}');
  });
});

describe("configChoices", () => {
  it("returns an empty list for an empty option list", () => {
    expect(configChoices([])).toEqual([]);
  });

  it("maps primitive options with the value as label", () => {
    expect(configChoices(["plan", "code"])).toEqual([
      { value: "plan", label: "plan", raw: "plan" },
      { value: "code", label: "code", raw: "code" },
    ]);
  });

  it("labels boolean options with enabled / disabled", () => {
    expect(configChoices([true, false])).toEqual([
      { value: "true", label: m.lib_config_enabled(), raw: true },
      { value: "false", label: m.lib_config_disabled(), raw: false },
    ]);
  });

  it("labels object options by name and keeps the raw object", () => {
    const choices = configChoices([
      { providerID: "opencode", id: "gpt-5", name: "GPT-5" },
      { providerID: "anthropic", id: "claude", name: "Claude" },
    ]);
    expect(choices.map((choice) => choice.label)).toEqual(["GPT-5", "Claude"]);
    expect(choices[0].value).toBe('{"providerID":"opencode","id":"gpt-5","name":"GPT-5"}');
    expect(choices[0].raw).toEqual({
      providerID: "opencode",
      id: "gpt-5",
      name: "GPT-5",
    });
  });

  it("falls back to provider/id or id when name is missing", () => {
    expect(configChoices([{ providerID: "opencode", id: "gpt-5" }])[0].label).toBe(
      "opencode/gpt-5",
    );
    expect(configChoices([{ id: "gpt-5" }])[0].label).toBe("gpt-5");
  });
});

describe("resolveConfigChoice", () => {
  const choices: ConfigOptionChoice[] = configChoices([
    "plan",
    { providerID: "opencode", id: "gpt-5", name: "GPT-5" },
  ]);

  it("restores the raw object from the JSON value", () => {
    expect(
      resolveConfigChoice(choices, '{"providerID":"opencode","id":"gpt-5","name":"GPT-5"}'),
    ).toEqual({ providerID: "opencode", id: "gpt-5", name: "GPT-5" });
  });

  it("restores primitive choices as-is", () => {
    expect(resolveConfigChoice(choices, "plan")).toBe("plan");
  });

  it("falls back to the raw string for unknown values", () => {
    expect(resolveConfigChoice(choices, "unknown")).toBe("unknown");
  });
});

describe("selectedConfigValue", () => {
  const choices: ConfigOptionChoice[] = configChoices([
    "plan",
    { providerID: "opencode", id: "gpt-5", name: "GPT-5" },
  ]);

  it("returns the select value for a known current value", () => {
    expect(selectedConfigValue(choices, "plan")).toBe("plan");
    expect(
      selectedConfigValue(choices, {
        providerID: "opencode",
        id: "gpt-5",
        name: "GPT-5",
      }),
    ).toBe('{"providerID":"opencode","id":"gpt-5","name":"GPT-5"}');
  });

  it("returns undefined for unset (null) or unknown values", () => {
    expect(selectedConfigValue(choices, null)).toBeUndefined();
    expect(selectedConfigValue(choices, "unknown")).toBeUndefined();
  });
});

describe("configChoiceDescription", () => {
  it("returns the provider for model objects", () => {
    expect(configChoiceDescription({ providerID: "anthropic", id: "claude", name: "Claude" })).toBe(
      "anthropic",
    );
  });

  it("returns null for primitive values", () => {
    expect(configChoiceDescription("plan")).toBeNull();
    expect(configChoiceDescription(true)).toBeNull();
  });
});

describe("orderedConfigOptions", () => {
  it("puts the model first, then the thought level, then agent-specific options", () => {
    const custom: ConfigOptionInfo = {
      ...thoughtOption,
      key: "custom",
      name: "Custom",
      category: null,
    };
    expect(orderedConfigOptions([custom, thoughtOption, booleanOption, modelOption])).toEqual([
      modelOption,
      thoughtOption,
      custom,
    ]);
  });

  it("hides options without choices", () => {
    expect(orderedConfigOptions([booleanOption])).toEqual([]);
  });
});

describe("mode config options", () => {
  const modeOption: ConfigOptionInfo = {
    ...thoughtOption,
    key: "mode",
    name: "Session Mode",
    category: "mode",
    current_value: "build",
    options: ["build", "plan"],
  };

  it("are not rendered as composer pills", () => {
    expect(orderedConfigOptions([modeOption, modelOption])).toEqual([modelOption]);
    expect(orderedConfigOptions([{ ...modeOption, category: null }, modelOption])).toEqual([
      modelOption,
    ]);
  });

  it("are not sent as initial config (the mode field controls them)", () => {
    expect(resolveInitialConfig([modeOption, modelOption], {})).toEqual({
      model: "gpt-5",
    });
  });
});

describe("resolveInitialConfig", () => {
  it("uses a saved value that is still selectable", () => {
    expect(resolveInitialConfig([modelOption], { model: "gpt-4" })).toEqual({
      model: "gpt-4",
    });
  });

  it("falls back to the agent current value when the saved value is stale", () => {
    expect(resolveInitialConfig([modelOption], { model: "removed-model" })).toEqual({
      model: "gpt-5",
    });
  });

  it("omits options without choices", () => {
    expect(resolveInitialConfig([booleanOption], {})).toEqual({});
  });
});

describe("isModelOption", () => {
  it("uses the semantic category instead of the key", () => {
    expect(isModelOption(modelOption)).toBe(true);
    expect(isModelOption({ ...modelOption, key: "not-the-model-key" })).toBe(true);
    expect(isModelOption(thoughtOption)).toBe(false);
  });

  it("falls back to the conventional key when the category is absent", () => {
    expect(isModelOption({ ...modelOption, category: null })).toBe(true);
  });
});
