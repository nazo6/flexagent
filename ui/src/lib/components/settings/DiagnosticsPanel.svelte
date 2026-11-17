<script lang="ts">
  import { onMount } from "svelte";
  import { errorMessage } from "#lib/api/error-messages.ts";
  import { Badge } from "#lib/components/ui/badge/index.ts";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { Card } from "#lib/components/ui/card/index.ts";
  import type { DiagnosticsResponse } from "#lib/generated/bindings.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { sync } from "#lib/stores/app.svelte.ts";
  import { toast } from "svelte-sonner";
  import RefreshCwIcon from "@lucide/svelte/icons/refresh-cw";

  let diagnostics = $state<DiagnosticsResponse | null>(null);
  let loading = $state(false);

  async function load(): Promise<void> {
    if (loading) return;
    loading = true;
    try {
      diagnostics = await sync.connection.client.diagnostics();
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      loading = false;
    }
  }

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

  onMount(() => {
    void load();
  });
</script>

<div class="flex flex-col gap-3">
  <div class="flex flex-wrap items-center justify-between gap-2">
    <p class="text-muted-foreground text-[13px]">
      {m.settings_health_description()}
    </p>
    <Button variant="outline" size="sm" disabled={loading} onclick={() => void load()}>
      <RefreshCwIcon />
      {m.common_refresh()}
    </Button>
  </div>

  {#if diagnostics === null}
    <p class="text-muted-foreground text-sm">
      {loading ? m.common_loading() : m.common_load_failed()}
    </p>
  {:else}
    <Card class="gap-2 p-4">
      <h2 class="font-medium">{m.settings_health_title()}</h2>
      <ul class="flex list-none flex-col gap-1.5 p-0">
        {#each diagnostics.health.components as component (component.component)}
          <li class="flex flex-wrap items-center gap-2 text-[13px]">
            <Badge variant={stateVariant(component.state)}>{stateLabel(component.state)}</Badge>
            <code class="text-muted-foreground">{component.component}</code>
            {#if component.message}
              <span class="text-muted-foreground text-xs">{component.message}</span>
            {/if}
          </li>
        {/each}
      </ul>
      <div class="text-muted-foreground flex flex-wrap gap-x-4 gap-y-1 text-xs">
        <span>dropped {diagnostics.health.counters.dropped_events}</span>
        <span>lagged {diagnostics.health.counters.lagged_broadcasts}</span>
        <span>replay failures {diagnostics.health.counters.replay_failures}</span>
        <span>outbox retries {diagnostics.health.counters.outbox_retries}</span>
        <span>push failures {diagnostics.health.counters.push_failures}</span>
      </div>
    </Card>
  {/if}
</div>
