<script lang="ts">
  import AuditLog from "#lib/components/settings/AuditLog.svelte";
  import ClientTokenCard from "#lib/components/settings/ClientTokenCard.svelte";
  import NodeTokensPanel from "#lib/components/settings/NodeTokensPanel.svelte";
  import ProcessSettingsPanel from "#lib/components/settings/ProcessSettingsPanel.svelte";
  import ProvisionerManager from "#lib/components/settings/ProvisionerManager.svelte";
  import SettingsCatalog from "#lib/components/settings/SettingsCatalog.svelte";
  import type { PageData } from "./$types";

  let { data }: { data: PageData } = $props();
</script>

{#if data.tab === "general"}
  <SettingsCatalog section="hub" />
  <ClientTokenCard />
{:else if data.tab === "process"}
  <ProcessSettingsPanel />
{:else if data.tab === "tokens"}
  <NodeTokensPanel />
{:else if data.tab === "provisioners"}
  <ProvisionerManager />
{:else if data.tab === "audit"}
  <AuditLog />
{/if}
