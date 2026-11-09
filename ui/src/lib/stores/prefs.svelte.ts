import {
  loadSendShortcutPreference,
  saveSendShortcutPreference,
  type SendShortcut,
} from "#lib/session-prefs.ts";

/**
 * Reactive user preferences store for the browser.
 */
class PreferencesStore {
  sendShortcut = $state<SendShortcut>("enter");

  constructor() {
    if (typeof localStorage !== "undefined") {
      this.sendShortcut = loadSendShortcutPreference();
    }
  }

  setSendShortcut(shortcut: SendShortcut): void {
    this.sendShortcut = shortcut;
    saveSendShortcutPreference(shortcut);
  }
}

export const prefs = new PreferencesStore();
