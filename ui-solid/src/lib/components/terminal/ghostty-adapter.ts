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

const FONT_SIZE = 13;
const FONT_FAMILY = "'Cascadia Mono', 'JetBrains Mono', Consolas, 'Courier New', monospace";

/**
 * Editing keys an active IME reports as `beforeinput` edits instead of
 * `keydown` (whose `keyCode === 229` the terminal drops).
 */
const CONTROL_INPUT: Partial<Record<string, string>> = {
  deleteContentBackward: "\x7f",
  deleteContentForward: "\x1b[3~",
  insertLineBreak: "\r",
  insertParagraph: "\r",
  insertTab: "\t",
};

/** Standard adapter backed by `ghostty-web` (libghostty WASM + Canvas 2D). */
export class GhosttyWebAdapter implements ITerminalAdapter {
  #terminal: Terminal | null = null;
  #fitAddon: FitAddon | null = null;
  #element: HTMLElement | null = null;
  /** Hidden textarea `ghostty-web` creates; the IME composes here. */
  #input: HTMLTextAreaElement | null = null;
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
        fontSize: FONT_SIZE,
        cursorBlink: true,
        scrollback: 5_000,
        theme: TERMINAL_THEME,
        fontFamily: FONT_FAMILY,
      });
      const fitAddon = new FitAddon();
      terminal.loadAddon(fitAddon);
      terminal.open(element);
      this.#terminal = terminal;
      this.#fitAddon = fitAddon;
      this.#element = element;
      this.#input = element.querySelector<HTMLTextAreaElement>("textarea");
      this.#disposers.push(
        terminal.onData((data) => {
          for (const listener of this.#dataListeners) listener(data);
        }),
        terminal.onResize(({ cols, rows }) => {
          for (const listener of this.#resizeListeners) listener({ cols, rows });
        }),
        ...this.#attachInput(element),
      );
      fitAddon.observeResize();
      fitAddon.fit();
      for (const chunk of this.#pendingWrites.splice(0)) terminal.write(chunk);
      this.focus();
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
    // Focus the hidden textarea instead of the wrapper `ghostty-web` would
    // focus, so an IME has a real editable target (see `#attachInput`).
    if (this.#input !== null) {
      this.#input.focus({ preventScroll: true });
      return;
    }
    this.#terminal?.focus();
  }

  dispose(): void {
    for (const disposer of this.#disposers.splice(0)) disposer.dispose();
    this.#fitAddon?.dispose();
    this.#terminal?.dispose();
    this.#terminal = null;
    this.#fitAddon = null;
    this.#element = null;
    this.#input = null;
    this.#pendingWrites = [];
    this.#dataListeners.clear();
    this.#resizeListeners.clear();
  }

  /**
   * Routes keyboard focus, IME composition and browser-inserted text through
   * the hidden textarea.
   *
   * `ghostty-web` 0.4.0 focuses the contenteditable wrapper rather than the
   * textarea and cancels every `beforeinput` that bubbles out of it, while its
   * key handler drops every key with `isComposing` / `keyCode === 229`. With an
   * IME enabled (Japanese/Chinese/Korean) the browser therefore delivered text
   * as `beforeinput` on the textarea — which the wrapper then cancelled — and
   * nothing reached the PTY, while Enter still worked (upstream #119, #120,
   * #190).
   *
   * Composing in the real textarea fixes this: the preedit is drawn as the
   * textarea's own text at the cursor cell, `beforeinput` is stopped before the
   * wrapper can cancel it, and inserted text is read back from the field (the
   * `input` event, or `compositionend` when the browser inserts nothing).
   */
  #attachInput(element: HTMLElement): { dispose(): void }[] {
    const input = this.#input;
    if (input === null) return [];
    // The textarea is absolutely positioned; anchor it to the terminal instead
    // of whichever ancestor happens to be positioned.
    if (getComputedStyle(element).position === "static") {
      element.style.position = "relative";
    }
    const disposers: { dispose(): void }[] = [];
    const listen = <K extends keyof HTMLElementEventMap>(
      target: HTMLElement,
      type: K,
      listener: (event: HTMLElementEventMap[K]) => void,
    ): void => {
      target.addEventListener(type, listener);
      disposers.push({ dispose: () => target.removeEventListener(type, listener) });
    };
    const focusInput = (): void => input.focus({ preventScroll: true });
    const send = (data: string): void => {
      for (const listener of this.#dataListeners) listener(data);
    };

    // `ghostty-web` focuses the wrapper on click and on `focus()`; take over
    // every focus that lands on it. A click reaches the wrapper after its own
    // handlers, so this bubble-phase listener runs last.
    listen(element, "focusin", (event) => {
      if (event.target !== input) focusInput();
    });
    listen(element, "mousedown", focusInput);

    listen(input, "compositionstart", (event) => {
      // Keep the terminal's own composition listeners out of the picture: the
      // adapter owns the text path, so a commit cannot be sent twice.
      event.stopPropagation();
      this.#showPreedit();
    });
    listen(input, "compositionupdate", (event) => {
      event.stopPropagation();
      this.#showPreedit();
    });
    listen(input, "compositionend", (event) => {
      event.stopPropagation();
      this.#hidePreedit();
      // The commit usually arrives as the following `input` event. When the
      // browser has already delivered it the field is empty and this is a no-op,
      // so the text is sent exactly once whichever event comes first.
      queueMicrotask(() => this.#flushInput(input, send));
    });

    listen(input, "input", (event) => {
      const inputEvent = event as InputEvent;
      // Preedit updates: leave the composing text in the field for display.
      if (inputEvent.isComposing) return;
      this.#flushInput(input, send);
    });

    listen(input, "beforeinput", (event) => {
      const inputEvent = event as InputEvent;
      // The wrapper cancels `beforeinput` to keep its contenteditable clean,
      // which also cancelled the text the IME inserted here (the event bubbles).
      event.stopPropagation();
      if (inputEvent.isComposing) return;
      const data = CONTROL_INPUT[inputEvent.inputType];
      if (data === undefined) return;
      // Editing keys produce no text, so `input` never fires for them.
      event.preventDefault();
      send(data);
    });
    return disposers;
  }

  /** Sends and drops whatever text the browser inserted into the field. */
  #flushInput(input: HTMLTextAreaElement, send: (data: string) => void): void {
    const text = input.value;
    if (text.length === 0) return;
    input.value = "";
    send(text);
  }

  /** Draws the composing text at the cursor cell (the canvas cannot show it). */
  #showPreedit(): void {
    const terminal = this.#terminal;
    const element = this.#element;
    const input = this.#input;
    if (terminal === null || element === null || input === null) return;
    const canvas = element.querySelector("canvas");
    if (canvas === null) return;
    const canvasRect = canvas.getBoundingClientRect();
    const elementRect = element.getBoundingClientRect();
    const cols = Math.max(terminal.cols, 1);
    const rows = Math.max(terminal.rows, 1);
    const cellWidth = canvasRect.width / cols;
    const cellHeight = canvasRect.height / rows;
    const cursor = terminal.buffer.active;
    // Absolute offsets resolve against the padding box, so drop the border.
    const originLeft = elementRect.left + element.clientLeft;
    const originTop = elementRect.top + element.clientTop;
    const left = canvasRect.left - originLeft + cursor.cursorX * cellWidth;
    const top = canvasRect.top - originTop + cursor.cursorY * cellHeight;

    input.style.left = `${left}px`;
    input.style.top = `${top}px`;
    input.style.width = `${Math.max(cellWidth, canvasRect.width - cursor.cursorX * cellWidth)}px`;
    input.style.height = `${cellHeight}px`;
    input.style.lineHeight = `${cellHeight}px`;
    input.style.fontSize = `${FONT_SIZE}px`;
    input.style.fontFamily = FONT_FAMILY;
    input.style.color = TERMINAL_THEME.foreground;
    input.style.background = "transparent";
    input.style.caretColor = "transparent";
    input.style.opacity = "1";
    input.style.clipPath = "none";
  }

  /** Hides the preedit again. */
  #hidePreedit(): void {
    const input = this.#input;
    if (input === null) return;
    input.style.left = "0";
    input.style.top = "0";
    input.style.width = "1px";
    input.style.height = "1px";
    input.style.lineHeight = "";
    input.style.opacity = "0";
    input.style.clipPath = "inset(50%)";
  }
}
