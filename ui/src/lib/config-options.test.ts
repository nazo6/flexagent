import { describe, expect, it } from "vitest";
import type { ConfigOptionInfo } from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";
import {
  configChoiceDescription,
  configChoices,
  configValueKey,
  configValueToString,
  isModelOption,
  modelChoiceKey,
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
    expect(configValueToString({ value: "opencode/gpt-5", name: "GPT-5" })).toBe(
      '{"value":"opencode/gpt-5","name":"GPT-5"}',
    );
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

  it("labels ACP choices by name and keeps the raw object", () => {
    const choices = configChoices([
      { value: "opencode/gpt-5", name: "GPT-5" },
      { value: "anthropic/claude", name: "Claude" },
    ]);
    expect(choices.map((choice) => choice.label)).toEqual(["GPT-5", "Claude"]);
    expect(choices[0].value).toBe('{"value":"opencode/gpt-5","name":"GPT-5"}');
    expect(choices[0].raw).toEqual({ value: "opencode/gpt-5", name: "GPT-5" });
  });

  it("falls back to the wire value when name is empty", () => {
    expect(configChoices([{ value: "opencode/gpt-5", name: "" }])[0].label).toBe("opencode/gpt-5");
  });

  it("uses ACP choice names and strips the provider prefix", () => {
    const choices = configChoices([
      { value: "opencode/beam-free", name: "opencode/Beam Free" },
      { value: "gemini-4-argon", name: "Gemini 4 Argon" },
      { value: "build", name: "Build", description: "The default agent." },
    ]);
    expect(choices.map((choice) => choice.label)).toEqual(["Beam Free", "Gemini 4 Argon", "Build"]);
  });
});

describe("resolveConfigChoice", () => {
  const choices: ConfigOptionChoice[] = configChoices([
    "plan",
    { value: "opencode/gpt-5", name: "GPT-5" },
  ]);

  it("restores the raw object from the JSON value", () => {
    expect(resolveConfigChoice(choices, '{"value":"opencode/gpt-5","name":"GPT-5"}')).toEqual({
      value: "opencode/gpt-5",
      name: "GPT-5",
    });
  });

  it("restores primitive choices as-is", () => {
    expect(resolveConfigChoice(choices, "plan")).toBe("plan");
  });

  it("falls back to the raw string for unknown values", () => {
    expect(resolveConfigChoice(choices, "unknown")).toBe("unknown");
  });

  it("resolves a saved bare value to the current metadata choice", () => {
    const acpChoices = configChoices([{ value: "opencode/gpt-5", name: "opencode/GPT-5" }]);
    expect(resolveConfigChoice(acpChoices, "opencode/gpt-5")).toEqual({
      value: "opencode/gpt-5",
      name: "opencode/GPT-5",
    });
  });
});

describe("selectedConfigValue", () => {
  const choices: ConfigOptionChoice[] = configChoices([
    "plan",
    { value: "opencode/gpt-5", name: "GPT-5" },
  ]);

  it("returns the select value for a known current value", () => {
    expect(selectedConfigValue(choices, "plan")).toBe("plan");
    expect(selectedConfigValue(choices, { value: "opencode/gpt-5", name: "GPT-5" })).toBe(
      '{"value":"opencode/gpt-5","name":"GPT-5"}',
    );
  });

  it("returns undefined for unset (null) or unknown values", () => {
    expect(selectedConfigValue(choices, null)).toBeUndefined();
    expect(selectedConfigValue(choices, "unknown")).toBeUndefined();
  });

  it("matches a saved bare value against a metadata choice", () => {
    const acpChoices = configChoices([{ value: "opencode/gpt-5", name: "opencode/GPT-5" }]);
    expect(selectedConfigValue(acpChoices, "opencode/gpt-5")).toBe(
      configValueToString({ value: "opencode/gpt-5", name: "opencode/GPT-5" }),
    );
  });
});

describe("configChoiceDescription", () => {
  it("returns the provider for model choices", () => {
    expect(configChoiceDescription({ value: "anthropic/claude", name: "anthropic/Claude" })).toBe(
      "anthropic",
    );
  });

  it("returns null for primitive values", () => {
    expect(configChoiceDescription("plan")).toBeNull();
    expect(configChoiceDescription(true)).toBeNull();
  });

  it("prefers the ACP description, then the provider of a model", () => {
    expect(
      configChoiceDescription({
        value: "build",
        name: "Build",
        description: "The default agent.",
      }),
    ).toBe("The default agent.");
    expect(
      configChoiceDescription({ value: "opencode/beam-free", name: "opencode/Beam Free" }),
    ).toBe("opencode");
    expect(configChoiceDescription({ value: "gemini-4-argon", name: "Gemini 4 Argon" })).toBeNull();
  });
});

describe("configValueKey", () => {
  it("uses the wire value of a choice and serialized primitives", () => {
    expect(configValueKey({ value: "opencode/gpt-5", name: "GPT-5" })).toBe("opencode/gpt-5");
    expect(configValueKey("gpt-5")).toBe("gpt-5");
    expect(configValueKey(true)).toBe("true");
  });
});

describe("modelChoiceKey", () => {
  it("uses the wire value of a model choice", () => {
    expect(modelChoiceKey({ value: "opencode/gpt-5", name: "GPT-5" })).toBe("opencode/gpt-5");
  });

  it("keeps the identity when the display name changes", () => {
    expect(modelChoiceKey({ value: "opencode/gpt-5", name: "GPT-5" })).toBe(
      modelChoiceKey({ value: "opencode/gpt-5", name: "GPT-5 (renamed)" }),
    );
  });

  it("falls back to the serialized value", () => {
    expect(modelChoiceKey("gpt-5")).toBe("gpt-5");
    expect(modelChoiceKey(true)).toBe("true");
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

  it("upgrades a saved bare value to the current ACP choice", () => {
    const option: ConfigOptionInfo = {
      ...modelOption,
      current_value: { value: "opencode/gpt-5", name: "opencode/GPT-5" },
      options: [
        { value: "opencode/gpt-5", name: "opencode/GPT-5" },
        { value: "opencode/gpt-4", name: "opencode/GPT-4" },
      ],
    };
    expect(resolveInitialConfig([option], { model: "opencode/gpt-4" })).toEqual({
      model: { value: "opencode/gpt-4", name: "opencode/GPT-4" },
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
