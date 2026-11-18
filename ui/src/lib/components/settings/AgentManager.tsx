import {
  createEffect,
  createMemo,
  createSignal,
  createStore,
  For,
  onSettled,
  Show,
} from "solid-js";
import IconPencil from "~icons/lucide/pencil";
import IconPlus from "~icons/lucide/plus";
import IconRefreshCw from "~icons/lucide/refresh-cw";
import IconSearch from "~icons/lucide/search";
import IconSettings2 from "~icons/lucide/settings-2";
import IconTrash2 from "~icons/lucide/trash-2";
import AgentIcon from "#lib/components/agent/AgentIcon.tsx";
import AgentManagementPanel from "#lib/components/settings/AgentManagementPanel.tsx";
import * as AlertDialog from "#lib/components/ui/alert-dialog/index.ts";
import { Badge } from "#lib/components/ui/badge/index.ts";
import { Button } from "#lib/components/ui/button/index.ts";
import { Card } from "#lib/components/ui/card/index.ts";
import * as Dialog from "#lib/components/ui/dialog/index.ts";
import { Input } from "#lib/components/ui/input/index.ts";
import { Label } from "#lib/components/ui/label/index.ts";
import * as Select from "#lib/components/ui/select/index.ts";
import * as Tabs from "#lib/components/ui/tabs/index.ts";
import { toast } from "#lib/components/ui/sonner/index.ts";
import {
  agentBackendLabel,
  agentSourceLabel,
  agentsAvailableOn,
  registryDistributionLabel,
} from "#lib/agents.ts";
import { errorMessage } from "#lib/api/error-messages.ts";
import type {
  AgentSummary,
  CustomAgentOrigin,
  RegistryAgentSummary,
} from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";
import { sync } from "#lib/stores/app.ts";
import { cn } from "#lib/utils.ts";

interface CustomForm {
  id: string;
  name: string;
  description: string;
  command: string;
  args: string;
  env: string;
  extension: string;
  subagents: boolean;
}

const emptyCustomForm = (): CustomForm => ({
  id: "",
  name: "",
  description: "",
  command: "",
  args: "",
  env: "",
  extension: "",
  subagents: true,
});

/** Agent catalog: installed agents, registry import and custom definitions. */
export default function AgentManager(props: {
  /** Target node passed by the settings page (catalog source and operation target). */
  nodeId?: string;
}) {
  const nodeId = () => props.nodeId ?? "";

  const [managedAgent, setManagedAgent] = createSignal<AgentSummary | null>(null);

  const [agents, setAgents] = createSignal<AgentSummary[]>([]);
  const [registryAgents, setRegistryAgents] = createSignal<RegistryAgentSummary[]>([]);
  const [loading, setLoading] = createSignal(false);
  /** Guard against overlapping catalog loads (a signal write is only visible
   * after the flush, and the retry below runs in the same microtask). */
  let inFlight = false;
  /** Label of the running operation (for example `Refreshing catalog…`). null = idle. */
  const [busyLabel, setBusyLabel] = createSignal<string | null>(null);
  /** Registry agent currently being imported. */
  const [importingId, setImportingId] = createSignal<string | null>(null);
  /** Pending removal target. */
  const [removeTarget, setRemoveTarget] = createSignal<AgentSummary | null>(null);

  // Agent dialog: registry import and the manual definition share one dialog.
  const [agentOpen, setAgentOpen] = createSignal(false);
  const [agentTab, setAgentTab] = createSignal<"registry" | "custom">("custom");
  const [catalogQuery, setCatalogQuery] = createSignal("");

  // Custom agent definition form (null = create new)
  const [customBusy, setCustomBusy] = createSignal(false);
  const [customEditId, setCustomEditId] = createSignal<string | null>(null);
  const [customOrigin, setCustomOrigin] = createSignal<CustomAgentOrigin | null>(null);
  const [customIcon, setCustomIcon] = createSignal<string | null>(null);
  const [customForm, setCustomForm] = createStore<CustomForm>(emptyCustomForm());

  const busy = () => busyLabel() !== null || importingId() !== null;

  /** Agents available on this node (built-in + custom). */
  const available = createMemo(() => agentsAvailableOn(agents(), nodeId()));
  /** Registry import candidates matching the search. */
  const importable = createMemo(() =>
    registryAgents().filter((agent) => matchesQuery(agent, catalogQuery())),
  );
  /** IDs already provided by a built-in provider on this node. */
  const builtinIds = createMemo(
    () =>
      new Set(
        agents()
          .filter((agent) => agent.custom_config === null)
          .map((agent) => agent.id),
      ),
  );
  /** Built-in extensions that a custom ACP launch can reference. */
  const extensionChoices = createMemo(() => {
    const byId = new Map<string, string>();
    for (const agent of agents()) {
      if (agent.extension !== null) byId.set(agent.extension, agent.name);
    }
    return [...byId].map(([id, name]) => ({ id, name }));
  });

  onSettled(() => {
    void loadAgents();
  });

  // Reload the catalog when the target node changes (the server no longer
  // returns a representative node's catalog). `loadAgents` reads `loading` and
  // `nodeId`, so run it untracked; otherwise the loading flag retriggers this
  // effect and a failing node loops forever.
  createEffect(
    () => nodeId(),
    (target) => {
      if (target === "") return;
      void loadAgents();
    },
  );

  function extensionLabel(value: string): string {
    if (value === "") return m.settings_agent_extension_none();
    const extension = extensionChoices().find((candidate) => candidate.id === value);
    return extension ? `${extension.name} (${extension.id})` : value;
  }

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
    const target = nodeId();
    if (inFlight || target === "") return;
    inFlight = true;
    // Capture the target: it can change while the request is in flight.
    setLoading(true);
    try {
      const catalog = await sync.connection.client.nodeAgentCatalog(target);
      setAgents(catalog.agents);
      setRegistryAgents(catalog.registry);
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      inFlight = false;
      setLoading(false);
      if (nodeId() !== target) void loadAgents();
    }
  }

  async function importAgent(agent: RegistryAgentSummary): Promise<void> {
    if (busy() || nodeId() === "" || !agent.available || agent.imported) return;
    setImportingId(agent.id);
    try {
      const result = await sync.connection.client.importRegistryAgent(nodeId(), agent.id);
      toast.success(result.message ?? m.settings_catalog_imported({ name: agent.name }));
      await Promise.all([loadAgents(), sync.refreshNodes()]);
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      setImportingId(null);
    }
  }

  async function refreshCatalog(): Promise<void> {
    if (busy() || nodeId() === "") return;
    setBusyLabel(m.settings_catalog_refreshing());
    try {
      const result = await sync.connection.client.refreshAgentsCatalog(nodeId());
      toast.success(result.message ?? m.settings_catalog_refreshed());
      await Promise.all([loadAgents(), sync.refreshNodes()]);
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      setBusyLabel(null);
    }
  }

  async function confirmRemove(): Promise<void> {
    const target = removeTarget();
    if (target === null || busy() || nodeId() === "" || target.custom_config === null) return;
    setBusyLabel(m.settings_agent_removing({ name: target.id }));
    try {
      const result = await sync.connection.client.removeCustomAgent(nodeId(), target.id);
      toast.success(result.message ?? m.settings_agent_removed({ name: target.id }));
      setRemoveTarget(null);
      await Promise.all([loadAgents(), sync.refreshNodes()]);
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      setBusyLabel(null);
    }
  }

  /** Reset the custom definition form (new agent). */
  function resetCustomForm(): void {
    setCustomEditId(null);
    setCustomOrigin({ kind: "manual" });
    setCustomIcon(null);
    setCustomForm((form) => {
      const next = emptyCustomForm();
      form.id = next.id;
      form.name = next.name;
      form.description = next.description;
      form.command = next.command;
      form.args = next.args;
      form.env = next.env;
      form.extension = next.extension;
      form.subagents = next.subagents;
    });
  }

  /** Open the agent dialog on the registry tab (new agent). */
  function openAdd(): void {
    resetCustomForm();
    setCatalogQuery("");
    setAgentTab("registry");
    setAgentOpen(true);
  }

  /** Open an existing custom definition in the dialog. */
  function openCustom(agent: AgentSummary): void {
    const config = agent.custom_config;
    setCustomEditId(agent.id);
    setCustomOrigin(config?.origin ?? { kind: "manual" });
    setCustomIcon(config?.icon ?? null);
    setCustomForm((form) => {
      form.id = agent.id;
      form.name = config?.name ?? "";
      form.description = config?.description ?? "";
      form.command = config?.command ?? "";
      form.args = (config?.args ?? []).join(" ");
      form.env = formatEnv(config?.env ?? {}, config?.env_secrets ?? {});
      form.extension = config?.extension ?? "";
      form.subagents = agent.capabilities.subagents ?? true;
    });
    setAgentTab("custom");
    setAgentOpen(true);
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
    if (customBusy() || nodeId() === "") return;
    const id = (customEditId() ?? customForm.id).trim();
    if (id === "") {
      toast.error(m.settings_agent_id_required());
      return;
    }
    if (customForm.command.trim() === "") {
      toast.error(m.settings_agent_command_required());
      return;
    }
    setCustomBusy(true);
    try {
      const result = await sync.connection.client.upsertCustomAgent(nodeId(), id, {
        name: customForm.name.trim() === "" ? null : customForm.name.trim(),
        description: customForm.description.trim() === "" ? null : customForm.description.trim(),
        command: customForm.command.trim(),
        args: customForm.args.split(/\s+/).filter((value) => value !== ""),
        ...parseEnv(customForm.env),
        extension: customForm.extension === "" ? null : customForm.extension,
        capabilities: { subagents: customForm.subagents },
        icon: customIcon(),
        origin: customOrigin(),
      });
      toast.success(result.message ?? m.settings_custom_agent_saved({ name: id }));
      setAgentOpen(false);
      await Promise.all([loadAgents(), sync.refreshNodes()]);
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      setCustomBusy(false);
    }
  }

  const customAgentForm = () => (
    <form class="flex min-h-0 flex-1 flex-col gap-3" onSubmit={(event) => void submitCustom(event)}>
      <div class="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto pr-1">
        <div class="grid gap-1.5">
          <Label for="custom-id">{m.settings_agent_id()}</Label>
          <Input
            id="custom-id"
            value={customForm.id}
            onValueChange={(value) =>
              setCustomForm((form) => {
                form.id = value;
              })
            }
            disabled={customEditId() !== null}
            placeholder="my-agent"
            class="font-mono text-[13px]"
          />
        </div>
        <div class="grid gap-1.5">
          <Label for="custom-name">{m.settings_display_name()}</Label>
          <Input
            id="custom-name"
            value={customForm.name}
            onValueChange={(value) =>
              setCustomForm((form) => {
                form.name = value;
              })
            }
            placeholder="My Agent"
            class="text-[13px]"
          />
        </div>
        <div class="grid gap-1.5">
          <Label for="custom-description">{m.settings_description()}</Label>
          <Input
            id="custom-description"
            value={customForm.description}
            onValueChange={(value) =>
              setCustomForm((form) => {
                form.description = value;
              })
            }
            placeholder="Short description shown in the agent list"
            class="text-[13px]"
          />
        </div>
        <div class="grid gap-1.5">
          <Label for="custom-extension">{m.settings_agent_extension()}</Label>
          <Select.Root
            type="single"
            value={customForm.extension === "" ? "none" : customForm.extension}
            onValueChange={(value) =>
              setCustomForm((form) => {
                form.extension = value === "none" ? "" : String(value);
              })
            }
          >
            <Select.Trigger id="custom-extension" size="sm" class="w-full text-[13px]">
              <span>{extensionLabel(customForm.extension)}</span>
            </Select.Trigger>
            <Select.Content>
              <Select.Item
                value="none"
                label={m.settings_agent_extension_none()}
                class="text-[13px]"
              >
                {m.settings_agent_extension_none()}
              </Select.Item>
              <For each={extensionChoices()}>
                {(extension) => (
                  <Select.Item
                    value={extension.id}
                    label={`${extension.name} (${extension.id})`}
                    class="text-[13px]"
                  >
                    {extension.name} ({extension.id})
                  </Select.Item>
                )}
              </For>
            </Select.Content>
          </Select.Root>
          <p class="text-muted-foreground text-[11px]">{m.settings_agent_extension_hint()}</p>
        </div>
        <div class="flex items-start justify-between gap-3">
          <div class="flex min-w-0 flex-col gap-0.5">
            <Label for="custom-subagents">{m.settings_agent_capabilities_subagents()}</Label>
            <p class="text-muted-foreground text-[11px]">
              {m.settings_agent_capabilities_subagents_hint()}
            </p>
          </div>
          <button
            id="custom-subagents"
            type="button"
            role="switch"
            aria-checked={customForm.subagents ? "true" : "false"}
            aria-label={m.settings_agent_capabilities_subagents()}
            class={cn(
              "relative inline-flex h-5 w-9 shrink-0 items-center rounded-full transition-colors",
              customForm.subagents ? "bg-signal" : "bg-muted",
            )}
            onClick={() =>
              setCustomForm((form) => {
                form.subagents = !form.subagents;
              })
            }
          >
            <span
              class={cn(
                "bg-background size-4 rounded-full shadow transition-transform",
                customForm.subagents ? "translate-x-4" : "translate-x-0.5",
              )}
            ></span>
          </button>
        </div>
        <div class="grid gap-1.5">
          <Label for="custom-command">{m.settings_command()}</Label>
          <Input
            id="custom-command"
            value={customForm.command}
            onValueChange={(value) =>
              setCustomForm((form) => {
                form.command = value;
              })
            }
            placeholder="npx / node / /path/to/agent"
            class="font-mono text-[13px]"
          />
          <p class="text-muted-foreground text-[11px]">{m.settings_command_hint()}</p>
        </div>
        <div class="grid gap-1.5">
          <Label for="custom-args">{m.settings_args()}</Label>
          <Input
            id="custom-args"
            value={customForm.args}
            onValueChange={(value) =>
              setCustomForm((form) => {
                form.args = value;
              })
            }
            placeholder="-y @scope/agent-acp"
            class="font-mono text-[13px]"
          />
        </div>
        <div class="grid gap-1.5">
          <Label for="custom-env">{m.settings_env_vars()}</Label>
          <p class="text-muted-foreground text-[11px]">
            {m.settings_env_vars_hint()} <code>KEY=VALUE</code>
            {m.settings_env_vars_hint_secret()} <code>KEY=@SECRET_KEY</code>
          </p>
          <textarea
            id="custom-env"
            rows={3}
            class="border-input bg-surface-raised focus-visible:ring-ring/40 rounded-md border px-2 py-1.5 font-mono text-[13px] outline-none focus-visible:ring-1"
            value={customForm.env}
            onInput={(event) =>
              setCustomForm((form) => {
                form.env = event.currentTarget.value;
              })
            }
            placeholder={"LOG_LEVEL=info\nOPENAI_API_KEY=@OPENAI_API_KEY"}
          ></textarea>
        </div>
      </div>
      <Dialog.Footer class="shrink-0">
        <Dialog.Close>{m.common_cancel()}</Dialog.Close>
        <Button type="submit" disabled={customBusy()}>
          {customBusy() ? m.settings_saving() : m.common_save()}
        </Button>
      </Dialog.Footer>
    </form>
  );

  return (
    <>
      <div class="flex flex-col gap-3">
        <div class="flex flex-wrap items-center justify-between gap-2">
          <p class="text-muted-foreground text-sm">{m.settings_agents_description()}</p>
          <div class="flex flex-wrap items-center gap-1.5">
            <Button
              variant="outline"
              size="sm"
              disabled={loading()}
              onClick={() => void loadAgents()}
            >
              <IconRefreshCw />
              {m.common_refresh()}
            </Button>
            <Button size="sm" disabled={nodeId() === ""} onClick={openAdd}>
              <IconPlus />
              {m.settings_add_agent()}
            </Button>
          </div>
        </div>

        <Show when={busyLabel() !== null}>
          <p class="text-muted-foreground text-[13px]">{busyLabel()}</p>
        </Show>

        <Show
          when={nodeId() !== ""}
          fallback={
            <p class="text-muted-foreground text-sm">
              {sync.nodes.length > 0 ? m.settings_no_online_nodes() : m.settings_select_node()}
            </p>
          }
        >
          <Show
            when={!(loading() && agents().length === 0)}
            fallback={<p class="text-muted-foreground text-sm">{m.settings_registry_loading()}</p>}
          >
            <Show
              when={available().length !== 0}
              fallback={<p class="text-muted-foreground text-sm">{m.settings_no_agents()}</p>}
            >
              <For each={available()}>
                {(agent) => (
                  <Card class="gap-2 p-4">
                    <div class="flex items-start gap-2">
                      <div class="flex min-w-0 flex-1 flex-col gap-0.5">
                        <div class="flex flex-wrap items-center gap-2">
                          <AgentIcon icon={agent.icon} class="size-4" />
                          <h2 class="font-medium">{agent.name}</h2>
                          <span class="text-muted-foreground font-mono text-[13px]">
                            {agent.id}
                          </span>
                          <Show when={agent.version !== "-"}>
                            <span class="text-muted-foreground text-[13px]">v{agent.version}</span>
                          </Show>
                        </div>
                        <Show when={agent.description}>
                          <p class="text-muted-foreground text-[13px]">{agent.description}</p>
                        </Show>
                        <div class="mt-0.5 flex flex-wrap items-center gap-1.5">
                          <Badge variant="secondary" class="text-[11px]">
                            {agentBackendLabel(agent.backend)}
                          </Badge>
                          <Show when={agent.extension !== null}>
                            <Badge variant="outline" class="text-[11px]">
                              {m.settings_agent_extension()}: {agent.extension}
                            </Badge>
                          </Show>
                          <Show when={agent.backend !== "custom"}>
                            <Badge variant="outline" class="text-[11px]">
                              {agent.capabilities.subagents === false
                                ? m.settings_agent_capabilities_subagents_off()
                                : m.settings_agent_capabilities_subagents()}
                            </Badge>
                          </Show>
                          <For each={agent.sources}>
                            {(source) => (
                              <Badge variant="outline" class="text-[11px]">
                                {agentSourceLabel(source)}
                              </Badge>
                            )}
                          </For>
                        </div>
                        <Show when={agent.external_path}>
                          <p class="text-muted-foreground font-mono text-[11px] break-all">
                            {agent.external_path}
                          </p>
                        </Show>
                      </div>

                      <div class="flex shrink-0 items-center gap-1.5">
                        <Show when={agent.has_management}>
                          <Button
                            variant={managedAgent()?.id === agent.id ? "secondary" : "ghost"}
                            size="icon-sm"
                            disabled={busy() || nodeId() === ""}
                            onClick={() =>
                              setManagedAgent(managedAgent()?.id === agent.id ? null : agent)
                            }
                            title={agent.name}
                          >
                            <IconSettings2 />
                            <span class="sr-only">{agent.name}</span>
                          </Button>
                        </Show>
                        <Show when={agent.custom_config !== null}>
                          <Button
                            variant="ghost"
                            size="icon-sm"
                            disabled={busy() || nodeId() === ""}
                            onClick={() => openCustom(agent)}
                            title={m.settings_edit_custom_title()}
                          >
                            <IconPencil />
                            <span class="sr-only">{m.settings_edit_agent({ name: agent.id })}</span>
                          </Button>
                        </Show>
                        <Show when={agent.custom_config !== null}>
                          <Button
                            variant="ghost"
                            size="icon-sm"
                            class="text-muted-foreground hover:text-destructive"
                            disabled={busy() || nodeId() === ""}
                            onClick={() => setRemoveTarget(agent)}
                          >
                            <IconTrash2 />
                            <span class="sr-only">
                              {m.settings_remove_agent({ name: agent.id })}
                            </span>
                          </Button>
                        </Show>
                      </div>
                    </div>

                    <Show when={agent.installed_nodes.length > 1}>
                      <p class="text-muted-foreground text-[11px]">
                        {m.settings_available_nodes()}{" "}
                        {agent.installed_nodes.map((id) => nodeName(id)).join(", ")}
                      </p>
                    </Show>
                  </Card>
                )}
              </For>
            </Show>
          </Show>
        </Show>
      </div>

      {/* Agent runtime management dialog */}
      <Dialog.Root
        open={managedAgent() !== null}
        onOpenChange={(open) => {
          if (!open) setManagedAgent(null);
        }}
      >
        <Dialog.Content class="flex max-h-[calc(100dvh-2rem)] flex-col overflow-hidden sm:max-w-2xl">
          <Show when={managedAgent()}>
            {(agent) => (
              <>
                <Dialog.Header>
                  <Dialog.Title>{agent().name}</Dialog.Title>
                  <Dialog.Description>{m.agent_management_description()}</Dialog.Description>
                </Dialog.Header>
                <div class="min-h-0 flex-1 overflow-y-auto pr-1">
                  <AgentManagementPanel
                    nodeId={nodeId()}
                    agentId={agent().id}
                    agentName={agent().name}
                    embedded
                  />
                </div>
                <Dialog.Footer class="shrink-0">
                  <Dialog.Close>{m.common_close()}</Dialog.Close>
                </Dialog.Footer>
              </>
            )}
          </Show>
        </Dialog.Content>
      </Dialog.Root>

      {/* Agent removal confirmation */}
      <AlertDialog.Root
        open={removeTarget() !== null}
        onOpenChange={(open) => {
          if (!open) setRemoveTarget(null);
        }}
      >
        <AlertDialog.Content>
          <AlertDialog.Header>
            <AlertDialog.Title>{m.settings_remove_agent_title()}</AlertDialog.Title>
            <AlertDialog.Description class="text-sm">
              {m.settings_remove_custom_description()} <code>{removeTarget()?.id}</code>
              {m.settings_remove_custom_description_suffix()} <code>node.db</code>{" "}
              {m.settings_remove_custom_description_tail()}
            </AlertDialog.Description>
          </AlertDialog.Header>
          <AlertDialog.Footer>
            <AlertDialog.Cancel disabled={busy()}>{m.common_cancel()}</AlertDialog.Cancel>
            <AlertDialog.Action
              variant="destructive"
              disabled={busy()}
              onClick={() => void confirmRemove()}
            >
              {m.common_delete()}
            </AlertDialog.Action>
          </AlertDialog.Footer>
        </AlertDialog.Content>
      </AlertDialog.Root>

      {/* Agent dialog: registry import and the manual definition share one surface. */}
      <Dialog.Root open={agentOpen()} onOpenChange={setAgentOpen}>
        <Dialog.Content
          class={cn(
            "flex max-h-[calc(100dvh-2rem)] flex-col overflow-hidden",
            agentTab() === "registry" && customEditId() === null ? "sm:max-w-2xl" : "sm:max-w-lg",
          )}
        >
          <Dialog.Header>
            <Dialog.Title>
              {customEditId() === null
                ? m.settings_add_agent()
                : m.settings_edit_agent({ name: customEditId() ?? "" })}
            </Dialog.Title>
            <Dialog.Description>
              <Show
                when={agentTab() === "registry" && customEditId() === null}
                fallback={
                  <>
                    {m.settings_custom_description()} <code>node.db</code>
                  </>
                }
              >
                {m.settings_catalog_description({ name: nodeName(nodeId()) })}
              </Show>
            </Dialog.Description>
          </Dialog.Header>

          <Show
            when={customEditId() === null}
            fallback={<Show when={customEditId() !== null}>{customAgentForm()}</Show>}
          >
            <Tabs.Root
              value={agentTab()}
              onValueChange={(value) => {
                if (value === "registry" || value === "custom") setAgentTab(value);
              }}
              class="flex min-h-0 flex-1 flex-col"
            >
              <Tabs.List variant="line" class="w-full shrink-0 border-b">
                <Tabs.Trigger value="registry">{m.settings_add_tab_registry()}</Tabs.Trigger>
                <Tabs.Trigger value="custom">{m.settings_add_tab_custom()}</Tabs.Trigger>
              </Tabs.List>

              <Tabs.Content value="registry" class="mt-3 flex min-h-0 flex-1 flex-col gap-3">
                <div class="flex items-center gap-1.5">
                  <div class="relative min-w-0 flex-1">
                    <IconSearch class="text-muted-foreground absolute top-2 left-2 size-4" />
                    <Input
                      value={catalogQuery()}
                      onValueChange={setCatalogQuery}
                      placeholder={m.settings_catalog_search_placeholder()}
                      aria-label={m.settings_catalog_search_placeholder()}
                      class="h-8 pl-8 text-[13px]"
                    />
                  </div>
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={busy() || nodeId() === ""}
                    onClick={() => void refreshCatalog()}
                    title={m.settings_catalog_refresh_title()}
                  >
                    <IconRefreshCw />
                    {busyLabel() === null
                      ? m.settings_catalog_refresh()
                      : m.settings_catalog_refreshing()}
                  </Button>
                </div>

                <div class="flex min-h-0 flex-1 flex-col gap-2 overflow-y-auto pr-1">
                  <Show
                    when={importable().length !== 0}
                    fallback={
                      <p class="text-muted-foreground text-sm">
                        {catalogQuery().trim() === ""
                          ? m.settings_catalog_empty()
                          : m.settings_catalog_no_match()}
                      </p>
                    }
                  >
                    <For each={importable()}>
                      {(agent) => (
                        <div class="flex items-start gap-2 rounded-md border p-2.5">
                          <div class="flex min-w-0 flex-1 items-start gap-2">
                            <AgentIcon icon={agent.icon} class="mt-0.5 size-4" />
                            <div class="flex min-w-0 flex-1 flex-col gap-0.5">
                              <div class="flex flex-wrap items-center gap-2">
                                <span class="text-sm font-medium">{agent.name}</span>
                                <span class="text-muted-foreground font-mono text-[13px]">
                                  {agent.id}
                                </span>
                                <span class="text-muted-foreground text-[13px]">
                                  v{agent.version}
                                </span>
                                <For each={agent.distribution}>
                                  {(kind) => (
                                    <Badge variant="outline" class="text-[11px]">
                                      {registryDistributionLabel(kind)}
                                    </Badge>
                                  )}
                                </For>
                                <Show when={agent.imported}>
                                  <Badge variant="secondary" class="text-[11px]">
                                    {m.settings_catalog_imported_badge()}
                                  </Badge>
                                </Show>
                              </div>
                              <Show when={agent.description}>
                                <p class="text-muted-foreground text-[13px]">{agent.description}</p>
                              </Show>
                              <Show when={!agent.available && agent.unavailable_reason}>
                                <p class="text-muted-foreground text-[11px]">
                                  {agent.unavailable_reason}
                                </p>
                              </Show>
                            </div>
                          </div>
                          <div class="flex shrink-0 items-center gap-1.5">
                            <Show
                              when={!agent.imported}
                              fallback={
                                <span class="text-muted-foreground text-[11px]">
                                  {m.settings_catalog_imported_hint()}
                                </span>
                              }
                            >
                              <Show
                                when={!builtinIds().has(agent.id)}
                                fallback={
                                  <Badge variant="outline" class="text-[11px]">
                                    {m.settings_catalog_builtin()}
                                  </Badge>
                                }
                              >
                                <Show
                                  when={agent.available}
                                  fallback={
                                    <span class="text-muted-foreground text-[11px]">
                                      {m.settings_catalog_unavailable()}
                                    </span>
                                  }
                                >
                                  <Button
                                    variant="outline"
                                    size="sm"
                                    disabled={busy() || nodeId() === ""}
                                    onClick={() => void importAgent(agent)}
                                  >
                                    <IconPlus />
                                    {importingId() === agent.id
                                      ? m.settings_catalog_importing({ name: agent.name })
                                      : m.settings_catalog_import()}
                                  </Button>
                                </Show>
                              </Show>
                            </Show>
                          </div>
                        </div>
                      )}
                    </For>
                  </Show>
                </div>
              </Tabs.Content>

              <Tabs.Content value="custom" class="mt-3 flex min-h-0 flex-1 flex-col">
                {customAgentForm()}
              </Tabs.Content>
            </Tabs.Root>
          </Show>
        </Dialog.Content>
      </Dialog.Root>
    </>
  );
}
