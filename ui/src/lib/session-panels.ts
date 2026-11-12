import type { Component } from "svelte";
import { m } from "#lib/paraglide/messages.js";
import FileCodeIcon from "@lucide/svelte/icons/file-code";
import SquareTerminalIcon from "@lucide/svelte/icons/square-terminal";

/** Panels available in the session tool rail. */
export type SessionPanelId = "diff" | "terminal";

export interface SessionPanelDefinition {
  id: SessionPanelId;
  label: () => string;
  icon: Component;
}

/**
 * Ordered tool list. Adding a panel here makes it appear in both the desktop
 * rail and the mobile tool sheet; the host only needs a render branch.
 */
export const SESSION_PANELS: SessionPanelDefinition[] = [
  {
    id: "diff",
    label: () => m.session_diff_panel_title(),
    icon: FileCodeIcon,
  },
  {
    id: "terminal",
    label: () => m.session_terminal_panel_title(),
    icon: SquareTerminalIcon,
  },
];

export function sessionPanel(id: SessionPanelId): SessionPanelDefinition {
  const panel = SESSION_PANELS.find((candidate) => candidate.id === id);
  if (panel === undefined) {
    throw new Error(`unknown session panel: ${id}`);
  }
  return panel;
}
