/** Terminal display dimensions (columns / rows). */
export interface TerminalDimensions {
  cols: number;
  rows: number;
}

/**
 * Abstraction interface for the web terminal (docs/07-web-ui-and-pwa.md §4).
 *
 * `ghostty-web` (WASM / Canvas) is the standard implementation; this keeps the
 * renderer replaceable (for example with a DOM renderer such as `wterm`) and
 * makes testing easier.
 */
export interface ITerminalAdapter {
  mount(element: HTMLElement): void;
  /** Clear the display (for example on session switch, leaving it empty until the next output). */
  clear(): void;
  write(data: string | Uint8Array): void;
  onData(callback: (data: string) => void): { dispose: () => void };
  onResize(callback: (dims: TerminalDimensions) => void): {
    dispose: () => void;
  };
  fit(): TerminalDimensions;
  focus(): void;
  dispose(): void;
}
