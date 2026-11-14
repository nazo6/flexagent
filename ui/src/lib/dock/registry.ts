import type { Component } from "svelte";
import { m } from "#lib/paraglide/messages.js";
import FileCodeIcon from "@lucide/svelte/icons/file-code";
import SquareTerminalIcon from "@lucide/svelte/icons/square-terminal";

/** Tools that can appear in the session workspace dock. */
export type DockToolId = "changes" | "terminal";

export interface DockToolDefinition {
  id: DockToolId;
  label: () => string;
  icon: Component;
  /** Listed in the desktop rail and the mobile tool sheet. */
  launcher: boolean;
  /** One tab per workspace; opening it again focuses the existing tab. */
  singleton: boolean;
  /** Keep the view mounted while the dock is open, even when another tab is active. */
  keepMounted: boolean;
}

/**
 * Ordered tool registry. A tool becomes available in the rail and the mobile
 * sheet by being listed here; the dock host owns its render branch.
 */
export const DOCK_TOOLS: readonly DockToolDefinition[] = [
  {
    id: "changes",
    label: () => m.dock_changes_title(),
    icon: FileCodeIcon,
    launcher: true,
    singleton: true,
    // Scope and file selection stay in the component, so hidden tabs keep them.
    keepMounted: true,
  },
  {
    id: "terminal",
    label: () => m.dock_terminal_title(),
    icon: SquareTerminalIcon,
    launcher: true,
    singleton: true,
    // Keeping the view mounted keeps the PTY attached while the dock is open.
    keepMounted: true,
  },
];

export function dockTool(id: DockToolId): DockToolDefinition {
  const tool = DOCK_TOOLS.find((candidate) => candidate.id === id);
  if (tool === undefined) {
    throw new Error(`unknown dock tool: ${id}`);
  }
  return tool;
}

export function isDockToolId(value: unknown): value is DockToolId {
  return typeof value === "string" && DOCK_TOOLS.some((tool) => tool.id === value);
}
