<script lang="ts">
  import { onMount } from "svelte";
  import { Badge } from "#lib/components/ui/badge/index.ts";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { Card } from "#lib/components/ui/card/index.ts";
  import { formatRelativeTime } from "#lib/format.ts";
  import type { DiagnosticsResponse, IncidentEntry } from "#lib/generated/bindings.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { sync } from "#lib/stores/app.svelte.ts";
  import { toast } from "svelte-sonner";
  import FileDownIcon from "@lucide/svelte/icons/file-down";
  import RefreshCwIcon from "@lucide/svelte/icons/refresh-cw";

  let diagnostics = $state<DiagnosticsResponse | null>(null);
  let loading = $state(false);
  let exporting = $state(false);

  async function load(): Promise<void> {
    if (loading) return;
    loading = true;
    try {
      diagnostics = await sync.connection.client.diagnostics();
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      loading = false;
    }
  }

  async function exportBundle(): Promise<void> {
    if (exporting) return;
    exporting = true;
    try {
      const bundle = await sync.connection.client.diagnosticsBundle();
      const blob = new Blob([JSON.stringify(bundle, null, 2)], { type: "application/json" });
      const url = URL.createObjectURL(blob);
      const anchor = document.createElement("a");
      anchor.href = url;
      anchor.download = `fxg-support-${bundle.generated_at}.json`;
      anchor.click();
      URL.revokeObjectURL(url);
      toast.success(m.settings_support_bundle_downloaded());
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      exporting = false;
    }
  }

  async function resolveIncident(incident: IncidentEntry): Promise<void> {
    try {
      await sync.connection.client.resolveIncident(incident.id);
      await load();
      toast.success(m.settings_incident_resolved());
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
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
      {m.settings_support_bundle_description()}
    </p>
    <div class="flex items-center gap-2">
      <Button variant="outline" size="sm" disabled={loading} onclick={() => void load()}>
        <RefreshCwIcon />
        {m.common_refresh()}
      </Button>
      <Button variant="outline" size="sm" disabled={exporting} onclick={() => void exportBundle()}>
        <FileDownIcon />
        {m.settings_export_support_bundle()}
      </Button>
    </div>
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

    <Card class="gap-2 p-4">
      <h2 class="font-medium">{m.settings_incidents_title()}</h2>
      {#if diagnostics.incidents.length === 0}
        <p class="text-muted-foreground text-sm">{m.settings_incidents_empty()}</p>
      {:else}
        <ul class="flex list-none flex-col gap-2 p-0">
          {#each diagnostics.incidents as incident (incident.id)}
            <li class="flex flex-col gap-1 border-l-2 border-[--danger] pl-3">
              <div class="flex flex-wrap items-center gap-2 text-[13px]">
                <Badge variant={incident.level === "error" ? "destructive" : "secondary"}>
                  {incident.code}
                </Badge>
                <code class="text-muted-foreground text-xs">{incident.source}</code>
                <span class="text-muted-foreground text-xs">
                  {formatRelativeTime(incident.last_seen_at)}
                </span>
                {#if incident.occurrence_count > 1}
                  <span class="text-muted-foreground text-xs">
                    {m.settings_incident_occurrences({ count: incident.occurrence_count })}
                  </span>
                {/if}
              </div>
              <p class="text-sm">{incident.summary}</p>
              {#if incident.resolved_at === null}
                <div>
                  <Button variant="ghost" size="sm" onclick={() => void resolveIncident(incident)}>
                    {m.settings_resolve_incident()}
                  </Button>
                </div>
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
    </Card>
  {/if}
</div>
