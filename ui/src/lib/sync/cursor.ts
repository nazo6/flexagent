/** 受信バッチのカーソル適用判定結果。 */
export interface CursorUpdate {
  /** 配信元ストアの再作成・リセット (カーソル巻き戻り) を検知したか */
  reset: boolean;
  /** 適用後のカーソル */
  cursor: number;
}

/**
 * 受信バッチのカーソルを現在値と突き合わせる。
 *
 * カーソルは配信元ストアへの取り込み順で単調増加するため、`incoming` が
 * `current` より小さい場合はストアの再作成・リセット (DB リセット) を意味する。
 * その場合はリセットを通知して新しいカーソルを採用し、呼び出し側で
 * タイムライン等のローカル状態を破棄して 0 から再同期する。
 */
export function mergeCursor(current: number | null, incoming: number): CursorUpdate {
  if (current !== null && incoming < current) {
    return { reset: true, cursor: incoming };
  }
  if (current === null || incoming > current) {
    return { reset: false, cursor: incoming };
  }
  return { reset: false, cursor: current };
}
