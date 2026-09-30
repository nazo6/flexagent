import { ConnectionStore } from "./connection.svelte";
import { SyncStore } from "./sync.svelte";

/** 接続先 (中央サーバー / ローカルノード) と認証状態のシングルトン。 */
export const connection = new ConnectionStore();

/** Client WS 差分同期のシングルトン。 */
export const sync = new SyncStore(connection);

/**
 * アプリ起動時の初期化 (`+layout.svelte` から一度だけ呼ぶ)。
 *
 * 1. `GET /api/v1/system/info` で認証状態を確認
 * 2. 認証済みなら Client WS を開始 (未認証はトークンダイアログを表示)
 */
export async function initApp(): Promise<void> {
  await connection.probe();
  if (connection.authenticated) sync.start();
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
