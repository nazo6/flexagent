import type { Component } from "svelte";
import { m } from "#lib/paraglide/messages.js";
import FileCodeIcon from "@lucide/svelte/icons/file-code";
import FileIcon from "@lucide/svelte/icons/file";
import FolderTreeIcon from "@lucide/svelte/icons/folder-tree";
import SquareTerminalIcon from "@lucide/svelte/icons/square-terminal";
import { basename } from "./paths.ts";

/** Tools that can be launched from the rail / mobile header. */
export type DockLauncherId = "files" | "changes" | "terminal";

/** Tools that can appear as a tab (including resource-backed file tabs). */
export type DockToolId = DockLauncherId | "file";

export interface DockToolDefinition {
  id: DockToolId;
  label: () => string;
  icon: Component;
  /** Listed in the desktop rail and the mobile header. */
  launcher: boolean;
  /** One tab per workspace; opening it again focuses the existing tab. */
  singleton: boolean;
  /** Keep the view mounted while the dock is open, even when another tab is active. */
  keepMounted: boolean;
}

/**
 * Ordered tool registry. A tool becomes available in the rail and the mobile
 * header by being listed here; the dock host owns its render branch.
 */
export const DOCK_TOOLS: readonly DockToolDefinition[] = [
  {
    id: "files",
    label: () => m.dock_files_title(),
    icon: FolderTreeIcon,
    launcher: true,
    singleton: true,
    // The expanded tree lives in the component; keep it across tab switches.
    keepMounted: true,
  },
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
  {
    id: "file",
    label: () => m.dock_file_title(),
    icon: FileIcon,
    launcher: false,
    singleton: false,
    // File tabs are read-only in Phase 1; reload them when they become active.
    keepMounted: false,
  },
];

/** Tools shown in the desktop rail and the mobile header. */
export const DOCK_LAUNCHERS: readonly (DockToolDefinition & { id: DockLauncherId })[] =
  DOCK_TOOLS.filter((tool): tool is DockToolDefinition & { id: DockLauncherId } => tool.launcher);

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

/** Display title of a tab (file tabs use the basename). */
export function dockTabLabel(tab: { tool: DockToolId; resource: string | null }): string {
  if (tab.tool === "file" && tab.resource !== null) {
    return basename(tab.resource);
  }
  return dockTool(tab.tool).label();
}
