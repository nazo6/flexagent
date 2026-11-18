import { describe, expect, it } from "vitest";
import {
  loadModelFavorites,
  loadNewSessionPrefs,
  loadSendShortcutPreference,
  loadSessionConfig,
  saveNewSessionPrefs,
  saveSendShortcutPreference,
  saveSessionConfigOption,
  shouldSubmitOnEnter,
  toggleModelFavorite,
} from "./session-prefs";

/** Minimal Storage implementation for tests (mimics localStorage in Node). */
function memoryStorage(): Storage {
  const map = new Map<string, string>();
  return {
    get length() {
      return map.size;
    },
    clear: () => map.clear(),
    getItem: (key: string) => map.get(key) ?? null,
    key: (index: number) => [...map.keys()][index] ?? null,
    removeItem: (key: string) => void map.delete(key),
    setItem: (key: string, value: string) => void map.set(key, value),
  };
}

describe("session prefs", () => {
  it("round-trips preferences per project", () => {
    const storage = memoryStorage();
    saveNewSessionPrefs(
      "github.com/nazo6/flexagent",
      { agent: "claude-code", mode: "plan" },
      storage,
    );
    expect(loadNewSessionPrefs("github.com/nazo6/flexagent", storage)).toEqual({
      agent: "claude-code",
      mode: "plan",
    });
    // Agent-specific mode IDs are kept verbatim
    saveNewSessionPrefs(
      "github.com/nazo6/flexagent",
      { agent: "opencode", mode: "build" },
      storage,
    );
    expect(loadNewSessionPrefs("github.com/nazo6/flexagent", storage)).toEqual({
      agent: "opencode",
      mode: "build",
    });
    expect(loadNewSessionPrefs("github.com/nazo6/other", storage)).toBeNull();
  });

  it("returns null when nothing meaningful is stored", () => {
    const storage = memoryStorage();
    expect(loadNewSessionPrefs("p", storage)).toBeNull();
    saveNewSessionPrefs("p", { agent: "", mode: "default" }, storage);
    expect(loadNewSessionPrefs("p", storage)).toBeNull();
    expect(loadNewSessionPrefs("", storage)).toBeNull();
    expect(loadNewSessionPrefs("p", null)).toBeNull();
  });

  it("ignores invalid JSON and non-string mode values", () => {
    const storage = memoryStorage();
    storage.setItem("fxg:new_session_prefs:p", "{not json");
    expect(loadNewSessionPrefs("p", storage)).toBeNull();

    storage.setItem("fxg:new_session_prefs:p", JSON.stringify({ agent: "acp-x", mode: 42 }));
    expect(loadNewSessionPrefs("p", storage)).toEqual({
      agent: "acp-x",
      mode: "default",
    });
  });

  it("round-trips per-project, per-agent config values", () => {
    const storage = memoryStorage();
    saveSessionConfigOption("p", "opencode", "model", "opencode-go/deepseek-v4.1-flash", storage);
    saveSessionConfigOption("p", "opencode", "thought_level", "high", storage);
    expect(loadSessionConfig("p", "opencode", storage)).toEqual({
      model: "opencode-go/deepseek-v4.1-flash",
      thought_level: "high",
    });
    // Another agent, another project, or no agent keeps its own memory
    expect(loadSessionConfig("p", "gemini", storage)).toEqual({});
    expect(loadSessionConfig("other", "opencode", storage)).toEqual({});
    expect(loadSessionConfig("p", "", storage)).toEqual({});
  });

  it("keeps config keys apart when IDs contain colons", () => {
    const storage = memoryStorage();
    saveSessionConfigOption("local:box:1", "a:b", "model", "provider/model", storage);
    expect(loadSessionConfig("local:box:1", "a:b", storage)).toEqual({
      model: "provider/model",
    });
    expect(loadSessionConfig("local:box:1:a", "b", storage)).toEqual({});
  });

  it("does not save config without an agent", () => {
    const storage = memoryStorage();
    saveSessionConfigOption("p", "", "model", "provider/model", storage);
    expect(loadSessionConfig("p", "", storage)).toEqual({});
    expect(storage.getItem("fxg:session_config:p:")).toBeNull();
  });

  it("ignores the legacy project-wide config", () => {
    const storage = memoryStorage();
    // Written by versions before model memory was scoped by agent. The agent
    // cannot be recovered, so it must not leak into another agent's probe.
    storage.setItem("fxg:session_config:p", JSON.stringify({ model: "gemini-3.8-flash" }));
    expect(loadSessionConfig("p", "opencode", storage)).toEqual({});
  });

  it("toggles model favorites per agent", () => {
    const storage = memoryStorage();
    expect(loadModelFavorites("agent-a", storage)).toEqual([]);

    expect(toggleModelFavorite("agent-a", "openai/gpt-5", storage)).toEqual(["openai/gpt-5"]);
    expect(toggleModelFavorite("agent-a", "anthropic/claude", storage)).toEqual([
      "openai/gpt-5",
      "anthropic/claude",
    ]);
    // Another agent keeps its own list
    expect(loadModelFavorites("agent-b", storage)).toEqual([]);

    expect(toggleModelFavorite("agent-a", "openai/gpt-5", storage)).toEqual(["anthropic/claude"]);
    expect(loadModelFavorites("agent-a", storage)).toEqual(["anthropic/claude"]);
  });

  it("drops malformed favorite entries", () => {
    const storage = memoryStorage();
    storage.setItem("fxg:model_favorites:agent-a", JSON.stringify(["a", 42, "", "a", "b"]));
    expect(loadModelFavorites("agent-a", storage)).toEqual(["a", "b"]);

    storage.setItem("fxg:model_favorites:agent-a", "{not json");
    expect(loadModelFavorites("agent-a", storage)).toEqual([]);

    storage.setItem("fxg:model_favorites:agent-a", JSON.stringify({ a: 1 }));
    expect(loadModelFavorites("agent-a", storage)).toEqual([]);

    expect(loadModelFavorites("", storage)).toEqual([]);
    expect(loadModelFavorites("agent-a", null)).toEqual([]);
  });

  it("handles send shortcut preference and shouldSubmitOnEnter", () => {
    const storage = memoryStorage();
    expect(loadSendShortcutPreference(storage)).toBe("enter");

    saveSendShortcutPreference("modifier_enter", storage);
    expect(loadSendShortcutPreference(storage)).toBe("modifier_enter");

    // "enter" mode tests
    const enterEvt = {
      key: "Enter",
      shiftKey: false,
      ctrlKey: false,
      metaKey: false,
      isComposing: false,
      keyCode: 13,
    } as KeyboardEvent;
    expect(shouldSubmitOnEnter(enterEvt, "enter")).toBe(true);

    const shiftEnterEvt = { ...enterEvt, shiftKey: true } as KeyboardEvent;
    expect(shouldSubmitOnEnter(shiftEnterEvt, "enter")).toBe(false);

    const ctrlEnterEvt = { ...enterEvt, ctrlKey: true } as KeyboardEvent;
    expect(shouldSubmitOnEnter(ctrlEnterEvt, "enter")).toBe(true);

    // IME composition tests
    const composingEvt = { ...enterEvt, isComposing: true } as KeyboardEvent;
    expect(shouldSubmitOnEnter(composingEvt, "enter")).toBe(false);

    const ime229Evt = { ...enterEvt, keyCode: 229 } as KeyboardEvent;
    expect(shouldSubmitOnEnter(ime229Evt, "enter")).toBe(false);

    // "modifier_enter" mode tests
    expect(shouldSubmitOnEnter(enterEvt, "modifier_enter")).toBe(false);
    expect(shouldSubmitOnEnter(shiftEnterEvt, "modifier_enter")).toBe(false);
    expect(shouldSubmitOnEnter(ctrlEnterEvt, "modifier_enter")).toBe(true);

    const metaEnterEvt = { ...enterEvt, metaKey: true } as KeyboardEvent;
    expect(shouldSubmitOnEnter(metaEnterEvt, "modifier_enter")).toBe(true);
  });
});
