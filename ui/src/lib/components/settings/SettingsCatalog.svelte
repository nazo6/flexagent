<script lang="ts">
  import { untrack } from "svelte";
  import { Badge } from "#lib/components/ui/badge/index.ts";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { Card } from "#lib/components/ui/card/index.ts";
  import { Input } from "#lib/components/ui/input/index.ts";
  import { Label } from "#lib/components/ui/label/index.ts";
  import type {
    NodeSettingsSnapshot,
    SecretSummary,
    HubSettingsSnapshot,
  } from "#lib/generated/bindings.ts";
  import { sync } from "#lib/stores/app.svelte.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { toast } from "svelte-sonner";

  type Section = "bootstrap" | "machine" | "hub";

  let { section }: { section: Section } = $props();

  const isHub = $derived(sync.connection.role === "hub");
  const onlineNodes = $derived(sync.nodes.filter((node) => node.is_online));

  let selectedNode = $state("");
  let nodeSnapshot = $state<NodeSettingsSnapshot | null>(null);
  let hubSnapshot = $state<HubSettingsSnapshot | null>(null);
  let secrets = $state<SecretSummary[]>([]);
  let loading = $state(false);
  let error = $state<string | null>(null);

  // Node settings form
  let nodeForm = $state({
    name: "",
    allow_remote_pty: false,
    project_scan_dirs: "",
    tray: "" as "" | "true" | "false",
    revision: 0,
  });

  // Hub settings form
  let hubForm = $state({ vapid_subject: "", revision: 0 });

  // Secret form
  let secretForm = $state({
    key: "",
    description: "",
    value: "",
  });

  const targetNode = $derived(isHub ? selectedNode : (sync.connection.systemInfo?.node_id ?? ""));
  const showSecrets = $derived(section === "machine" || (section === "hub" && isHub));

  $effect(() => {
    if (isHub && selectedNode === "" && onlineNodes.length > 0) {
      selectedNode = onlineNodes[0].node_id;
    }
  });

  $effect(() => {
    // Track only the keys; `load` itself must not become an effect dependency
    // (otherwise setting `loading` would retrigger it).
    const key = `${section}:${targetNode}:${sync.connection.role}`;
    void key;
    untrack(() => void load());
  });

  async function load(): Promise<void> {
    if (loading) return;
    loading = true;
    error = null;
    try {
      if (section === "bootstrap" || section === "machine") {
        nodeSnapshot =
          targetNode !== "" ? await sync.connection.client.nodeSettings(targetNode) : null;
        syncNodeForm();
      }
      if (section === "bootstrap" || section === "machine" || section === "hub") {
        if (isHub) {
          hubSnapshot = await sync.connection.client.hubSettings();
          syncHubForm();
        }
      }
      if (section === "machine") {
        secrets =
          targetNode !== "" ? (await sync.connection.client.nodeSecrets(targetNode)).secrets : [];
      } else if (section === "hub" && isHub) {
        secrets = (await sync.connection.client.hubSecrets()).secrets;
      }
    } catch (cause) {
      error = cause instanceof Error ? cause.message : String(cause);
    } finally {
      loading = false;
    }
  }

  function syncNodeForm(): void {
    if (!nodeSnapshot) return;
    nodeForm = {
      name: nodeSnapshot.node.name,
      allow_remote_pty: nodeSnapshot.node.allow_remote_pty,
      project_scan_dirs: nodeSnapshot.node.project_scan_dirs.join("\n"),
      tray: nodeSnapshot.node.tray === null ? "" : nodeSnapshot.node.tray ? "true" : "false",
      revision: nodeSnapshot.node.revision,
    };
  }

  function syncHubForm(): void {
    if (!hubSnapshot) return;
    hubForm = {
      vapid_subject: hubSnapshot.hub.vapid_subject ?? "",
      revision: hubSnapshot.hub.revision,
    };
  }

  async function saveNode(): Promise<void> {
    const request = {
      name: nodeForm.name.trim(),
      allow_remote_pty: nodeForm.allow_remote_pty,
      project_scan_dirs: nodeForm.project_scan_dirs
        .split("\n")
        .map((line) => line.trim())
        .filter((line) => line !== ""),
      tray: nodeForm.tray === "" ? null : nodeForm.tray === "true",
      expected_revision: nodeForm.revision,
    };
    try {
      if (targetNode === "") throw new Error("node is not selected");
      nodeSnapshot = await sync.connection.client.updateNodeSettings(targetNode, request);
      syncNodeForm();
      toast.success("Saved node settings.");
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : String(cause));
    }
  }

  async function saveHub(): Promise<void> {
    try {
      hubSnapshot = await sync.connection.client.updateHubSettings({
        vapid_subject: hubForm.vapid_subject.trim() === "" ? null : hubForm.vapid_subject.trim(),
        expected_revision: hubForm.revision,
      });
      syncHubForm();
      toast.success("Saved hub settings.");
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : String(cause));
    }
  }

  async function saveSecret(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    const key = secretForm.key.trim();
    if (secretForm.value === "" || key === "") return;
    try {
      const request = {
        description: secretForm.description.trim() === "" ? null : secretForm.description.trim(),
        value: secretForm.value,
      };
      if (!(section === "hub" && isHub) && targetNode === "") {
        throw new Error("node is not selected");
      }
      const response =
        section === "hub" && isHub
          ? await sync.connection.client.upsertHubSecret(key, request)
          : await sync.connection.client.upsertNodeSecret(targetNode, key, request);
      secrets = response.secrets;
      secretForm = { key: "", description: "", value: "" };
      toast.success("Secret saved (the value is write-only and never returned).");
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : String(cause));
    }
  }

  async function deleteSecret(secret: SecretSummary): Promise<void> {
    if (!confirm(`Delete secret "${secret.id}"?`)) return;
    try {
      if (!(section === "hub" && isHub) && targetNode === "") {
        throw new Error("node is not selected");
      }
      const response =
        section === "hub" && isHub
          ? await sync.connection.client.deleteHubSecret(secret.id)
          : await sync.connection.client.deleteNodeSecret(targetNode, secret.id);
      secrets = response.secrets;
      toast.success(`Deleted secret "${secret.id}".`);
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : String(cause));
    }
  }

  function sourceBadge(source: string): string {
    return source.replaceAll("_", " ");
  }

  type Row = { label: string; value: string; source: string; detail?: string | null };

  const bootstrapRows = $derived<Row[]>(
    section === "bootstrap"
      ? isHub
        ? hubSnapshot
          ? [
              {
                label: "Data directory",
                value: hubSnapshot.bootstrap.fxg_home.value,
                source: hubSnapshot.bootstrap.fxg_home.source,
              },
              {
                label: "Listen address",
                value: hubSnapshot.bootstrap.listen_addr.value,
                source: hubSnapshot.bootstrap.listen_addr.source,
              },
              {
                label: "Allowed hosts",
                value: hubSnapshot.bootstrap.allowed_hosts.value.join(", ") || "-",
                source: hubSnapshot.bootstrap.allowed_hosts.source,
              },
              {
                label: "Allowed origins",
                value: hubSnapshot.bootstrap.allowed_origins.value.join(", ") || "-",
                source: hubSnapshot.bootstrap.allowed_origins.source,
              },
              {
                label: "Log level",
                value: hubSnapshot.bootstrap.log_level.value,
                source: hubSnapshot.bootstrap.log_level.source,
              },
              {
                label: "Log file",
                value: hubSnapshot.bootstrap.log_file.value ?? "-",
                source: hubSnapshot.bootstrap.log_file.source,
              },
              {
                label: "Log format",
                value: hubSnapshot.bootstrap.log_format.value,
                source: hubSnapshot.bootstrap.log_format.source,
              },
              {
                label: "No color",
                value: String(hubSnapshot.bootstrap.log_no_color.value),
                source: hubSnapshot.bootstrap.log_no_color.source,
              },
            ]
          : []
        : nodeSnapshot
          ? [
              {
                label: "Data directory",
                value: nodeSnapshot.bootstrap.fxg_home.value,
                source: nodeSnapshot.bootstrap.fxg_home.source,
              },
              {
                label: "Node ID",
                value: nodeSnapshot.bootstrap.node_id.value,
                source: nodeSnapshot.bootstrap.node_id.source,
              },
              {
                label: "Listen address",
                value: nodeSnapshot.bootstrap.listen_addr.value,
                source: nodeSnapshot.bootstrap.listen_addr.source,
              },
              {
                label: "Hub",
                value: nodeSnapshot.bootstrap.hub_url.value ?? "-",
                source: nodeSnapshot.bootstrap.hub_url.source,
              },
              {
                label: "Log level",
                value: nodeSnapshot.bootstrap.log_level.value,
                source: nodeSnapshot.bootstrap.log_level.source,
              },
              {
                label: "Log file",
                value: nodeSnapshot.bootstrap.log_file.value ?? "-",
                source: nodeSnapshot.bootstrap.log_file.source,
              },
              {
                label: "Log format",
                value: nodeSnapshot.bootstrap.log_format.value,
                source: nodeSnapshot.bootstrap.log_format.source,
              },
              {
                label: "No color",
                value: String(nodeSnapshot.bootstrap.log_no_color.value),
                source: nodeSnapshot.bootstrap.log_no_color.source,
              },
            ]
          : []
      : [],
  );

  const filteredBootstrap = $derived(bootstrapRows);
</script>

<div class="flex flex-col gap-3">
  {#if isHub && (section === "bootstrap" || section === "machine")}
    <div class="flex flex-wrap items-center gap-2">
      <Label for="catalog-node">Target node</Label>
      <select
        id="catalog-node"
        class="border-input bg-background h-8 rounded-md border px-2 text-[13px]"
        bind:value={selectedNode}
      >
        {#each onlineNodes as node (node.node_id)}
          <option value={node.node_id}>{node.name} ({node.node_id})</option>
        {/each}
      </select>
      {#if selectedNode === ""}
        <span class="text-muted-foreground text-[13px]">
          No node is online. Node settings are read-only while offline.
        </span>
      {/if}
    </div>
  {/if}

  {#if error}
    <p class="text-destructive text-sm">{error}</p>
  {/if}

  {#if section === "bootstrap"}
    <Card class="gap-2 p-4">
      <h2 class="font-medium">Bootstrap (read-only)</h2>
      <p class="text-muted-foreground text-[13px]">
        Process start-up values from config.toml, environment variables and CLI flags. Change them
        in the file and restart the process.
      </p>
      {#each filteredBootstrap as row (row.label)}
        <div class="flex flex-wrap items-center gap-2 border-b py-1.5 last:border-b-0">
          <span class="w-40 text-[13px] font-medium">{row.label}</span>
          <code class="min-w-0 flex-1 truncate text-[13px]">{row.value}</code>
          <Badge variant="outline" class="text-[11px]">{sourceBadge(row.source)}</Badge>
        </div>
      {/each}
      {#if filteredBootstrap.length === 0}
        <p class="text-muted-foreground text-[13px]">
          {loading ? "Loading…" : "No values available."}
        </p>
      {/if}
    </Card>
  {:else if section === "machine"}
    <Card class="gap-3 p-4">
      <h2 class="font-medium">This machine</h2>
      <p class="text-muted-foreground text-[13px]">
        Owned by <code>node.db</code>. The hub proxies writes to the target node; offline nodes are
        read-only.
      </p>
      {#if nodeSnapshot}
        <div class="grid gap-3 sm:grid-cols-2">
          <div class="grid gap-1.5">
            <Label for="node-name">Display name</Label>
            <Input id="node-name" bind:value={nodeForm.name} />
          </div>
          <div class="grid gap-1.5">
            <Label for="node-tray">Tray</Label>
            <select
              id="node-tray"
              class="border-input bg-background h-9 rounded-md border px-2 text-sm"
              bind:value={nodeForm.tray}
            >
              <option value="">Default</option>
              <option value="true">Enabled</option>
              <option value="false">Disabled</option>
            </select>
          </div>
          <label class="flex items-center gap-2 text-sm">
            <input type="checkbox" bind:checked={nodeForm.allow_remote_pty} />
            Allow remote Web PTY
          </label>
          <div class="grid gap-1.5 sm:col-span-2">
            <Label for="node-scan">Project scan directories (one per line)</Label>
            <textarea
              id="node-scan"
              class="border-input bg-background min-h-24 rounded-md border p-2 font-mono text-[13px]"
              bind:value={nodeForm.project_scan_dirs}></textarea>
          </div>
        </div>
        <div class="flex items-center gap-2">
          <Button size="sm" disabled={isHub && targetNode === ""} onclick={() => void saveNode()}>
            Save
          </Button>
          <span class="text-muted-foreground text-[13px]">revision {nodeForm.revision}</span>
        </div>
      {:else}
        <p class="text-muted-foreground text-[13px]">
          {loading ? "Loading…" : "Node settings are unavailable."}
        </p>
      {/if}
    </Card>
  {:else if section === "hub"}
    {#if !isHub}
      <Card class="gap-2 p-4">
        <h2 class="font-medium">Hub settings</h2>
        <p class="text-muted-foreground text-[13px]">
          Hub-owned settings live in <code>hub.db</code>. Connect to the hub to edit them.
        </p>
      </Card>
    {:else}
      <Card class="gap-3 p-4">
        <h2 class="font-medium">Hub</h2>
        <p class="text-muted-foreground text-[13px]">
          Owned by <code>hub.db</code>: the Web Push contact claim.
        </p>
        <div class="grid gap-1.5">
          <Label for="vapid-subject">VAPID subject</Label>
          <Input
            id="vapid-subject"
            placeholder="mailto:admin@example.com"
            bind:value={hubForm.vapid_subject}
          />
        </div>
        <div class="flex items-center gap-2">
          <Button size="sm" onclick={() => void saveHub()}>Save</Button>
          <span class="text-muted-foreground text-[13px]">revision {hubForm.revision}</span>
        </div>
      </Card>
    {/if}
  {/if}

  {#if showSecrets}
    <Card class="gap-3 p-4">
      <h2 class="font-medium">Secrets</h2>
      <p class="text-muted-foreground text-[13px]">
        Key-value secrets stored in <code>~/.flexagent/secrets/</code>. Values are write-only and
        never returned by the API. Reference them in custom agents as <code>@KEY</code> or in
        provisioner environment variables as <code>KEY</code>.
      </p>
      {#if secrets.length > 0}
        <ul class="flex list-none flex-col gap-1 p-0 text-[13px]">
          {#each secrets as secret (secret.id)}
            <li class="flex flex-wrap items-center gap-2 rounded-md border px-2 py-1.5">
              <span class="font-mono font-medium">{secret.id}</span>
              <Button
                variant="ghost"
                size="sm"
                class="h-6 px-1.5 text-[11px]"
                onclick={() => {
                  if (typeof navigator !== "undefined" && navigator.clipboard) {
                    void navigator.clipboard.writeText(secret.id);
                    toast.success(`Copied key "${secret.id}" to clipboard.`);
                  }
                }}
              >
                Copy key
              </Button>
              {#if secret.description}
                <span class="text-muted-foreground">{secret.description}</span>
              {/if}
              <Badge variant={secret.configured ? "secondary" : "outline"} class="text-[11px]">
                {secret.configured ? "Configured" : "Missing"}
              </Badge>
              {#if secret.used_by.length > 0}
                <span class="text-muted-foreground">used by {secret.used_by.join(", ")}</span>
              {/if}
              <div class="ml-auto flex gap-1">
                <Button
                  variant="ghost"
                  size="sm"
                  class="text-destructive"
                  onclick={() => void deleteSecret(secret)}
                  disabled={secret.used_by.length > 0}
                >
                  Delete
                </Button>
              </div>
            </li>
          {/each}
        </ul>
      {/if}
      <form class="grid gap-2 sm:grid-cols-4" onsubmit={saveSecret}>
        <Input
          placeholder="Key (e.g. OPENAI_API_KEY)"
          aria-label={m.settings_secret_key_label()}
          bind:value={secretForm.key}
        />
        <Input
          placeholder="Description (optional)"
          aria-label={m.settings_secret_description_label()}
          bind:value={secretForm.description}
        />
        <Input
          type="password"
          placeholder="Value (write-only)"
          aria-label={m.settings_secret_value_label()}
          autocomplete="new-password"
          bind:value={secretForm.value}
        />
        <Button type="submit" disabled={secretForm.key.trim() === "" || secretForm.value === ""}>
          Store secret
        </Button>
      </form>
    </Card>
  {/if}
</div>
