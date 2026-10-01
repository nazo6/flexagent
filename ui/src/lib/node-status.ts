import type { NodeLifecycleStatus } from "$lib/generated/NodeLifecycleStatus";
import type { NodeSummary } from "$lib/generated/NodeSummary";

export interface NodeAvailability {
  isAvailable: boolean;
  statusText: string;
  dotColorClass: string;
  badgeVariant: "default" | "secondary" | "destructive" | "outline";
}

/**
 * ノードの稼働状態を判定し、UI表示用のプロパティを返す。
 */
export function getNodeAvailability(node: NodeSummary | null | undefined): NodeAvailability {
  if (!node) {
    return {
      isAvailable: false,
      statusText: "ノード未検出",
      dotColorClass: "bg-muted-foreground",
      badgeVariant: "outline",
    };
  }

  if (!node.is_online) {
    return {
      isAvailable: false,
      statusText: "オフライン",
      dotColorClass: "bg-destructive",
      badgeVariant: "destructive",
    };
  }

  switch (node.lifecycle_status as NodeLifecycleStatus) {
    case "ready":
      return {
        isAvailable: true,
        statusText: "オンライン",
        dotColorClass: "bg-emerald-500",
        badgeVariant: "secondary",
      };
    case "bootstrapping":
    case "provisioning":
      return {
        isAvailable: false,
        statusText: "準備中…",
        dotColorClass: "bg-amber-500 animate-pulse",
        badgeVariant: "outline",
      };
    case "draining":
      return {
        isAvailable: false,
        statusText: "停止処理中",
        dotColorClass: "bg-amber-600",
        badgeVariant: "outline",
      };
    case "terminated":
      return {
        isAvailable: false,
        statusText: "停止済み",
        dotColorClass: "bg-muted-foreground",
        badgeVariant: "outline",
      };
    case "error":
      return {
        isAvailable: false,
        statusText: "エラー",
        dotColorClass: "bg-destructive",
        badgeVariant: "destructive",
      };
    default:
      return {
        isAvailable: node.is_online,
        statusText: node.is_online ? "稼働中" : "オフライン",
        dotColorClass: node.is_online ? "bg-emerald-500" : "bg-muted-foreground",
        badgeVariant: "outline",
      };
  }
}
