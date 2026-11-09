import { FitAddon, Terminal, init } from "ghostty-web";
import type { ITerminalAdapter, TerminalDimensions } from "./types";

/** Dark terminal color palette (matches the Desk dark background). */
const TERMINAL_THEME = {
  background: "#17140f",
  foreground: "#ece6da",
  cursor: "#e0a05a",
  cursorAccent: "#17140f",
  selectionBackground: "#3a3327",
  black: "#17140f",
  red: "#f08a7e",
  green: "#9ac58c",
  yellow: "#e0a95e",
  blue: "#93b8f0",
  magenta: "#c9a7e8",
  cyan: "#8fcfc4",
  white: "#ece6da",
  brightBlack: "#9a9184",
  brightRed: "#f4a79d",
  brightGreen: "#b3d8a8",
  brightYellow: "#efc98c",
  brightBlue: "#b3ccf5",
  brightMagenta: "#ddc3f2",
  brightCyan: "#b1e1da",
  brightWhite: "#faf6ec",
} as const;

/** Standard adapter backed by `ghostty-web` (libghostty WASM + Canvas 2D). */
export class GhosttyWebAdapter implements ITerminalAdapter {
  #terminal: Terminal | null = null;
  #fitAddon: FitAddon | null = null;
  #pendingWrites: (string | Uint8Array)[] = [];
  #dataListeners = new Set<(data: string) => void>();
  #resizeListeners = new Set<(dims: TerminalDimensions) => void>();
  #disposers: { dispose(): void }[] = [];

  mount(element: HTMLElement): void {
    if (this.#terminal !== null) return;
    // WASM initialization is async, so writes are buffered until it completes
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

  clear(): void {
    // Also drop writes that have not been flushed before WASM initialization
    // completes (prevents leftovers from the previous session)
    this.#pendingWrites = [];
    this.#terminal?.clear();
  }

  onData(callback: (data: string) => void): { dispose: () => void } {
    this.#dataListeners.add(callback);
    return { dispose: () => this.#dataListeners.delete(callback) };
  }

  onResize(callback: (dims: TerminalDimensions) => void): {
    dispose: () => void;
  } {
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
