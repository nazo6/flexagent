import type { NodeSummary } from "#lib/generated/bindings.ts";
import { getNodeAvailability } from "#lib/node-status.ts";
import { sync } from "#lib/stores/app.ts";
import { cn } from "#lib/utils.ts";
import { Show } from "solid-js";

export interface NodeStatusBadgeProps {
  node?: NodeSummary | null;
  showLabel?: boolean;
  showNodeName?: boolean;
  class?: string;
}

/** Availability dot + label for a node. */
export default function NodeStatusBadge(props: NodeStatusBadgeProps) {
  const node = () => props.node ?? null;
  const availability = () => getNodeAvailability(node(), sync.wsConnected);

  return (
    <div
      class={cn("inline-flex items-center gap-1.5 text-[11px]", props.class ?? "")}
      title={
        node()
          ? `${node()!.name} (${node()!.os}/${node()!.arch}): ${availability().statusText}`
          : availability().statusText
      }
    >
      <span class={cn("size-2 shrink-0 rounded-full", availability().dotColorClass)} />
      <Show when={props.showNodeName && node()}>
        <span class="text-foreground font-medium">{node()!.name}</span>
      </Show>
      <Show when={props.showLabel ?? true}>
        <span class="text-muted-foreground">{availability().statusText}</span>
      </Show>
    </div>
  );
}
