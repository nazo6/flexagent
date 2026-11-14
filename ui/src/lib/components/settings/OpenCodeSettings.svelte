<script lang="ts">
  import { untrack } from "svelte";
  import { errorMessage } from "#lib/api/error-messages.ts";
  import { Badge } from "#lib/components/ui/badge/index.ts";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { Card } from "#lib/components/ui/card/index.ts";
  import { Input } from "#lib/components/ui/input/index.ts";
  import { Label } from "#lib/components/ui/label/index.ts";
  import type { OpenCodeManagementSnapshot } from "#lib/generated/bindings.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { sync } from "#lib/stores/app.svelte.ts";
  import { toast } from "svelte-sonner";
  import RefreshCwIcon from "@lucide/svelte/icons/refresh-cw";

  let { nodeId = "" }: { nodeId?: string } = $props();

  const targetNode = $derived(nodeId !== "" ? nodeId : (sync.connection.systemInfo?.node_id ?? ""));
  let snapshot = $state<OpenCodeManagementSnapshot | null>(null);
  let loading = $state(false);
  let busy = $state<string | null>(null);
  let error = $state<string | null>(null);

  let mcpName = $state("");
  let mcpKind = $state<"local" | "remote">("local");
  let mcpCommand = $state("");
  let mcpUrl = $state("");

  $effect(() => {
    if (targetNode === "") return;
    untrack(() => void load());
  });

  async function load(): Promise<void> {
    if (targetNode === "" || loading) return;
    loading = true;
    error = null;
    try {
      snapshot = await sync.connection.client.openCodeManagement(targetNode);
    } catch (cause) {
      error = errorMessage(cause);
    } finally {
      loading = false;
    }
  }

  async function run(name: string, task: () => Promise<OpenCodeManagementSnapshot>): Promise<void> {
    if (busy !== null) return;
    busy = name;
    try {
      snapshot = await task();
    } catch (cause) {
      toast.error(errorMessage(cause));
    } finally {
      busy = null;
    }
  }

  async function addMcp(): Promise<void> {
    const name = mcpName.trim();
    if (name === "" || targetNode === "") return;
    await run("add", () =>
      sync.connection.client.upsertOpenCodeMcp(targetNode, name, {
        kind: mcpKind,
        command:
          mcpKind === "local"
            ? mcpCommand
                .trim()
                .split(/\s+/)
                .filter((part) => part !== "")
            : [],
        environment: {},
        url: mcpKind === "remote" ? mcpUrl.trim() : null,
        enabled: true,
      }),
    );
    mcpName = "";
    mcpCommand = "";
    mcpUrl = "";
  }
</script>

<Card class="gap-4 p-5">
  <div class="flex flex-wrap items-start justify-between gap-3">
    <div>
      <h2 class="text-sm font-semibold">{m.settings_tab_opencode()}</h2>
      <p class="text-muted-foreground mt-1 max-w-[60ch] text-[13px]">
        {m.opencode_description()}
      </p>
    </div>
    <Button
      variant="outline"
      size="sm"
      disabled={loading || targetNode === ""}
      onclick={() => void load()}
    >
      <RefreshCwIcon class="size-3.5" />
      {m.opencode_refresh()}
    </Button>
  </div>

  {#if error !== null}
    <p class="text-destructive text-[13px]">{m.opencode_load_failed()}: {error}</p>
  {:else if loading && snapshot === null}
    <p class="text-muted-foreground text-[13px]">{m.opencode_loading()}</p>
  {:else if snapshot !== null && !snapshot.available}
    <p class="text-muted-foreground text-[13px]">{m.opencode_unavailable()}</p>
  {:else if snapshot !== null}
    <p class="text-muted-foreground desk-mono text-[12px]">
      {m.opencode_version({ version: snapshot.version ?? "-" })}
    </p>

    <section class="grid gap-2">
      <h3 class="text-[13px] font-semibold">{m.opencode_agents()}</h3>
      <div class="flex flex-wrap gap-2">
        {#each snapshot.agents as agent (agent.id)}
          <Badge variant="outline" title={agent.description ?? agent.id}>
            {agent.name} · {agent.mode}
          </Badge>
        {:else}
          <span class="text-muted-foreground text-[13px]">{m.opencode_no_items()}</span>
        {/each}
      </div>
    </section>

    <section class="grid gap-2">
      <h3 class="text-[13px] font-semibold">{m.opencode_models()}</h3>
      <div class="flex flex-wrap gap-2">
        {#each snapshot.models.filter((model) => model.enabled) as model (model.provider_id + "/" + model.id)}
          <Badge variant="secondary" title={model.id}>
            {model.name}
            {#if model.variants.length > 0}
              · {model.variants.join("/")}
            {/if}
          </Badge>
        {:else}
          <span class="text-muted-foreground text-[13px]">{m.opencode_no_items()}</span>
        {/each}
      </div>
    </section>

    <section class="grid gap-2">
      <h3 class="text-[13px] font-semibold">{m.opencode_providers()}</h3>
      <div class="flex flex-wrap gap-2">
        {#each snapshot.providers as provider (provider.id)}
          <Badge variant="outline" title={provider.package ?? provider.id}>
            {provider.name}{#if provider.activation}
              · {provider.activation}{/if}
          </Badge>
        {:else}
          <span class="text-muted-foreground text-[13px]">{m.opencode_no_items()}</span>
        {/each}
      </div>
    </section>

    <section class="grid gap-3">
      <h3 class="text-[13px] font-semibold">{m.opencode_mcp()}</h3>
      <div class="grid gap-2">
        {#each snapshot.mcp_servers as server (server.name)}
          <div class="border-border flex flex-wrap items-center gap-2 rounded-md border px-3 py-2">
            <span class="desk-mono text-[12px]">{server.name}</span>
            <Badge variant={server.status === "connected" ? "secondary" : "outline"}>
              {server.status}
            </Badge>
            {#if server.error}
              <span class="text-destructive text-[12px]">{server.error}</span>
            {/if}
            <span class="flex-1"></span>
            <Button
              variant="outline"
              size="sm"
              disabled={busy !== null}
              onclick={() =>
                void run(server.name, () =>
                  sync.connection.client.openCodeMcpAction(
                    targetNode,
                    server.name,
                    server.status === "connected" ? "disconnect" : "connect",
                  ),
                )}
            >
              {server.status === "connected"
                ? m.opencode_mcp_disconnect()
                : m.opencode_mcp_connect()}
            </Button>
            <Button
              variant="ghost"
              size="sm"
              disabled={busy !== null}
              onclick={() =>
                void run(server.name, () =>
                  sync.connection.client.removeOpenCodeMcp(targetNode, server.name),
                )}
            >
              {m.opencode_mcp_remove()}
            </Button>
          </div>
        {:else}
          <span class="text-muted-foreground text-[13px]">{m.opencode_no_items()}</span>
        {/each}
      </div>

      <div class="grid gap-2 sm:grid-cols-2">
        <div class="grid gap-1">
          <Label for="opencode-mcp-name">{m.opencode_mcp_name()}</Label>
          <Input id="opencode-mcp-name" bind:value={mcpName} />
        </div>
        <div class="grid gap-1">
          <Label for="opencode-mcp-kind">{m.opencode_mcp_kind()}</Label>
          <select
            id="opencode-mcp-kind"
            class="border-border bg-background h-9 rounded-md border px-2 text-[13px]"
            bind:value={mcpKind}
          >
            <option value="local">{m.opencode_mcp_local()}</option>
            <option value="remote">{m.opencode_mcp_remote()}</option>
          </select>
        </div>
        {#if mcpKind === "local"}
          <div class="grid gap-1 sm:col-span-2">
            <Label for="opencode-mcp-command">{m.opencode_mcp_command()}</Label>
            <Input
              id="opencode-mcp-command"
              bind:value={mcpCommand}
              placeholder="npx -y @modelcontextprotocol/server-filesystem ."
            />
          </div>
        {:else}
          <div class="grid gap-1 sm:col-span-2">
            <Label for="opencode-mcp-url">{m.opencode_mcp_url()}</Label>
            <Input id="opencode-mcp-url" bind:value={mcpUrl} placeholder="https://…" />
          </div>
        {/if}
      </div>
      <div>
        <Button
          size="sm"
          disabled={busy !== null || mcpName.trim() === ""}
          onclick={() => void addMcp()}
        >
          {m.opencode_mcp_add()}
        </Button>
      </div>
    </section>

    <section class="grid gap-2">
      <h3 class="text-[13px] font-semibold">{m.opencode_skills()}</h3>
      <div class="flex flex-wrap gap-2">
        {#each snapshot.skills as skill (skill.id)}
          <Badge variant="outline" title={skill.description ?? skill.id}>{skill.name}</Badge>
        {:else}
          <span class="text-muted-foreground text-[13px]">{m.opencode_no_items()}</span>
        {/each}
      </div>
    </section>

    <section class="grid gap-2">
      <h3 class="text-[13px] font-semibold">{m.opencode_commands()}</h3>
      <div class="flex flex-wrap gap-2">
        {#each snapshot.commands as command (command.name)}
          <Badge variant="outline" title={command.description}>/{command.name}</Badge>
        {:else}
          <span class="text-muted-foreground text-[13px]">{m.opencode_no_items()}</span>
        {/each}
      </div>
    </section>
  {/if}
</Card>
