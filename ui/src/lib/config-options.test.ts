import { describe, expect, it } from "vitest";
import {
  configChoices,
  configValueToString,
  resolveConfigChoice,
  type ConfigOptionChoice,
} from "./config-options";

describe("configValueToString", () => {
  it("keeps strings as-is", () => {
    expect(configValueToString("plan")).toBe("plan");
  });

  it("serializes non-string values to JSON", () => {
    expect(configValueToString(42)).toBe("42");
    expect(configValueToString(true)).toBe("true");
    expect(configValueToString(null)).toBe("null");
    expect(configValueToString({ providerID: "opencode", id: "gpt-5", name: "GPT-5" })).toBe(
      '{"providerID":"opencode","id":"gpt-5","name":"GPT-5"}',
    );
  });
});

describe("configChoices", () => {
  it("returns an empty list for non-array input", () => {
    expect(configChoices(null)).toEqual([]);
    expect(configChoices("plan")).toEqual([]);
  });

  it("maps primitive options with the value as label", () => {
    expect(configChoices(["plan", "code"])).toEqual([
      { value: "plan", label: "plan", raw: "plan" },
      { value: "code", label: "code", raw: "code" },
    ]);
  });

  it("skips null options", () => {
    expect(configChoices(["plan", null])).toHaveLength(1);
  });

  it("labels object options by name and keeps the raw object", () => {
    const choices = configChoices([
      { providerID: "opencode", id: "gpt-5", name: "GPT-5" },
      { providerID: "anthropic", id: "claude", name: "Claude" },
    ]);
    expect(choices.map((choice) => choice.label)).toEqual(["GPT-5", "Claude"]);
    expect(choices[0].value).toBe('{"providerID":"opencode","id":"gpt-5","name":"GPT-5"}');
    expect(choices[0].raw).toEqual({ providerID: "opencode", id: "gpt-5", name: "GPT-5" });
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
