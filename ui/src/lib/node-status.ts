import type { NodeLifecycleStatus, NodeSummary } from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";

export interface NodeAvailability {
  isAvailable: boolean;
  statusText: string;
  dotColorClass: string;
  badgeVariant: "default" | "secondary" | "destructive" | "outline";
}

/**
 * Determine a node's availability and return properties for UI display.
 */
export function getNodeAvailability(node: NodeSummary | null | undefined): NodeAvailability {
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
    case "bootstrapping":
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
