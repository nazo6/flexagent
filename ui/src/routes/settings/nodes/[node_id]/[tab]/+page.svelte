<script lang="ts">
  import AgentManager from "#lib/components/settings/AgentManager.svelte";
  import AuditLog from "#lib/components/settings/AuditLog.svelte";
  import ClientTokenCard from "#lib/components/settings/ClientTokenCard.svelte";
  import PairingPanel from "#lib/components/settings/PairingPanel.svelte";
  import ProcessSettingsPanel from "#lib/components/settings/ProcessSettingsPanel.svelte";
  import SettingsCatalog from "#lib/components/settings/SettingsCatalog.svelte";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { Card } from "#lib/components/ui/card/index.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { isNodeTabAvailable, nodeSettingsPath } from "#lib/settings-routes.ts";
  import { sync } from "#lib/stores/app.svelte.ts";
  import type { PageData } from "./$types";

  let { data }: { data: PageData } = $props();

  const isHub = $derived(sync.connection.role === "hub");
  const tabAvailable = $derived(isNodeTabAvailable(data.tab, isHub));
</script>

{#if !tabAvailable}
  <Card class="gap-3 p-5">
    <p class="text-muted-foreground text-[13px]">
      {m.settings_node_tab_local_only()}
    </p>
    <div>
      <Button href={nodeSettingsPath("general", data.nodeId)} variant="outline" size="sm">
        {m.settings_tab_general()}
      </Button>
    </div>
  </Card>
{:else if data.tab === "general"}
  <SettingsCatalog section="machine" nodeId={data.nodeId} />
  {#if !isHub}
    <ClientTokenCard />
  {/if}
{:else if data.tab === "process"}
  <ProcessSettingsPanel />
{:else if data.tab === "pairing"}
  <PairingPanel nodeId={data.nodeId} />
{:else if data.tab === "agents"}
  <AgentManager nodeId={data.nodeId} />
{:else if data.tab === "audit"}
  <AuditLog />
{/if}
