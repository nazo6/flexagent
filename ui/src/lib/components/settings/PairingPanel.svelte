<script lang="ts">
  import { untrack } from "svelte";
  import { errorMessage } from "#lib/api/error-messages.ts";
  import { hubLinkLabel } from "#lib/node-status.ts";
  import { Badge } from "#lib/components/ui/badge/index.ts";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { Card } from "#lib/components/ui/card/index.ts";
  import { Input } from "#lib/components/ui/input/index.ts";
  import { Label } from "#lib/components/ui/label/index.ts";
  import type { HubLinkState, NodeSettingsSnapshot } from "#lib/generated/bindings.ts";
  import { sync } from "#lib/stores/app.svelte.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { toast } from "svelte-sonner";
  import CopyIcon from "@lucide/svelte/icons/copy";
  import LinkIcon from "@lucide/svelte/icons/link";
  import PlugZapIcon from "@lucide/svelte/icons/plug-zap";
  import UnplugIcon from "@lucide/svelte/icons/unplug";

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
  let hubUrl = $state("");
  let token = $state("");
  let tested = $state<HubLinkState | null>(null);

  const pairing = $derived(snapshot?.pairing ?? null);
  const connection = $derived(sync.hubLink ?? pairing?.connection ?? null);
  const linkState = $derived(connection?.state ?? "standalone");
  const switchesHub = $derived(
    hubUrl.trim() !== "" && pairing?.hub_url !== null && pairing?.hub_url !== hubUrl.trim(),
  );

  $effect(() => {
    // Load once the node identity is known. Do not read `snapshot` here: `load()`
    // assigns it, which would retrigger the effect and overwrite the form while
    // the user types. Save/clear already refresh the snapshot from the response.
    if (targetNode === "") return;
    untrack(() => void load());
  });

  async function load(): Promise<void> {
    if (targetNode === "" || loading) return;
    // Capture the target: it can change while the request is in flight.
    const node = targetNode;
    loading = true;
    error = null;
    try {
      snapshot = await sync.connection.client.nodeSettings(node);
      // Do not overwrite what the user is typing if the target moved on.
      if (targetNode !== node) return;
      hubUrl = snapshot.pairing.hub_url ?? "";
      token = "";
    } catch (cause) {
      error = errorMessage(cause);
    } finally {
      loading = false;
      if (targetNode !== node) void load();
    }
  }

  function linkStateLabel(value: HubLinkState): string {
    return hubLinkLabel(value);
  }

  function linkVariant(value: HubLinkState): "secondary" | "outline" | "destructive" {
    if (value === "connected") return "secondary";
    if (value === "standalone") return "outline";
    return "destructive";
  }

  async function test(): Promise<void> {
    if (busy || hubUrl.trim() === "") return;
    busy = true;
    tested = null;
    try {
      const status = await sync.connection.client.testPairing(targetNode, {
        hub_url: hubUrl.trim(),
        token: token.trim() === "" ? null : token.trim(),
      });
      tested = status.state;
      if (status.state === "connected") {
        toast.success(m.settings_pairing_test_ok());
      } else {
        toast.error(status.message ?? linkStateLabel(status.state));
      }
    } catch (cause) {
      toast.error(errorMessage(cause));
    } finally {
      busy = false;
    }
  }

  async function save(): Promise<void> {
    if (busy || hubUrl.trim() === "" || snapshot === null) return;
    busy = true;
    try {
      snapshot = await sync.connection.client.updatePairing(targetNode, {
        hub_url: hubUrl.trim(),
        token: token.trim() === "" ? null : token.trim(),
        expected_revision: snapshot.pairing.revision,
      });
      token = "";
      tested = null;
      toast.success(m.settings_pairing_saved({ url: snapshot.pairing.hub_url ?? "" }));
    } catch (cause) {
      toast.error(errorMessage(cause));
    } finally {
      busy = false;
    }
  }

  async function clear(): Promise<void> {
    if (busy || snapshot === null) return;
    busy = true;
    try {
      snapshot = await sync.connection.client.clearPairing(targetNode, {
        expected_revision: snapshot.pairing.revision,
      });
      hubUrl = "";
      token = "";
      tested = null;
      toast.success(m.settings_pairing_cleared());
    } catch (cause) {
      toast.error(errorMessage(cause));
    } finally {
      busy = false;
    }
  }

  async function copyNodeId(): Promise<void> {
    try {
      await navigator.clipboard.writeText(targetNode);
      toast.success(m.settings_copied({ name: m.settings_pairing_node_id() }));
    } catch {
      toast.error(m.settings_copy_failed());
    }
  }
</script>

<Card class="gap-3 p-4">
  <div class="flex items-start gap-2">
    <PlugZapIcon class="text-muted-foreground mt-0.5 size-4 shrink-0" />
    <div class="flex flex-1 flex-col gap-3">
      <div class="flex flex-col gap-1">
        <h2 class="font-medium">{m.settings_pairing_title()}</h2>
        <p class="text-muted-foreground text-[13px]">
          {m.settings_pairing_description()}
        </p>
      </div>

      <div class="flex flex-wrap items-center gap-2 text-[13px]">
        <span class="text-muted-foreground">{m.settings_pairing_state()}</span>
        <Badge variant={linkVariant(linkState)} title={connection?.message ?? undefined}>
          {linkStateLabel(linkState)}
        </Badge>
        {#if connection?.hub_url}
          <code class="text-muted-foreground text-[11px]">{connection.hub_url}</code>
        {/if}
      </div>

      <div class="flex flex-wrap items-center gap-2 text-[13px]">
        <span class="text-muted-foreground">{m.settings_pairing_node_id()}</span>
        <code class="text-[12px]">{targetNode}</code>
        <Button variant="ghost" size="sm" onclick={copyNodeId}>
          <CopyIcon />
          {m.settings_pairing_copy_node_id()}
        </Button>
      </div>

      <div class="grid gap-2 sm:grid-cols-2">
        <div class="flex flex-col gap-1">
          <Label for="pairing-hub-url">{m.settings_pairing_hub_url()}</Label>
          <Input
            id="pairing-hub-url"
            bind:value={hubUrl}
            placeholder="http://hub.example:8080"
            autocomplete="off"
            spellcheck="false"
          />
        </div>
        <div class="flex flex-col gap-1">
          <Label for="pairing-token">{m.settings_pairing_token()}</Label>
          <Input
            id="pairing-token"
            type="password"
            bind:value={token}
            placeholder={pairing?.token_configured
              ? m.settings_pairing_token_configured()
              : m.settings_pairing_token_placeholder()}
            autocomplete="off"
            spellcheck="false"
          />
        </div>
      </div>

      <p class="text-muted-foreground text-[12px]">
        {m.settings_pairing_token_hint()}
      </p>

      {#if switchesHub}
        <p class="text-[12px] text-amber-600 dark:text-amber-400">
          {m.settings_pairing_switch_warning()}
        </p>
      {/if}

      {#if pairing?.sessions_truncated}
        <p class="text-[12px] text-amber-600 dark:text-amber-400">
          {m.settings_pairing_truncated()}
        </p>
      {/if}

      {#if error}
        <p class="text-destructive text-[13px]">{error}</p>
      {/if}

      <div class="flex flex-wrap items-center gap-2">
        <Button variant="outline" size="sm" disabled={busy || hubUrl.trim() === ""} onclick={test}>
          <LinkIcon />
          {m.settings_pairing_test()}
        </Button>
        <Button size="sm" disabled={busy || hubUrl.trim() === ""} onclick={save}>
          {m.settings_pairing_save()}
        </Button>
        <Button
          variant="outline"
          size="sm"
          disabled={busy || pairing?.hub_url === null}
          onclick={clear}
        >
          <UnplugIcon />
          {m.settings_pairing_clear()}
        </Button>
        {#if tested === "connected"}
          <span class="text-muted-foreground text-[12px]">{m.settings_pairing_test_ok()}</span>
        {/if}
      </div>
    </div>
  </div>
</Card>
