/** ターミナルの表示寸法 (列数 / 行数)。 */
export interface TerminalDimensions {
  cols: number;
  rows: number;
}

/**
 * Web ターミナルの抽象化インターフェース (docs/05 §3.4)。
 *
 * `ghostty-web` (WASM / Canvas) を標準実装とし、将来的な DOM レンダラ
 * (`wterm` 等) への差し替えやテスト容易性を確保する。
 */
export interface ITerminalAdapter {
  mount(element: HTMLElement): void;
  write(data: string | Uint8Array): void;
  onData(callback: (data: string) => void): { dispose: () => void };
  onResize(callback: (dims: TerminalDimensions) => void): { dispose: () => void };
  fit(): TerminalDimensions;
  focus(): void;
  dispose(): void;
}
