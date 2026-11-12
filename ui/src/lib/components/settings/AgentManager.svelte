<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { errorMessage } from "#lib/api/error-messages.ts";
  import AgentIcon from "#lib/components/agent/AgentIcon.svelte";
  import * as AlertDialog from "#lib/components/ui/alert-dialog/index.ts";
  import { Badge } from "#lib/components/ui/badge/index.ts";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { Card } from "#lib/components/ui/card/index.ts";
  import * as Dialog from "#lib/components/ui/dialog/index.ts";
  import { Input } from "#lib/components/ui/input/index.ts";
  import { Label } from "#lib/components/ui/label/index.ts";
  import * as Tabs from "#lib/components/ui/tabs/index.ts";
  import {
    agentBackendLabel,
    agentSourceLabel,
    agentsAvailableOn,
    registryDistributionLabel,
  } from "#lib/agents.ts";
  import type {
    AgentSummary,
    CustomAgentOrigin,
    RegistryAgentSummary,
  } from "#lib/generated/bindings.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { sync } from "#lib/stores/app.svelte.ts";
  import { toast } from "svelte-sonner";
  import PencilIcon from "@lucide/svelte/icons/pencil";
  import PlusIcon from "@lucide/svelte/icons/plus";
  import RefreshCwIcon from "@lucide/svelte/icons/refresh-cw";
  import SearchIcon from "@lucide/svelte/icons/search";
  import Trash2Icon from "@lucide/svelte/icons/trash-2";

  /** Target node passed by the settings page (catalog source and operation target). */
  let { nodeId = "" }: { nodeId?: string } = $props();

  let agents = $state<AgentSummary[]>([]);
  let registryAgents = $state<RegistryAgentSummary[]>([]);
  let loading = $state(false);
  /** Label of the running operation (for example `Refreshing catalog…`). null = idle. */
  let busyLabel = $state<string | null>(null);
  /** Registry agent currently being imported. */
  let importingId = $state<string | null>(null);
  /** Pending removal target. */
  let removeTarget = $state<AgentSummary | null>(null);

  // Agent dialog: registry import and the manual definition share one dialog.
  let agentOpen = $state(false);
  let agentTab = $state<"registry" | "custom">("custom");
  let catalogQuery = $state("");

  // Custom agent definition form (null = create new)
  let customBusy = $state(false);
  let customEditId = $state<string | null>(null);
  let customOrigin = $state<CustomAgentOrigin | null>(null);
  let customIcon = $state<string | null>(null);
  let customForm = $state({
    id: "",
    name: "",
    description: "",
    command: "",
    args: "",
    env: "",
    extension: "",
  });

  const busy = $derived(busyLabel !== null || importingId !== null);

  /** Agents available on this node (built-in + custom). */
  const available = $derived(agentsAvailableOn(agents, nodeId));
  /** Registry import candidates matching the search. */
  const importable = $derived(registryAgents.filter((agent) => matchesQuery(agent, catalogQuery)));
  /** IDs already provided by a built-in provider on this node. */
  const builtinIds = $derived(
    new Set(agents.filter((agent) => agent.custom_config === null).map((agent) => agent.id)),
  );
  /** Built-in extensions that a custom ACP launch can reference. */
  const extensionChoices = $derived(
    agents.filter((agent) => agent.custom_config === null && agent.backend === "acp_extended"),
  );

  onMount(() => {
    void loadAgents();
  });

  // Reload the catalog when the target node changes (the server no longer
  // returns a representative node's catalog). `loadAgents` reads `loading` and
  // `nodeId`, so run it untracked; otherwise the loading flag retriggers this
  // effect and a failing node loops forever.
  $effect(() => {
    const target = nodeId;
    if (target === "") return;
    untrack(() => void loadAgents());
  });

  function matchesQuery(agent: AgentSummary | RegistryAgentSummary, query: string): boolean {
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
    // Capture the target: it can change while the request is in flight.
    const target = nodeId;
    loading = true;
    try {
      const catalog = await sync.connection.client.nodeAgentCatalog(target);
      agents = catalog.agents;
      registryAgents = catalog.registry;
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      loading = false;
      if (nodeId !== target) void loadAgents();
    }
  }

  async function importAgent(agent: RegistryAgentSummary): Promise<void> {
    if (busy || nodeId === "" || !agent.available || agent.imported) return;
    importingId = agent.id;
    try {
      const result = await sync.connection.client.importRegistryAgent(nodeId, agent.id);
      toast.success(result.message ?? m.settings_catalog_imported({ name: agent.name }));
      await Promise.all([loadAgents(), sync.refreshNodes()]);
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      importingId = null;
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
      toast.error(errorMessage(error));
    } finally {
      busyLabel = null;
    }
  }

  async function confirmRemove(): Promise<void> {
    const target = removeTarget;
    if (target === null || busy || nodeId === "" || target.custom_config === null) return;
    busyLabel = m.settings_agent_removing({ name: target.id });
    try {
      const result = await sync.connection.client.removeCustomAgent(nodeId, target.id);
      toast.success(result.message ?? m.settings_agent_removed({ name: target.id }));
      removeTarget = null;
      await Promise.all([loadAgents(), sync.refreshNodes()]);
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      busyLabel = null;
    }
  }

  /** Reset the custom definition form (new agent). */
  function resetCustomForm(): void {
    customEditId = null;
    customOrigin = { kind: "manual" };
    customIcon = null;
    customForm = {
      id: "",
      name: "",
      description: "",
      command: "",
      args: "",
      env: "",
      extension: "",
    };
  }

  /** Open the agent dialog on the registry tab (new agent). */
  function openAdd(): void {
    resetCustomForm();
    catalogQuery = "";
    agentTab = "registry";
    agentOpen = true;
  }

  /** Open an existing custom definition in the dialog. */
  function openCustom(agent: AgentSummary): void {
    const config = agent.custom_config;
    customEditId = agent.id;
    customOrigin = config?.origin ?? { kind: "manual" };
    customIcon = config?.icon ?? null;
    customForm = {
      id: agent.id,
      name: config?.name ?? "",
      description: config?.description ?? "",
      command: config?.command ?? "",
      args: (config?.args ?? []).join(" "),
      env: formatEnv(config?.env ?? {}, config?.env_secrets ?? {}),
      extension: config?.extension ?? "",
    };
    agentTab = "custom";
    agentOpen = true;
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
        description: customForm.description.trim() === "" ? null : customForm.description.trim(),
        command: customForm.command.trim(),
        args: customForm.args.split(/\s+/).filter((value) => value !== ""),
        ...parseEnv(customForm.env),
        extension: customForm.extension === "" ? null : customForm.extension,
        icon: customIcon,
        origin: customOrigin,
      });
      toast.success(result.message ?? m.settings_custom_agent_saved({ name: id }));
      agentOpen = false;
      await Promise.all([loadAgents(), sync.refreshNodes()]);
    } catch (error) {
      toast.error(errorMessage(error));
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
      <Button variant="outline" size="sm" disabled={loading} onclick={() => void loadAgents()}>
        <RefreshCwIcon />
        {m.common_refresh()}
      </Button>
      <Button size="sm" disabled={nodeId === ""} onclick={openAdd}>
        <PlusIcon />
        {m.settings_add_agent()}
      </Button>
    </div>
  </div>

  {#if busyLabel !== null}
    <p class="text-muted-foreground text-[13px]">{busyLabel}</p>
  {/if}

  {#if nodeId === ""}
    <p class="text-muted-foreground text-sm">
      {sync.nodes.length > 0 ? m.settings_no_online_nodes() : m.settings_select_node()}
    </p>
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
              <AgentIcon icon={agent.icon} class="size-4" />
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
              <Badge variant="secondary" class="text-[11px]">
                {agentBackendLabel(agent.backend)}
              </Badge>
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
        {m.settings_remove_custom_description()}
        <code>{removeTarget?.id}</code>
        {m.settings_remove_custom_description_suffix()}
        <code>node.db</code>
        {m.settings_remove_custom_description_tail()}
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

<!-- Agent dialog: registry import and the manual definition share one surface. -->
<Dialog.Root open={agentOpen} onOpenChange={(open) => (agentOpen = open)}>
  <Dialog.Content
    class="flex max-h-[calc(100dvh-2rem)] flex-col overflow-hidden {agentTab === 'registry' &&
    customEditId === null
      ? 'sm:max-w-2xl'
      : 'sm:max-w-lg'}"
  >
    <Dialog.Header>
      <Dialog.Title>
        {customEditId === null
          ? m.settings_add_agent()
          : m.settings_edit_agent({ name: customEditId })}
      </Dialog.Title>
      <Dialog.Description>
        {#if agentTab === "registry" && customEditId === null}
          {m.settings_catalog_description({ name: nodeName(nodeId) })}
        {:else}
          {m.settings_custom_description()} <code>node.db</code>
        {/if}
      </Dialog.Description>
    </Dialog.Header>

    {#if customEditId === null}
      <Tabs.Root
        value={agentTab}
        onValueChange={(value) => (agentTab = value as "registry" | "custom")}
        class="flex min-h-0 flex-1 flex-col"
      >
        <Tabs.List variant="line" class="w-full shrink-0 border-b">
          <Tabs.Trigger value="registry">{m.settings_add_tab_registry()}</Tabs.Trigger>
          <Tabs.Trigger value="custom">{m.settings_add_tab_custom()}</Tabs.Trigger>
        </Tabs.List>

        <Tabs.Content value="registry" class="mt-3 flex min-h-0 flex-1 flex-col gap-3">
          <div class="flex items-center gap-1.5">
            <div class="relative min-w-0 flex-1">
              <SearchIcon class="text-muted-foreground absolute top-2 left-2 size-4" />
              <Input
                bind:value={catalogQuery}
                placeholder={m.settings_catalog_search_placeholder()}
                aria-label={m.settings_catalog_search_placeholder()}
                class="h-8 pl-8 text-[13px]"
              />
            </div>
            <Button
              variant="outline"
              size="sm"
              disabled={busy || nodeId === ""}
              onclick={() => void refreshCatalog()}
              title={m.settings_catalog_refresh_title()}
            >
              <RefreshCwIcon />
              {busyLabel === null ? m.settings_catalog_refresh() : m.settings_catalog_refreshing()}
            </Button>
          </div>

          <div class="flex min-h-0 flex-1 flex-col gap-2 overflow-y-auto pr-1">
            {#if importable.length === 0}
              <p class="text-muted-foreground text-sm">
                {catalogQuery.trim() === ""
                  ? m.settings_catalog_empty()
                  : m.settings_catalog_no_match()}
              </p>
            {:else}
              {#each importable as agent (agent.id)}
                <div
                  class="flex flex-wrap items-start justify-between gap-2 rounded-md border p-2.5"
                >
                  <div class="flex min-w-0 items-start gap-2">
                    <AgentIcon icon={agent.icon} class="mt-0.5 size-4" />
                    <div class="flex min-w-0 flex-col gap-0.5">
                      <div class="flex flex-wrap items-center gap-2">
                        <span class="text-sm font-medium">{agent.name}</span>
                        <span class="text-muted-foreground font-mono text-[13px]">{agent.id}</span>
                        <span class="text-muted-foreground text-[13px]">v{agent.version}</span>
                        {#each agent.distribution as kind (kind)}
                          <Badge variant="outline" class="text-[11px]">
                            {registryDistributionLabel(kind)}
                          </Badge>
                        {/each}
                        {#if agent.imported}
                          <Badge variant="secondary" class="text-[11px]">
                            {m.settings_catalog_imported_badge()}
                          </Badge>
                        {/if}
                      </div>
                      {#if agent.description}
                        <p class="text-muted-foreground text-[13px]">
                          {agent.description}
                        </p>
                      {/if}
                      {#if !agent.available && agent.unavailable_reason}
                        <p class="text-muted-foreground text-[11px]">
                          {agent.unavailable_reason}
                        </p>
                      {/if}
                    </div>
                  </div>
                  <div class="flex flex-wrap items-center gap-1.5">
                    {#if agent.imported}
                      <span class="text-muted-foreground text-[11px]">
                        {m.settings_catalog_imported_hint()}
                      </span>
                    {:else if builtinIds.has(agent.id)}
                      <Badge variant="outline" class="text-[11px]">
                        {m.settings_catalog_builtin()}
                      </Badge>
                    {:else if agent.available}
                      <Button
                        variant="outline"
                        size="sm"
                        disabled={busy || nodeId === ""}
                        onclick={() => void importAgent(agent)}
                      >
                        <PlusIcon />
                        {importingId === agent.id
                          ? m.settings_catalog_importing({ name: agent.name })
                          : m.settings_catalog_import()}
                      </Button>
                    {:else}
                      <span class="text-muted-foreground text-[11px]">
                        {m.settings_catalog_unavailable()}
                      </span>
                    {/if}
                  </div>
                </div>
              {/each}
            {/if}
          </div>
        </Tabs.Content>

        <Tabs.Content value="custom" class="mt-3 flex min-h-0 flex-1 flex-col">
          {@render customAgentForm()}
        </Tabs.Content>
      </Tabs.Root>
    {:else}
      {@render customAgentForm()}
    {/if}
  </Dialog.Content>
</Dialog.Root>

{#snippet customAgentForm()}
  <form class="flex min-h-0 flex-1 flex-col gap-3" onsubmit={submitCustom}>
    <div class="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto pr-1">
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
        <Label for="custom-description">{m.settings_description()}</Label>
        <Input
          id="custom-description"
          bind:value={customForm.description}
          placeholder="Short description shown in the agent list"
          class="text-[13px]"
        />
      </div>
      <div class="grid gap-1.5">
        <Label for="custom-extension">{m.settings_agent_extension()}</Label>
        <select
          id="custom-extension"
          bind:value={customForm.extension}
          class="border-input bg-surface-raised h-8 rounded-md border px-2 text-[13px]"
        >
          <option value="">{m.settings_agent_extension_none()}</option>
          {#each extensionChoices as extension (extension.id)}
            <option value={extension.id}>{extension.name} ({extension.id})</option>
          {/each}
        </select>
        <p class="text-muted-foreground text-[11px]">
          {m.settings_agent_extension_hint()}
        </p>
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
        <Label for="custom-env">{m.settings_env_vars()}</Label>
        <p class="text-muted-foreground text-[11px]">
          {m.settings_env_vars_hint()} <code>KEY=VALUE</code>{m.settings_env_vars_hint_secret()}
          <code>KEY=@SECRET_KEY</code>
        </p>
        <textarea
          id="custom-env"
          bind:value={customForm.env}
          rows="3"
          class="border-input bg-surface-raised focus-visible:ring-ring/40 rounded-md border px-2 py-1.5 font-mono text-[13px] outline-none focus-visible:ring-1"
          placeholder="LOG_LEVEL=info&#10;OPENAI_API_KEY=@OPENAI_API_KEY"></textarea>
      </div>
    </div>
    <Dialog.Footer class="shrink-0">
      <Dialog.Close>{m.common_cancel()}</Dialog.Close>
      <Button type="submit" disabled={customBusy}>
        {customBusy ? m.settings_saving() : m.common_save()}
      </Button>
    </Dialog.Footer>
  </form>
{/snippet}
