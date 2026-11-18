import { createRoot } from "solid-js";
import { ReactiveCell } from "#lib/stores/reactive.ts";
import {
  loadSendShortcutPreference,
  saveSendShortcutPreference,
  type SendShortcut,
} from "#lib/session-prefs.ts";

/**
 * Reactive user preferences store for the browser.
 */
export class PreferencesStore {
  readonly #sendShortcut: ReactiveCell<SendShortcut>;

  constructor() {
    const stored = typeof localStorage === "undefined" ? "enter" : loadSendShortcutPreference();
    this.#sendShortcut = new ReactiveCell<SendShortcut>(stored);
  }

  get sendShortcut(): SendShortcut {
    return this.#sendShortcut.get();
  }

  set sendShortcut(shortcut: SendShortcut) {
    this.#sendShortcut.set(shortcut);
  }

  setSendShortcut(shortcut: SendShortcut): void {
    this.sendShortcut = shortcut;
    saveSendShortcutPreference(shortcut);
  }
}

export const prefs = createRoot(() => new PreferencesStore());
