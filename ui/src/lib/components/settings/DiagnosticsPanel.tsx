import { createEffect, createSignal, For, Show } from "solid-js";
import IconRefreshCw from "~icons/lucide/refresh-cw";
import { errorMessage } from "#lib/api/error-messages.ts";
import { Badge } from "#lib/components/ui/badge/index.ts";
import { Button } from "#lib/components/ui/button/index.ts";
import { Card } from "#lib/components/ui/card/index.ts";
import { toast } from "#lib/components/ui/sonner/index.ts";
import type { DiagnosticsResponse } from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";
import { sync } from "#lib/stores/app.ts";

/** Component health and counters of the endpoint or a target node. */
export default function DiagnosticsPanel(props: {
  /** Target node (`""` = the endpoint itself, i.e. the hub's own diagnostics). */
  nodeId?: string;
}) {
  const [diagnostics, setDiagnostics] = createSignal<DiagnosticsResponse | null>(null);
  const [loading, setLoading] = createSignal(false);
  let generation = 0;

  async function load(target = props.nodeId ?? ""): Promise<void> {
    const current = ++generation;
    setLoading(true);
    try {
      const next =
        target === ""
          ? await sync.connection.client.diagnostics()
          : await sync.connection.client.nodeDiagnostics(target);
      if (current !== generation) return;
      setDiagnostics(next);
    } catch (error) {
      if (current === generation) toast.error(errorMessage(error));
    } finally {
      if (current === generation) setLoading(false);
    }
  }

  createEffect(
    () => props.nodeId ?? "",
    (target) => {
      setDiagnostics(null);
      void load(target);
    },
  );

  function stateLabel(state: string): string {
    switch (state) {
      case "ok":
        return m.settings_health_ok();
      case "degraded":
        return m.settings_health_degraded();
      case "error":
        return m.settings_health_error();
      default:
        return m.settings_health_unknown();
    }
  }

  function stateVariant(state: string): "outline" | "secondary" | "destructive" {
    switch (state) {
      case "error":
        return "destructive";
      case "degraded":
        return "secondary";
      default:
        return "outline";
    }
  }

  return (
    <div class="flex flex-col gap-3">
      <div class="flex flex-wrap items-center justify-between gap-2">
        <p class="text-muted-foreground text-[13px]">{m.settings_health_description()}</p>
        <Button variant="outline" size="sm" disabled={loading()} onClick={() => void load()}>
          <IconRefreshCw />
          {m.common_refresh()}
        </Button>
      </div>

      <Show
        when={diagnostics()}
        fallback={
          <p class="text-muted-foreground text-sm">
            {loading() ? m.common_loading() : m.common_load_failed()}
          </p>
        }
      >
        {(current) => (
          <Card class="gap-2 p-4">
            <h2 class="font-medium">{m.settings_health_title()}</h2>
            <ul class="flex list-none flex-col gap-1.5 p-0">
              <For each={current().health.components}>
                {(component) => (
                  <li class="flex flex-wrap items-center gap-2 text-[13px]">
                    <Badge variant={stateVariant(component.state)}>
                      {stateLabel(component.state)}
                    </Badge>
                    <code class="text-muted-foreground">{component.component}</code>
                    <Show when={component.message}>
                      <span class="text-muted-foreground text-xs">{component.message}</span>
                    </Show>
                  </li>
                )}
              </For>
            </ul>
            <div class="text-muted-foreground flex flex-wrap gap-x-4 gap-y-1 text-xs">
              <span>dropped {current().health.counters.dropped_events}</span>
              <span>lagged {current().health.counters.lagged_broadcasts}</span>
              <span>replay failures {current().health.counters.replay_failures}</span>
              <span>outbox retries {current().health.counters.outbox_retries}</span>
              <span>push failures {current().health.counters.push_failures}</span>
            </div>
          </Card>
        )}
      </Show>
    </div>
  );
}
