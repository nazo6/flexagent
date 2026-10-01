import { describe, expect, it } from "vitest";
import { loadNewSessionPrefs, saveNewSessionPrefs } from "./session-prefs";

/** テスト用の最小 Storage 実装 (Node 環境でも localStorage を模す)。 */
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
      { agent: "opencode2", mode: "plan", opencodeMode: "acp" },
      storage,
    );
    expect(loadNewSessionPrefs("github.com/nazo6/flexagent", storage)).toEqual({
      agent: "opencode2",
      mode: "plan",
      opencodeMode: "acp",
    });
    expect(loadNewSessionPrefs("github.com/nazo6/other", storage)).toBeNull();
  });

  it("returns null when nothing meaningful is stored", () => {
    const storage = memoryStorage();
    expect(loadNewSessionPrefs("p", storage)).toBeNull();
    saveNewSessionPrefs("p", { agent: "", mode: "default", opencodeMode: "default" }, storage);
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

    storage.setItem(
      "fxg:new_session_prefs:p",
      JSON.stringify({ agent: "acp-x", mode: "bogus", opencodeMode: "bridge" }),
    );
    expect(loadNewSessionPrefs("p", storage)).toEqual({
      agent: "acp-x",
      mode: "default",
      opencodeMode: "bridge",
    });
  });
});
