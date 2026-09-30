import { ConnectionStore } from "./connection.svelte";
import { SyncStore } from "./sync.svelte";

/** 接続先 (中央サーバー / ローカルノード) と認証状態のシングルトン。 */
export const connection = new ConnectionStore();

/** Client WS 差分同期のシングルトン。 */
export const sync = new SyncStore(connection);

/**
 * アプリ起動時の初期化 (`+layout.svelte` から一度だけ呼ぶ)。
 *
 * 1. URL の `?token=` (`fxg web` が生成するトークン付き URL) があればそれで
 *    ログインし、クエリから除去する (履歴にトークンを残さない)
 * 2. `GET /api/v1/system/info` で認証状態を確認
 * 3. 認証済みなら Client WS を開始 (未認証はトークンダイアログを表示)
 */
export async function initApp(): Promise<void> {
  const urlToken = readUrlToken();
  if (urlToken !== null) {
    stripUrlToken();
    try {
      await loginAndStart(urlToken);
      return;
    } catch {
      /* 無効なトークンはダイアログ入力へフォールバックする */
    }
  }
  await connection.probe();
  if (connection.authenticated) sync.start();
}

/** URL の `?token=` を読み取る (無ければ null)。 */
function readUrlToken(): string | null {
  if (typeof location === "undefined") return null;
  const token = new URLSearchParams(location.search).get("token");
  return token !== null && token.trim() !== "" ? token.trim() : null;
}

/** `?token=` を履歴から除去する (アドレスバー / 履歴に残さない)。 */
function stripUrlToken(): void {
  if (typeof history === "undefined" || typeof location === "undefined") return;
  const url = new URL(location.href);
  url.searchParams.delete("token");
  history.replaceState(null, "", `${url.pathname}${url.search}${url.hash}`);
}

/** トークン入力ダイアログからのログイン + 同期開始。 */
export async function loginAndStart(token: string): Promise<void> {
  await connection.login(token);
  sync.restart();
}

/** ログアウト (同期停止 + Cookie 失効)。 */
export async function logoutAndStop(): Promise<void> {
  sync.stop();
  await connection.logout();
}
