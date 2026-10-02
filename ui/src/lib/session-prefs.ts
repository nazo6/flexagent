/**
 * 新規セッション画面の端末ごとの記憶 (localStorage)。
 *
 * プロジェクト単位で最後に使ったエージェント / 初期モード / OpenCode2 実行モードを
 * 保持する。端末ごとの好みであり、サーバー (ノード / 中央サーバー) には保存しない。
 */
export type SessionModePreference = "default" | "code" | "plan";

/** 新規セッション画面で復元する起動設定。 */
export interface NewSessionPrefs {
  /** 最後に使ったエージェントID (ノードに未導入ならカスタム入力へ復元する) */
  agent: string;
  /** エージェント初期モード */
  mode: SessionModePreference;
}

const KEY_PREFIX = "fxg:new_session_prefs:";
const SESSION_MODES: readonly SessionModePreference[] = ["default", "code", "plan"];

/** `localStorage` を解決する (SSR / テストでは引数で差し替える)。 */
function resolveStorage(storage?: Storage | null): Storage | null {
  if (storage !== undefined) return storage;
  return typeof localStorage === "undefined" ? null : localStorage;
}

function isSessionMode(value: unknown): value is SessionModePreference {
  return SESSION_MODES.includes(value as SessionModePreference);
}

/**
 * 保存済みの起動設定を読み出す。
 *
 * 未保存・不正な JSON・意味のある値が無い場合は `null` を返す。
 * 不正な列挙値は既定値 (`default`) に丸める。
 */
export function loadNewSessionPrefs(
  projectId: string,
  storage?: Storage | null,
): NewSessionPrefs | null {
  const store = resolveStorage(storage);
  if (store === null || projectId === "") return null;
  const raw = store.getItem(KEY_PREFIX + projectId);
  if (raw === null) return null;
  try {
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null) return null;
    const record = parsed as Record<string, unknown>;
    const prefs: NewSessionPrefs = {
      agent: typeof record.agent === "string" ? record.agent : "",
      mode: isSessionMode(record.mode) ? record.mode : "default",
    };
    if (prefs.agent === "" && prefs.mode === "default") {
      return null;
    }
    return prefs;
  } catch {
    return null;
  }
}

/** 起動設定を保存する (容量超過等の失敗は無視する)。 */
export function saveNewSessionPrefs(
  projectId: string,
  prefs: NewSessionPrefs,
  storage?: Storage | null,
): void {
  const store = resolveStorage(storage);
  if (store === null || projectId === "") return;
  try {
    store.setItem(KEY_PREFIX + projectId, JSON.stringify(prefs));
  } catch {
    // localStorage 無効・容量超過時は記憶しない
  }
}
