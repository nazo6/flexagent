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
      dotColorClass: "bg-destructive",
      badgeVariant: "destructive",
    };
  }

  switch (node.lifecycle_status as NodeLifecycleStatus) {
    case "ready":
      return {
        isAvailable: true,
        statusText: m.status_online(),
        dotColorClass: "bg-emerald-500",
        badgeVariant: "secondary",
      };
    case "bootstrapping":
    case "provisioning":
      return {
        isAvailable: false,
        statusText: m.status_preparing(),
        dotColorClass: "bg-amber-500 animate-pulse",
        badgeVariant: "outline",
      };
    case "draining":
      return {
        isAvailable: false,
        statusText: m.status_stopping(),
        dotColorClass: "bg-amber-600",
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
        dotColorClass: "bg-destructive",
        badgeVariant: "destructive",
      };
    default:
      return {
        isAvailable: node.is_online,
        statusText: node.is_online ? m.status_active() : m.status_offline(),
        dotColorClass: node.is_online ? "bg-emerald-500" : "bg-muted-foreground",
        badgeVariant: "outline",
      };
  }
}
