import { describe, expect, it } from "vitest";
import {
  loadNewSessionPrefs,
  loadSendShortcutPreference,
  loadSessionConfig,
  saveNewSessionPrefs,
  saveSendShortcutPreference,
  saveSessionConfigOption,
  shouldSubmitOnEnter,
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

  it("ignores invalid JSON and invalid enum values", () => {
    const storage = memoryStorage();
    storage.setItem("fxg:new_session_prefs:p", "{not json");
    expect(loadNewSessionPrefs("p", storage)).toBeNull();

    storage.setItem("fxg:new_session_prefs:p", JSON.stringify({ mode: "bogus", agent: 42 }));
    expect(loadNewSessionPrefs("p", storage)).toBeNull();

    storage.setItem("fxg:new_session_prefs:p", JSON.stringify({ agent: "acp-x", mode: "bogus" }));
    expect(loadNewSessionPrefs("p", storage)).toEqual({
      agent: "acp-x",
      mode: "default",
    });
  });

  it("round-trips per-project config values", () => {
    const storage = memoryStorage();
    saveSessionConfigOption("p", "model", "opencode-go/deepseek-v4.1-flash", storage);
    saveSessionConfigOption("p", "thought_level", "high", storage);
    expect(loadSessionConfig("p", storage)).toEqual({
      model: "opencode-go/deepseek-v4.1-flash",
      thought_level: "high",
    });
    expect(loadSessionConfig("other", storage)).toEqual({});
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
