import { FitAddon, Terminal, init } from "ghostty-web";
import type { ITerminalAdapter, TerminalDimensions } from "./types";

/** ダークテーマの端末配色 (アプリのダーク背景に合わせる)。 */
const TERMINAL_THEME = {
  background: "#0f172a",
  foreground: "#e2e8f0",
  cursor: "#7dd3fc",
  cursorAccent: "#0f172a",
  selectionBackground: "#334155",
  black: "#0f172a",
  red: "#f87171",
  green: "#4ade80",
  yellow: "#facc15",
  blue: "#60a5fa",
  magenta: "#c084fc",
  cyan: "#22d3ee",
  white: "#e2e8f0",
  brightBlack: "#64748b",
  brightRed: "#fca5a5",
  brightGreen: "#86efac",
  brightYellow: "#fde047",
  brightBlue: "#93c5fd",
  brightMagenta: "#d8b4fe",
  brightCyan: "#67e8f9",
  brightWhite: "#f8fafc",
} as const;

/** `ghostty-web` (libghostty WASM + Canvas 2D) による標準アダプタ。 */
export class GhosttyWebAdapter implements ITerminalAdapter {
  #terminal: Terminal | null = null;
  #fitAddon: FitAddon | null = null;
  #pendingWrites: (string | Uint8Array)[] = [];
  #dataListeners = new Set<(data: string) => void>();
  #resizeListeners = new Set<(dims: TerminalDimensions) => void>();
  #disposers: { dispose(): void }[] = [];

  mount(element: HTMLElement): void {
    if (this.#terminal !== null) return;
    // WASM 初期化は非同期のため、完了までの write はバッファリングする
    void (async () => {
      await init();
      const terminal = new Terminal({
        fontSize: 13,
        cursorBlink: true,
        scrollback: 5_000,
        theme: TERMINAL_THEME,
        fontFamily: "'Cascadia Mono', 'JetBrains Mono', Consolas, 'Courier New', monospace",
      });
      const fitAddon = new FitAddon();
      terminal.loadAddon(fitAddon);
      terminal.open(element);
      this.#terminal = terminal;
      this.#fitAddon = fitAddon;
      this.#disposers.push(
        terminal.onData((data) => {
          for (const listener of this.#dataListeners) listener(data);
        }),
        terminal.onResize(({ cols, rows }) => {
          for (const listener of this.#resizeListeners) listener({ cols, rows });
        }),
      );
      fitAddon.observeResize();
      fitAddon.fit();
      for (const chunk of this.#pendingWrites.splice(0)) terminal.write(chunk);
    })();
  }

  write(data: string | Uint8Array): void {
    if (this.#terminal === null) {
      this.#pendingWrites.push(data);
      return;
    }
    this.#terminal.write(data);
  }

  onData(callback: (data: string) => void): { dispose: () => void } {
    this.#dataListeners.add(callback);
    return { dispose: () => this.#dataListeners.delete(callback) };
  }

  onResize(callback: (dims: TerminalDimensions) => void): { dispose: () => void } {
    this.#resizeListeners.add(callback);
    return { dispose: () => this.#resizeListeners.delete(callback) };
  }

  fit(): TerminalDimensions {
    if (this.#terminal === null || this.#fitAddon === null) {
      return { cols: 80, rows: 24 };
    }
    this.#fitAddon.fit();
    return { cols: this.#terminal.cols, rows: this.#terminal.rows };
  }

  focus(): void {
    this.#terminal?.focus();
  }

  dispose(): void {
    for (const disposer of this.#disposers.splice(0)) disposer.dispose();
    this.#fitAddon?.dispose();
    this.#terminal?.dispose();
    this.#terminal = null;
    this.#fitAddon = null;
    this.#pendingWrites = [];
    this.#dataListeners.clear();
    this.#resizeListeners.clear();
  }
}
