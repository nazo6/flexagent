import type { JSX } from "@solidjs/web";
import { m } from "#lib/paraglide/messages.js";
import IconFileCode from "~icons/lucide/file-code";
import IconFolderTree from "~icons/lucide/folder-tree";
import IconSquareTerminal from "~icons/lucide/square-terminal";

/** Fixed tools of the session dock. */
export type DockToolId = "files" | "changes" | "terminal";

/** Tools that can be launched from the rail / mobile header. */
export type DockLauncherId = DockToolId;

export interface DockToolDefinition {
  id: DockToolId;
  label: () => string;
  icon: (props: { class?: string }) => JSX.Element;
}

/**
 * Ordered tool registry. Every entry is a fixed, singleton tool; resource
 * views (files) live inside the owning tool instead of becoming outer tabs.
 */
export const DOCK_TOOLS: readonly DockToolDefinition[] = [
  {
    id: "files",
    label: () => m.dock_files_title(),
    icon: IconFolderTree,
  },
  {
    id: "changes",
    label: () => m.dock_changes_title(),
    icon: IconFileCode,
  },
  {
    id: "terminal",
    label: () => m.dock_terminal_title(),
    icon: IconSquareTerminal,
  },
];
