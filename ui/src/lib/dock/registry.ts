import type { Component } from "svelte";
import { m } from "#lib/paraglide/messages.js";
import FileCodeIcon from "@lucide/svelte/icons/file-code";
import FolderTreeIcon from "@lucide/svelte/icons/folder-tree";
import SquareTerminalIcon from "@lucide/svelte/icons/square-terminal";

/** Fixed tools of the session dock. */
export type DockToolId = "files" | "changes" | "terminal";

/** Tools that can be launched from the rail / mobile header. */
export type DockLauncherId = DockToolId;

export interface DockToolDefinition {
  id: DockToolId;
  label: () => string;
  icon: Component;
}

/**
 * Ordered tool registry. Every entry is a fixed, singleton tool; resource
 * views (files) live inside the owning tool instead of becoming outer tabs.
 */
export const DOCK_TOOLS: readonly DockToolDefinition[] = [
  {
    id: "files",
    label: () => m.dock_files_title(),
    icon: FolderTreeIcon,
  },
  {
    id: "changes",
    label: () => m.dock_changes_title(),
    icon: FileCodeIcon,
  },
  {
    id: "terminal",
    label: () => m.dock_terminal_title(),
    icon: SquareTerminalIcon,
  },
];

/** Tools shown in the desktop rail and the mobile header. */
export const DOCK_LAUNCHERS: readonly DockToolDefinition[] = DOCK_TOOLS;

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
