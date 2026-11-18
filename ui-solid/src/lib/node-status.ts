import type { HubLinkState, NodeLifecycleStatus, NodeSummary } from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";

/** Localized label for a node's link state to the hub. */
export function hubLinkLabel(state: HubLinkState) {
  switch (state) {
    case "connected":
      return m.shell_connected();
    case "connecting":
      return m.shell_hub_connecting();
    case "unauthorized":
      return m.shell_hub_unauthorized();
    case "failed":
      return m.shell_hub_unreachable();
    case "standalone":
      return m.shell_hub_standalone();
  }
}

export interface NodeAvailability {
  isAvailable: boolean;
  statusText: string;
  dotColorClass: string;
  badgeVariant: "default" | "secondary" | "destructive" | "outline";
}

/**
 * Determine a node's availability and return properties for UI display.
 *
 * `connected` is this client's link to the server (`sync.wsConnected`). While
 * the link is down the cached node snapshot is stale, so every node is reported
 * as disconnected instead of keeping a green "Online" that no longer holds.
 */
export function getNodeAvailability(
  node: NodeSummary | null | undefined,
  connected: boolean,
): NodeAvailability {
  if (!connected) {
    return {
      isAvailable: false,
      statusText: m.shell_disconnected(),
      dotColorClass: "bg-danger",
      badgeVariant: "outline",
    };
  }

  if (!node) {
    return {
      isAvailable: false,
      statusText: m.status_node_not_found(),
      dotColorClass: "bg-muted-foreground",
      badgeVariant: "outline",
    };
  }

  if (!node.is_online) {
    return {
      isAvailable: false,
      statusText: m.status_offline(),
      dotColorClass: "bg-danger",
      badgeVariant: "destructive",
    };
  }

  switch (node.lifecycle_status as NodeLifecycleStatus) {
    case "ready":
      return {
        isAvailable: true,
        statusText: m.status_online(),
        dotColorClass: "bg-ok",
        badgeVariant: "secondary",
      };
    case "provisioning":
      return {
        isAvailable: false,
        statusText: m.status_preparing(),
        dotColorClass: "bg-warn animate-pulse",
        badgeVariant: "outline",
      };
    case "draining":
      return {
        isAvailable: false,
        statusText: m.status_stopping(),
        dotColorClass: "bg-warn",
        badgeVariant: "outline",
      };
    case "terminated":
      return {
        isAvailable: false,
        statusText: m.status_terminated(),
        dotColorClass: "bg-muted-foreground",
        badgeVariant: "outline",
      };
    case "error":
      return {
        isAvailable: false,
        statusText: m.status_error(),
        dotColorClass: "bg-danger",
        badgeVariant: "destructive",
      };
    default:
      return {
        isAvailable: node.is_online,
        statusText: node.is_online ? m.status_active() : m.status_offline(),
        dotColorClass: node.is_online ? "bg-ok" : "bg-muted-foreground",
        badgeVariant: "outline",
      };
  }
}
