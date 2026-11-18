import { createMemo, Show } from "solid-js";
import { hubLinkLabel } from "#lib/node-status.ts";
import { m } from "#lib/paraglide/messages.js";
import { connection, sync } from "#lib/stores/app.ts";
import { cn } from "#lib/utils.ts";
import IconCpu from "~icons/lucide/cpu";
import IconServer from "~icons/lucide/server";

/**
 * Identity badge for the connection target (Hub vs. local Node), ported from
 * `ui/src/lib/components/ConnectionScopeBadge.svelte`.
 *
 * `compact` is the single-line chip used in tight headers (mobile); the default
 * is the two-line chip with the link health under the scope label.
 */

export interface ConnectionScopeBadgeProps {
  /** Small single-line chip for tight headers (mobile). */
  compact?: boolean;
  class?: string;
}

export default function ConnectionScopeBadge(props: ConnectionScopeBadgeProps) {
  const isHub = createMemo(() => connection.role === "hub");
  const isLocalNode = createMemo(() => connection.role === "local_node");
  const label = createMemo(() => (isHub() ? m.shell_scope_hub() : m.shell_scope_node()));
  const nodeId = createMemo(() => connection.systemInfo?.node_id ?? "");
  const nodeName = createMemo(
    () => sync.nodes.find((node) => node.node_id === nodeId())?.name ?? nodeId(),
  );

  /** Host shown next to the scope label (`hub.example:8080`). */
  const host = createMemo(() => {
    const origin = connection.origin;
    try {
      return origin === "" ? location.host : new URL(origin).host;
    } catch {
      return origin;
    }
  });

  const detail = createMemo(() => (isHub() ? host() : nodeName() || host()));
  const title = createMemo(() =>
    isHub()
      ? m.shell_scope_hub_tooltip({ target: host() })
      : m.shell_scope_node_tooltip({ target: detail() }),
  );

  const hubLink = createMemo(() => sync.hubLink ?? connection.systemInfo?.hub_link ?? null);
  const hubState = createMemo(() => hubLink()?.state ?? "standalone");

  /** Link health of this client and, on a node, of the node's hub link. */
  const healthText = createMemo(() => {
    const parts = [sync.wsConnected ? m.shell_connected() : m.shell_disconnected()];
    if (isLocalNode()) {
      parts.push(
        hubState() === "standalone"
          ? hubLinkLabel("standalone")
          : m.shell_hub_status({ status: hubLinkLabel(hubState()) }),
      );
    }
    if (hubState() !== "standalone" && sync.unsyncedEventCount > 0) {
      parts.push(m.shell_unsynced_count({ count: sync.unsyncedEventCount }));
    }
    return parts.join(" · ");
  });

  const healthDotClass = createMemo(() =>
    !sync.wsConnected
      ? "bg-danger"
      : isLocalNode() && hubState() !== "standalone" && hubState() !== "connected"
        ? "bg-warn"
        : "bg-ok",
  );

  return (
    <Show
      when={!props.compact}
      fallback={
        <span
          class={cn(
            "inline-flex shrink-0 items-center gap-1 rounded-md border px-1.5 py-0.5 text-[10px] font-semibold tracking-wider uppercase",
            isHub()
              ? "border-signal/50 bg-signal-soft text-foreground"
              : "border-border bg-muted/60",
            props.class,
          )}
          title={title()}
        >
          <Show when={isHub()} fallback={<IconCpu class="size-3 shrink-0" />}>
            <IconServer class="size-3 shrink-0" />
          </Show>
          {label()}
        </span>
      }
    >
      <span
        class={cn(
          "flex min-w-0 items-center gap-2 rounded-md border px-2 py-1.5",
          isHub() ? "border-signal/50 bg-signal-soft/60" : "border-border bg-muted/40",
          props.class,
        )}
        title={title()}
      >
        <Show when={isHub()} fallback={<IconCpu class="text-muted-foreground size-3.5 shrink-0" />}>
          <IconServer class="text-muted-foreground size-3.5 shrink-0" />
        </Show>
        <span class="flex min-w-0 flex-col gap-0.5 leading-tight">
          <span class="flex min-w-0 items-baseline gap-1.5">
            <span class="shrink-0 text-[10px] font-semibold tracking-wider uppercase">
              {label()}
            </span>
            <span class="text-muted-foreground truncate text-[11px]">{detail()}</span>
          </span>
          <span class="text-muted-foreground flex min-w-0 items-center gap-1.5 text-[11px]">
            <span class={cn("size-1.5 shrink-0 rounded-full", healthDotClass())} />
            <span class="truncate">{healthText()}</span>
          </span>
        </span>
      </span>
    </Show>
  );
}
