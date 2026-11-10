<script lang="ts">
  import { onMount } from "svelte";
  import * as AlertDialog from "#lib/components/ui/alert-dialog/index.ts";
  import { Badge } from "#lib/components/ui/badge/index.ts";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { Card } from "#lib/components/ui/card/index.ts";
  import * as Dialog from "#lib/components/ui/dialog/index.ts";
  import { Input } from "#lib/components/ui/input/index.ts";
  import { Label } from "#lib/components/ui/label/index.ts";
  import { agentSourceLabel, agentsAvailableOn, isPackageEnabled } from "#lib/agents.ts";
  import type { AgentSummary } from "#lib/generated/bindings.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { sync } from "#lib/stores/app.svelte.ts";
  import { toast } from "svelte-sonner";
  import BotIcon from "@lucide/svelte/icons/bot";
  import PencilIcon from "@lucide/svelte/icons/pencil";
  import PlusIcon from "@lucide/svelte/icons/plus";
  import PackageIcon from "@lucide/svelte/icons/package";
  import PowerOffIcon from "@lucide/svelte/icons/power-off";
  import RefreshCwIcon from "@lucide/svelte/icons/refresh-cw";
  import SearchIcon from "@lucide/svelte/icons/search";
  import Trash2Icon from "@lucide/svelte/icons/trash-2";

  let agents = $state<AgentSummary[]>([]);
  let loading = $state(false);
  /** Target node (catalog source and operation target). */
  let nodeId = $state("");
  /** Label of the running operation (for example `opencode2 installing…`). null = idle. */
  let busyLabel = $state<string | null>(null);
  /** Pending removal target. */
  let removeTarget = $state<AgentSummary | null>(null);

  // Catalog dialog (registry agents not yet installed on this node)
  let catalogOpen = $state(false);
  let catalogQuery = $state("");

  // Custom agent definition form (null = create new)
  let customOpen = $state(false);
  let customBusy = $state(false);
  let customEditId = $state<string | null>(null);
  let customForm = $state({ id: "", name: "", command: "", args: "", env: "" });

  const onlineNodes = $derived(sync.nodes.filter((node) => node.is_online));
  const busy = $derived(busyLabel !== null);

  /** Agents available on this node (npx/uvx are included only when enabled). */
  const available = $derived(agentsAvailableOn(agents, nodeId));
  /** Registry agents not yet available on this node. */
  const catalogOnly = $derived(
    agents
      .filter((agent) => agent.registry && !isAvailableOn(agent))
      .filter((agent) => matchesQuery(agent, catalogQuery)),
  );

  onMount(() => {
    void loadAgents();
  });

  // Default online node selection (set on update because the node list is empty right after
  // connecting)
  $effect(() => {
    if (nodeId === "" && onlineNodes.length > 0) {
      nodeId = onlineNodes[0].node_id;
    }
  });

  // Reload the catalog when the target node changes (the server no longer
  // returns a representative node's catalog).
  $effect(() => {
    if (nodeId !== "") void loadAgents();
  });

  function isAvailableOn(agent: AgentSummary): boolean {
    return agent.installed_nodes.includes(nodeId);
  }

  function matchesQuery(agent: AgentSummary, query: string): boolean {
    const needle = query.trim().toLowerCase();
    if (needle === "") return true;
    return [agent.id, agent.name, agent.description ?? ""].some((value) =>
      value.toLowerCase().includes(needle),
    );
  }

  function nodeName(id: string): string {
    return sync.nodes.find((node) => node.node_id === id)?.name ?? id;
  }

  async function loadAgents(): Promise<void> {
    if (loading || nodeId === "") return;
    loading = true;
    try {
      agents = await sync.connection.client.nodeAgents(nodeId);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      loading = false;
    }
  }

  async function setPackageEnabled(agent: AgentSummary, enabled: boolean): Promise<void> {
    if (busy || nodeId === "") return;
    busyLabel = enabled
      ? m.settings_agent_package_enabling({ name: agent.id })
      : m.settings_agent_package_disabling({ name: agent.id });
    try {
      const result = enabled
        ? await sync.connection.client.enablePackageAgent(nodeId, agent.id)
        : await sync.connection.client.disablePackageAgent(nodeId, agent.id);
      toast.success(
        result.message ??
          (enabled
            ? m.settings_agent_package_enabled({ name: agent.id })
            : m.settings_agent_package_disabled({ name: agent.id })),
      );
      await Promise.all([loadAgents(), sync.refreshNodes()]);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      busyLabel = null;
    }
  }

  async function refreshCatalog(): Promise<void> {
    if (busy || nodeId === "") return;
    busyLabel = m.settings_catalog_refreshing();
    try {
      const result = await sync.connection.client.refreshAgentsCatalog(nodeId);
      toast.success(result.message ?? m.settings_catalog_refreshed());
      await Promise.all([loadAgents(), sync.refreshNodes()]);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      busyLabel = null;
    }
  }

  async function confirmRemove(): Promise<void> {
    const target = removeTarget;
    if (target === null || busy || nodeId === "") return;
    const isCustom = target.custom_config !== null;
    busyLabel = m.settings_agent_removing({ name: target.id });
    try {
      const result = isCustom
        ? await sync.connection.client.removeCustomAgent(nodeId, target.id)
        : await sync.connection.client.disablePackageAgent(nodeId, target.id);
      toast.success(result.message ?? m.settings_agent_removed({ name: target.id }));
      removeTarget = null;
      await Promise.all([loadAgents(), sync.refreshNodes()]);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      busyLabel = null;
    }
  }

  /** Open the custom definition form (creates a new agent when `agent` is omitted). */
  function openCustom(agent: AgentSummary | null): void {
    const config = agent?.custom_config ?? null;
    customEditId = agent?.id ?? null;
    customForm = {
      id: agent?.id ?? "",
      name: config?.name ?? "",
      command: config?.command ?? "",
      args: (config?.args ?? []).join(" "),
      env: formatEnv(config?.env ?? {}, config?.env_secrets ?? {}),
    };
    customOpen = true;
  }

  function formatEnv(env: Record<string, string>, envSecrets: Record<string, string>): string {
    return [
      ...Object.entries(env).map(([key, value]) => `${key}=${value}`),
      ...Object.entries(envSecrets).map(([key, value]) => `${key}=@${value}`),
    ].join("\n");
  }

  function parseEnv(text: string): {
    env: Record<string, string>;
    env_secrets: Record<string, string>;
  } {
    const env: Record<string, string> = {};
    const env_secrets: Record<string, string> = {};
    for (const line of text.split("\n")) {
      const trimmed = line.trim();
      if (trimmed === "" || trimmed.startsWith("#")) continue;
      const index = trimmed.indexOf("=");
      if (index <= 0) continue;
      const key = trimmed.slice(0, index).trim();
      const value = trimmed.slice(index + 1).trim();
      if (value.startsWith("@")) env_secrets[key] = value.slice(1).trim();
      else env[key] = value;
    }
    return { env, env_secrets };
  }

  async function submitCustom(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (customBusy || nodeId === "") return;
    const id = (customEditId ?? customForm.id).trim();
    if (id === "") {
      toast.error(m.settings_agent_id_required());
      return;
    }
    if (customForm.command.trim() === "") {
      toast.error(m.settings_agent_command_required());
      return;
    }
    customBusy = true;
    try {
      const result = await sync.connection.client.upsertCustomAgent(nodeId, id, {
        name: customForm.name.trim() === "" ? null : customForm.name.trim(),
        command: customForm.command.trim(),
        args: customForm.args.split(/\s+/).filter((value) => value !== ""),
        ...parseEnv(customForm.env),
      });
      toast.success(result.message ?? m.settings_custom_agent_saved({ name: id }));
      customOpen = false;
      await Promise.all([loadAgents(), sync.refreshNodes()]);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      customBusy = false;
    }
  }
</script>

<div class="flex flex-col gap-3">
  <div class="flex flex-wrap items-center justify-between gap-2">
    <p class="text-muted-foreground text-sm">
      {m.settings_agents_description()}
    </p>
    <div class="flex flex-wrap items-center gap-1.5">
      <select
        bind:value={nodeId}
        aria-label={m.settings_target_node()}
        class="border-input bg-background h-8 rounded-md border px-2 text-[13px]"
        title={m.settings_target_node()}
      >
        {#each sync.nodes as node (node.node_id)}
          <option value={node.node_id} disabled={!node.is_online}>
            {node.name}{node.is_online ? "" : m.settings_node_offline_suffix()}
          </option>
        {/each}
      </select>
      <Button
        variant="outline"
        size="sm"
        disabled={busy || nodeId === ""}
        onclick={() => void refreshCatalog()}
        title={m.settings_catalog_refresh_title()}
      >
        <RefreshCwIcon />
        {m.settings_catalog_refresh()}
      </Button>
      <Button variant="outline" size="sm" disabled={loading} onclick={() => void loadAgents()}>
        <RefreshCwIcon />
        {m.common_refresh()}
      </Button>
      <Button size="sm" disabled={nodeId === ""} onclick={() => openCustom(null)}>
        <PlusIcon />
        {m.settings_add_custom()}
      </Button>
    </div>
  </div>

  {#if busyLabel !== null}
    <p class="text-muted-foreground text-[13px]">{busyLabel}</p>
  {/if}

  {#if onlineNodes.length === 0 && sync.nodes.length > 0}
    <p class="text-muted-foreground text-sm">
      {m.settings_no_online_nodes()}
    </p>
  {:else if nodeId === ""}
    <p class="text-muted-foreground text-sm">{m.settings_select_node()}</p>
  {:else if loading && agents.length === 0}
    <p class="text-muted-foreground text-sm">{m.settings_registry_loading()}</p>
  {:else if available.length === 0}
    <p class="text-muted-foreground text-sm">
      {m.settings_no_agents()}
    </p>
  {:else}
    {#each available as agent (agent.id)}
      <Card class="gap-2 p-4">
        <div class="flex flex-wrap items-start justify-between gap-2">
          <div class="flex min-w-0 flex-col gap-0.5">
            <div class="flex flex-wrap items-center gap-2">
              <BotIcon class="text-muted-foreground size-4" />
              <h2 class="font-medium">{agent.name}</h2>
              <span class="text-muted-foreground font-mono text-[13px]">{agent.id}</span>
              {#if agent.version !== "-"}
                <span class="text-muted-foreground text-[13px]">v{agent.version}</span>
              {/if}
            </div>
            {#if agent.description}
              <p class="text-muted-foreground text-[13px]">{agent.description}</p>
            {/if}
            <div class="mt-0.5 flex flex-wrap items-center gap-1.5">
              {#each agent.sources as source (source)}
                <Badge variant="outline" class="text-[11px]">{agentSourceLabel(source)}</Badge>
              {/each}
            </div>
            {#if agent.external_path}
              <p class="text-muted-foreground font-mono text-[11px] break-all">
                {agent.external_path}
              </p>
            {/if}
          </div>

          <div class="flex flex-wrap items-center gap-1.5">
            {#if isPackageEnabled(agent)}
              <Button
                variant="ghost"
                size="sm"
                class="text-muted-foreground"
                disabled={busy || nodeId === ""}
                onclick={() => void setPackageEnabled(agent, false)}
                title={m.settings_disable_package_title()}
              >
                <PowerOffIcon />
                {m.settings_disable_package()}
              </Button>
            {/if}
            {#if agent.custom_config !== null}
              <Button
                variant="ghost"
                size="icon-sm"
                disabled={busy || nodeId === ""}
                onclick={() => openCustom(agent)}
                title={m.settings_edit_custom_title()}
              >
                <PencilIcon />
                <span class="sr-only">{m.settings_edit_agent({ name: agent.id })}</span>
              </Button>
            {/if}
            {#if agent.custom_config !== null}
              <Button
                variant="ghost"
                size="icon-sm"
                class="text-muted-foreground hover:text-destructive"
                disabled={busy || nodeId === ""}
                onclick={() => (removeTarget = agent)}
              >
                <Trash2Icon />
                <span class="sr-only">{m.settings_remove_agent({ name: agent.id })}</span>
              </Button>
            {/if}
          </div>
        </div>

        {#if agent.installed_nodes.length > 1}
          <p class="text-muted-foreground text-[11px]">
            {m.settings_available_nodes()}
            {agent.installed_nodes.map((id) => nodeName(id)).join(", ")}
          </p>
        {/if}
      </Card>
    {/each}
  {/if}

  <div class="mt-1">
    <Button
      variant="outline"
      size="sm"
      disabled={nodeId === ""}
      onclick={() => (catalogOpen = true)}
    >
      <PlusIcon />
      {m.settings_add_from_catalog({ count: catalogOnly.length })}
    </Button>
  </div>
</div>

<!-- Agent removal confirmation -->
<AlertDialog.Root
  open={removeTarget !== null}
  onOpenChange={(open) => !open && (removeTarget = null)}
>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>{m.settings_remove_agent_title()}</AlertDialog.Title>
      <AlertDialog.Description class="text-sm">
        {#if removeTarget?.custom_config !== null}
          {m.settings_remove_custom_description()}
          <code>{removeTarget?.id}</code>
          {m.settings_remove_custom_description_suffix()}
          <code>config.toml</code>
          {m.settings_remove_custom_description_tail()}
        {:else}
          {m.settings_remove_package_description()}
          <code>{removeTarget?.id}</code>
          {m.settings_remove_package_description_tail({ name: nodeName(nodeId) })}
        {/if}
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel disabled={busy}>{m.common_cancel()}</AlertDialog.Cancel>
      <AlertDialog.Action
        variant="destructive"
        disabled={busy}
        onclick={() => void confirmRemove()}
      >
        {m.common_delete()}
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>

<!-- Catalog dialog (not installed) -->
<Dialog.Root open={catalogOpen} onOpenChange={(open) => (catalogOpen = open)}>
  <Dialog.Content class="sm:max-w-2xl">
    <Dialog.Header>
      <Dialog.Title>{m.settings_catalog_title()}</Dialog.Title>
      <Dialog.Description>
        {m.settings_catalog_description({ name: nodeName(nodeId) })}
      </Dialog.Description>
    </Dialog.Header>
    <div class="flex flex-col gap-3">
      <div class="relative">
        <SearchIcon class="text-muted-foreground absolute top-2 left-2 size-4" />
        <Input
          bind:value={catalogQuery}
          placeholder={m.settings_catalog_search_placeholder()}
          class="h-8 pl-8 text-[13px]"
        />
      </div>
      <div class="flex max-h-[60vh] flex-col gap-2 overflow-y-auto">
        {#if catalogOnly.length === 0}
          <p class="text-muted-foreground text-sm">
            {catalogQuery.trim() === ""
              ? m.settings_catalog_empty()
              : m.settings_catalog_no_match()}
          </p>
        {:else}
          {#each catalogOnly as agent (agent.id)}
            <div class="flex flex-wrap items-start justify-between gap-2 rounded-md border p-2.5">
              <div class="flex min-w-0 flex-col gap-0.5">
                <div class="flex flex-wrap items-center gap-2">
                  <span class="text-sm font-medium">{agent.name}</span>
                  <span class="text-muted-foreground font-mono text-[13px]">{agent.id}</span>
                  {#if agent.version !== "-"}
                    <span class="text-muted-foreground text-[13px]">v{agent.version}</span>
                  {/if}
                  <Badge variant="secondary" class="text-[11px]">{m.settings_not_installed()}</Badge
                  >
                </div>
                {#if agent.description}
                  <p class="text-muted-foreground text-[13px]">
                    {agent.description}
                  </p>
                {/if}
              </div>
              <div class="flex flex-wrap items-center gap-1.5">
                {#if agent.package_available}
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={busy || nodeId === ""}
                    onclick={() => void setPackageEnabled(agent, true)}
                    title={m.settings_enable_package_title()}
                  >
                    <PackageIcon />
                    {m.settings_enable_package()}
                  </Button>
                {:else}
                  <span class="text-muted-foreground text-[11px]">
                    {m.settings_manual_install_required()}
                  </span>
                {/if}
              </div>
            </div>
          {/each}
        {/if}
      </div>
    </div>
  </Dialog.Content>
</Dialog.Root>

<!-- Custom agent definition dialog -->
<Dialog.Root open={customOpen} onOpenChange={(open) => (customOpen = open)}>
  <Dialog.Content class="sm:max-w-lg">
    <Dialog.Header>
      <Dialog.Title>
        {customEditId === null
          ? m.settings_add_custom_agent()
          : m.settings_edit_agent({ name: customEditId })}
      </Dialog.Title>
      <Dialog.Description>
        {m.settings_custom_description()}
        <code>~/.flexagent/custom_agents.json</code>
      </Dialog.Description>
    </Dialog.Header>
    <form class="flex flex-col gap-3" onsubmit={submitCustom}>
      <div class="grid gap-1.5">
        <Label for="custom-id">{m.settings_agent_id()}</Label>
        <Input
          id="custom-id"
          bind:value={customForm.id}
          disabled={customEditId !== null}
          placeholder="my-agent"
          class="font-mono text-[13px]"
        />
      </div>
      <div class="grid gap-1.5">
        <Label for="custom-name">{m.settings_display_name()}</Label>
        <Input
          id="custom-name"
          bind:value={customForm.name}
          placeholder="My Agent"
          class="text-[13px]"
        />
      </div>
      <div class="grid gap-1.5">
        <Label for="custom-command">{m.settings_command()}</Label>
        <Input
          id="custom-command"
          bind:value={customForm.command}
          placeholder="npx / node / /path/to/agent"
          class="font-mono text-[13px]"
        />
        <p class="text-muted-foreground text-[11px]">
          {m.settings_command_hint()}
        </p>
      </div>
      <div class="grid gap-1.5">
        <Label for="custom-args">{m.settings_args()}</Label>
        <Input
          id="custom-args"
          bind:value={customForm.args}
          placeholder="-y @scope/agent-acp"
          class="font-mono text-[13px]"
        />
      </div>
      <div class="grid gap-1.5">
        <Label for="custom-env">
          {m.settings_env_vars()} (<code>KEY=VALUE</code>, secret reference
          <code>KEY=@SECRET_KEY</code>)
        </Label>
        <textarea
          id="custom-env"
          bind:value={customForm.env}
          rows="3"
          class="border-input bg-background focus-visible:ring-ring/40 rounded-md border px-2 py-1.5 font-mono text-[13px] outline-none focus-visible:ring-1"
          placeholder="LOG_LEVEL=info&#10;OPENAI_API_KEY=@OPENAI_API_KEY"></textarea>
      </div>
      <Dialog.Footer>
        <Dialog.Close>{m.common_cancel()}</Dialog.Close>
        <Button type="submit" disabled={customBusy}>
          {customBusy ? m.settings_saving() : m.common_save()}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>
