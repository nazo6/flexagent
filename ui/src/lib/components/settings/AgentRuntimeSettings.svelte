<script lang="ts">
  import { untrack } from "svelte";
  import { errorMessage } from "#lib/api/error-messages.ts";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { Card } from "#lib/components/ui/card/index.ts";
  import { Input } from "#lib/components/ui/input/index.ts";
  import { Label } from "#lib/components/ui/label/index.ts";
  import type {
    NodeSettingsSnapshot,
    UpdateAgentSettingsRequest,
  } from "#lib/generated/bindings.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { sync } from "#lib/stores/app.svelte.ts";
  import { toast } from "svelte-sonner";
  import GaugeIcon from "@lucide/svelte/icons/gauge";

  /**
   * Node to edit. The settings page passes the selected node when connected to
   * the hub; defaults to the local node.
   */
  let { nodeId = "" }: { nodeId?: string } = $props();

  const targetNode = $derived(nodeId !== "" ? nodeId : (sync.connection.systemInfo?.node_id ?? ""));

  let snapshot = $state<NodeSettingsSnapshot | null>(null);
  let loading = $state(false);
  let busy = $state(false);
  let error = $state<string | null>(null);
  /** Target whose snapshot is currently loaded (null = none). */
  let loadedTarget = $state<string | null>(null);
  let registryUrl = $state("");
  let registryTtl = $state("");
  let warmIdle = $state("");

  const agents = $derived(snapshot?.agents ?? null);
  const ready = $derived(snapshot !== null && loadedTarget === targetNode && !busy);

  $effect(() => {
    // Load once the node identity is known. Do not read `snapshot` here: `load()`
    // assigns it, which would retrigger the effect and overwrite the form while
    // the user types.
    if (targetNode === "") return;
    untrack(() => void load());
  });

  async function load(): Promise<void> {
    if (targetNode === "" || loading) return;
    // Capture the target: it can change while the request is in flight.
    const node = targetNode;
    if (loadedTarget !== node) {
      snapshot = null;
      loadedTarget = null;
      registryUrl = "";
      registryTtl = "";
      warmIdle = "";
    }
    loading = true;
    error = null;
    try {
      const next = await sync.connection.client.nodeSettings(node);
      if (targetNode !== node) return;
      snapshot = next;
      loadedTarget = node;
      syncForm();
    } catch (cause) {
      error = errorMessage(cause);
    } finally {
      loading = false;
      if (targetNode !== node) void load();
    }
  }

  function syncForm(): void {
    const settings = snapshot?.agents;
    if (!settings) return;
    registryUrl = settings.registry_url ?? "";
    registryTtl = settings.registry_cache_ttl_secs?.toString() ?? "";
    warmIdle = settings.warm_idle_secs?.toString() ?? "";
  }

  function parseSeconds(value: string, field: string): number | null {
    const trimmed = value.trim();
    if (trimmed === "") return null;
    const parsed = Number(trimmed);
    if (!/^\d+$/.test(trimmed) || !Number.isSafeInteger(parsed)) {
      throw new Error(m.settings_agent_seconds_invalid({ field }));
    }
    return parsed;
  }

  async function save(): Promise<void> {
    if (!ready || agents === null) {
      toast.error(m.settings_node_not_loaded());
      return;
    }
    let request: UpdateAgentSettingsRequest;
    try {
      request = {
        registry_url: registryUrl.trim() === "" ? null : registryUrl.trim(),
        registry_cache_ttl_secs: parseSeconds(registryTtl, m.settings_agent_registry_ttl()),
        warm_idle_secs: parseSeconds(warmIdle, m.settings_agent_warm_idle()),
        aliases: agents.aliases,
        expected_revision: agents.revision,
      };
    } catch (cause) {
      toast.error(errorMessage(cause));
      return;
    }
    busy = true;
    const node = targetNode;
    try {
      const next = await sync.connection.client.updateAgentSettings(node, request);
      // The target changed while saving: the effect will load the new one.
      if (targetNode !== node) return;
      snapshot = next;
      syncForm();
      toast.success(m.settings_agent_runtime_saved());
    } catch (cause) {
      toast.error(errorMessage(cause));
    } finally {
      busy = false;
    }
  }
</script>

<Card class="gap-3 p-4">
  <div class="flex items-start gap-2">
    <GaugeIcon class="text-muted-foreground mt-0.5 size-4 shrink-0" />
    <div class="flex flex-1 flex-col gap-3">
      <div class="flex flex-col gap-1">
        <h2 class="font-medium">{m.settings_agent_runtime_title()}</h2>
        <p class="text-muted-foreground text-[13px]">
          {m.settings_agent_runtime_description()}
        </p>
      </div>

      {#if error}
        <p class="text-destructive text-[13px]">{error}</p>
      {/if}

      <div class="grid gap-3 sm:grid-cols-2">
        <div class="flex flex-col gap-1 sm:col-span-2">
          <Label for="agent-registry-url">{m.settings_agent_registry_url()}</Label>
          <Input
            id="agent-registry-url"
            bind:value={registryUrl}
            placeholder="https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json"
            autocomplete="off"
            spellcheck="false"
          />
          <p class="text-muted-foreground text-[12px]">
            {m.settings_agent_registry_url_hint()}
          </p>
        </div>

        <div class="flex flex-col gap-1">
          <Label for="agent-registry-ttl">{m.settings_agent_registry_ttl()}</Label>
          <Input
            id="agent-registry-ttl"
            type="text"
            inputmode="numeric"
            pattern="[0-9]*"
            bind:value={registryTtl}
            placeholder="86400"
            autocomplete="off"
          />
          <p class="text-muted-foreground text-[12px]">
            {m.settings_agent_registry_ttl_hint()}
          </p>
        </div>

        <div class="flex flex-col gap-1">
          <Label for="agent-warm-idle">{m.settings_agent_warm_idle()}</Label>
          <Input
            id="agent-warm-idle"
            type="text"
            inputmode="numeric"
            pattern="[0-9]*"
            bind:value={warmIdle}
            placeholder="600"
            autocomplete="off"
          />
          <p class="text-muted-foreground text-[12px]">
            {m.settings_agent_warm_idle_hint()}
          </p>
        </div>
      </div>

      <div class="flex flex-wrap items-center gap-2">
        <Button size="sm" disabled={!ready} onclick={save}>
          {busy ? m.settings_saving() : m.common_save()}
        </Button>
        {#if loading && snapshot === null}
          <span class="text-muted-foreground text-[12px]">{m.common_loading()}</span>
        {/if}
      </div>
    </div>
  </div>
</Card>
