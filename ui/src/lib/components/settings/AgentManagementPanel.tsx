import { createEffect, createMemo, createSignal, For, Match, Show, Switch } from "solid-js";
import IconRefreshCw from "~icons/lucide/refresh-cw";
import { errorMessage } from "#lib/api/error-messages.ts";
import { Badge } from "#lib/components/ui/badge/index.ts";
import { Button } from "#lib/components/ui/button/index.ts";
import { Card } from "#lib/components/ui/card/index.ts";
import { Input } from "#lib/components/ui/input/index.ts";
import { Label } from "#lib/components/ui/label/index.ts";
import { toast } from "#lib/components/ui/sonner/index.ts";
import type { AgentManagementSnapshot } from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";
import { sync } from "#lib/stores/app.ts";

/** Native agent management (agents / models / MCP / skills) for one agent. */
export default function AgentManagementPanel(props: {
  nodeId?: string;
  agentId: string;
  agentName?: string;
  embedded?: boolean;
}) {
  const targetNode = createMemo(() =>
    (props.nodeId ?? "") !== ""
      ? (props.nodeId ?? "")
      : (sync.connection.systemInfo?.node_id ?? ""),
  );
  const [snapshot, setSnapshot] = createSignal<AgentManagementSnapshot | null>(null);
  const displayName = () =>
    snapshot()?.agent_name ??
    ((props.agentName ?? "") !== "" ? (props.agentName ?? "") : props.agentId);
  const [loading, setLoading] = createSignal(false);
  const [busy, setBusy] = createSignal<string | null>(null);
  const [error, setError] = createSignal<string | null>(null);

  const [mcpName, setMcpName] = createSignal("");
  const [mcpKind, setMcpKind] = createSignal<"local" | "remote">("local");
  const [mcpCommand, setMcpCommand] = createSignal("");
  const [mcpUrl, setMcpUrl] = createSignal("");

  createEffect(
    () => `${targetNode()}:${props.agentId}`,
    () => {
      if (targetNode() === "" || props.agentId === "") return;
      void load();
    },
  );

  async function load(): Promise<void> {
    const node = targetNode();
    const agent = props.agentId;
    if (node === "" || agent === "" || loading()) return;
    setLoading(true);
    setError(null);
    try {
      setSnapshot(await sync.connection.client.agentManagement(node, agent));
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setLoading(false);
    }
  }

  async function run(name: string, task: () => Promise<AgentManagementSnapshot>): Promise<boolean> {
    if (busy() !== null) return false;
    setBusy(name);
    try {
      setSnapshot(await task());
      return true;
    } catch (cause) {
      toast.error(errorMessage(cause));
      return false;
    } finally {
      setBusy(null);
    }
  }

  async function addMcp(): Promise<void> {
    const name = mcpName().trim();
    if (name === "" || targetNode() === "") return;
    const kind = mcpKind();
    const added = await run("add", () =>
      sync.connection.client.upsertAgentMcp(targetNode(), props.agentId, name, {
        kind,
        command:
          kind === "local"
            ? mcpCommand()
                .trim()
                .split(/\s+/)
                .filter((part) => part !== "")
            : [],
        environment: {},
        url: kind === "remote" ? mcpUrl().trim() : null,
        enabled: true,
      }),
    );
    if (!added) return;
    setMcpName("");
    setMcpCommand("");
    setMcpUrl("");
  }

  const panelBody = () => (
    <>
      <div class="flex flex-wrap items-start justify-between gap-3">
        <Show
          when={!props.embedded}
          fallback={
            <Show when={snapshot() !== null && snapshot()?.available} fallback={<span></span>}>
              <p class="text-muted-foreground desk-mono self-center text-[12px]">
                {m.agent_management_version({
                  name: displayName(),
                  version: snapshot()?.version ?? "-",
                })}
              </p>
            </Show>
          }
        >
          <div>
            <h2 class="text-sm font-semibold">{displayName()}</h2>
            <p class="text-muted-foreground mt-1 max-w-[60ch] text-[13px]">
              {m.agent_management_description()}
            </p>
          </div>
        </Show>
        <Button
          variant="outline"
          size="sm"
          disabled={loading() || targetNode() === ""}
          onClick={() => void load()}
        >
          <IconRefreshCw class="size-3.5" />
          {m.agent_management_refresh()}
        </Button>
      </div>

      <Switch
        fallback={
          <Show when={snapshot()}>
            {(current) => (
              <>
                <Show when={!props.embedded}>
                  <p class="text-muted-foreground desk-mono text-[12px]">
                    {m.agent_management_version({
                      name: displayName(),
                      version: current().version ?? "-",
                    })}
                  </p>
                </Show>

                <section class="grid gap-2">
                  <h3 class="text-[13px] font-semibold">{m.agent_management_agents()}</h3>
                  <div class="flex flex-wrap gap-2">
                    <For
                      each={current().agents}
                      fallback={
                        <span class="text-muted-foreground text-[13px]">
                          {m.agent_management_no_items()}
                        </span>
                      }
                    >
                      {(agent) => (
                        <Badge variant="outline" title={agent.description ?? agent.id}>
                          {agent.name} · {agent.mode}
                        </Badge>
                      )}
                    </For>
                  </div>
                </section>

                <section class="grid gap-2">
                  <h3 class="text-[13px] font-semibold">{m.agent_management_models()}</h3>
                  <div class="flex flex-wrap gap-2">
                    <For
                      each={current().models.filter((model) => model.enabled)}
                      fallback={
                        <span class="text-muted-foreground text-[13px]">
                          {m.agent_management_no_items()}
                        </span>
                      }
                    >
                      {(model) => (
                        <Badge variant="secondary" title={model.id}>
                          {model.name}
                          <Show when={model.variants.length > 0}>
                            {" "}
                            · {model.variants.join("/")}
                          </Show>
                        </Badge>
                      )}
                    </For>
                  </div>
                </section>

                <section class="grid gap-2">
                  <h3 class="text-[13px] font-semibold">{m.agent_management_providers()}</h3>
                  <div class="flex flex-wrap gap-2">
                    <For
                      each={current().providers}
                      fallback={
                        <span class="text-muted-foreground text-[13px]">
                          {m.agent_management_no_items()}
                        </span>
                      }
                    >
                      {(provider) => (
                        <Badge variant="outline" title={provider.package ?? provider.id}>
                          {provider.name}
                          <Show when={provider.activation}> · {provider.activation}</Show>
                        </Badge>
                      )}
                    </For>
                  </div>
                </section>

                <section class="grid gap-3">
                  <h3 class="text-[13px] font-semibold">{m.agent_management_mcp()}</h3>
                  <div class="grid gap-2">
                    <For
                      each={current().mcp_servers}
                      fallback={
                        <span class="text-muted-foreground text-[13px]">
                          {m.agent_management_no_items()}
                        </span>
                      }
                    >
                      {(server) => (
                        <div class="border-border flex flex-wrap items-center gap-2 rounded-md border px-3 py-2">
                          <span class="desk-mono text-[12px]">{server.name}</span>
                          <Badge variant={server.status === "connected" ? "secondary" : "outline"}>
                            {server.status}
                          </Badge>
                          <Show when={server.error}>
                            <span class="text-destructive text-[12px]">{server.error}</span>
                          </Show>
                          <span class="flex-1"></span>
                          <Button
                            variant="outline"
                            size="sm"
                            disabled={busy() !== null}
                            onClick={() =>
                              void run(server.name, () =>
                                sync.connection.client.agentMcpAction(
                                  targetNode(),
                                  props.agentId,
                                  server.name,
                                  server.status === "connected" ? "disconnect" : "connect",
                                ),
                              )
                            }
                          >
                            {server.status === "connected"
                              ? m.agent_management_mcp_disconnect()
                              : m.agent_management_mcp_connect()}
                          </Button>
                          <Button
                            variant="ghost"
                            size="sm"
                            disabled={busy() !== null}
                            onClick={() =>
                              void run(server.name, () =>
                                sync.connection.client.removeAgentMcp(
                                  targetNode(),
                                  props.agentId,
                                  server.name,
                                ),
                              )
                            }
                          >
                            {m.agent_management_mcp_remove()}
                          </Button>
                        </div>
                      )}
                    </For>
                  </div>

                  <div class="grid gap-2 sm:grid-cols-2">
                    <div class="grid gap-1">
                      <Label for="agent-mcp-name">{m.agent_management_mcp_name()}</Label>
                      <Input id="agent-mcp-name" value={mcpName()} onValueChange={setMcpName} />
                    </div>
                    <div class="grid gap-1">
                      <Label for="agent-mcp-kind">{m.agent_management_mcp_kind()}</Label>
                      <select
                        id="agent-mcp-kind"
                        class="border-border bg-background h-9 rounded-md border px-2 text-[13px]"
                        value={mcpKind()}
                        onChange={(event) =>
                          setMcpKind(event.currentTarget.value === "remote" ? "remote" : "local")
                        }
                      >
                        <option value="local">{m.agent_management_mcp_local()}</option>
                        <option value="remote">{m.agent_management_mcp_remote()}</option>
                      </select>
                    </div>
                    <Show
                      when={mcpKind() === "local"}
                      fallback={
                        <div class="grid gap-1 sm:col-span-2">
                          <Label for="agent-mcp-url">{m.agent_management_mcp_url()}</Label>
                          <Input
                            id="agent-mcp-url"
                            value={mcpUrl()}
                            onValueChange={setMcpUrl}
                            placeholder="https://…"
                          />
                        </div>
                      }
                    >
                      <div class="grid gap-1 sm:col-span-2">
                        <Label for="agent-mcp-command">{m.agent_management_mcp_command()}</Label>
                        <Input
                          id="agent-mcp-command"
                          value={mcpCommand()}
                          onValueChange={setMcpCommand}
                          placeholder="npx -y @modelcontextprotocol/server-filesystem ."
                        />
                      </div>
                    </Show>
                  </div>
                  <div>
                    <Button
                      size="sm"
                      disabled={busy() !== null || mcpName().trim() === ""}
                      onClick={() => void addMcp()}
                    >
                      {m.agent_management_mcp_add()}
                    </Button>
                  </div>
                </section>

                <section class="grid gap-2">
                  <h3 class="text-[13px] font-semibold">{m.agent_management_skills()}</h3>
                  <div class="flex flex-wrap gap-2">
                    <For
                      each={current().skills}
                      fallback={
                        <span class="text-muted-foreground text-[13px]">
                          {m.agent_management_no_items()}
                        </span>
                      }
                    >
                      {(skill) => (
                        <Badge variant="outline" title={skill.description ?? skill.id}>
                          {skill.name}
                        </Badge>
                      )}
                    </For>
                  </div>
                </section>

                <section class="grid gap-2">
                  <h3 class="text-[13px] font-semibold">{m.agent_management_commands()}</h3>
                  <div class="flex flex-wrap gap-2">
                    <For
                      each={current().commands}
                      fallback={
                        <span class="text-muted-foreground text-[13px]">
                          {m.agent_management_no_items()}
                        </span>
                      }
                    >
                      {(command) => (
                        <Badge variant="outline" title={command.description}>
                          /{command.name}
                        </Badge>
                      )}
                    </For>
                  </div>
                </section>
              </>
            )}
          </Show>
        }
      >
        <Match when={error() !== null}>
          <p class="text-destructive text-[13px]">
            {m.agent_management_load_failed()}: {error()}
          </p>
        </Match>
        <Match when={loading() && snapshot() === null}>
          <p class="text-muted-foreground text-[13px]">{m.agent_management_loading()}</p>
        </Match>
        <Match when={snapshot() !== null && !snapshot()?.available}>
          <p class="text-muted-foreground text-[13px]">
            {m.agent_management_unavailable({ name: displayName() })}
          </p>
        </Match>
      </Switch>
    </>
  );

  return (
    <Show when={props.embedded} fallback={<Card class="gap-4 p-5">{panelBody()}</Card>}>
      <div class="grid gap-4">{panelBody()}</div>
    </Show>
  );
}
